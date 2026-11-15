//! Accounts and sessions.

use agora_core::UserId;
use agora_protocol::{Timestamp, User};
use sqlx::Row;

use crate::{conflict, rows, Result, Store};

/// A session found by its token.
#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: i64,
    pub user: User,
    pub expires_at: Timestamp,
}

impl Store {
    pub async fn create_user(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
        now: Timestamp,
    ) -> Result<User> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, display_name, password_hash, created_at)
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(conflict)?;
        Ok(User { id: UserId(id), username: username.to_owned(), display_name: display_name.to_owned() })
    }

    /// The user and their password hash, for logging in.
    pub async fn user_credentials(&self, username: &str) -> Result<Option<(User, String)>> {
        let row = sqlx::query("SELECT id, username, display_name, password_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| Ok((rows::user(&r, "")?, r.try_get("password_hash")?))).transpose()
    }

    pub async fn user(&self, id: UserId) -> Result<Option<User>> {
        let row = sqlx::query("SELECT id, username, display_name FROM users WHERE id = ?")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| rows::user(&r, "")).transpose()
    }

    pub async fn user_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&self.pool).await?)
    }

    pub async fn set_display_name(&self, id: UserId, display_name: &str) -> Result<()> {
        sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
            .bind(display_name)
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        token_hash: &[u8],
        user: UserId,
        now: Timestamp,
        expires_at: Timestamp,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(token_hash)
        .bind(user.0)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .fetch_one(&self.pool)
        .await?)
    }

    /// The live session for a token hash, or `None` when it does not exist or has expired.
    pub async fn session(&self, token_hash: &[u8], now: Timestamp) -> Result<Option<SessionRow>> {
        let row = sqlx::query(
            "SELECT s.id AS session_id, s.expires_at, u.id, u.username, u.display_name
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ? AND s.expires_at > ?",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(SessionRow { id: r.try_get("session_id")?, user: rows::user(&r, "")?, expires_at: r.try_get("expires_at")? })
        })
        .transpose()
    }

    /// Record activity on a session at most once a minute, to keep writes off the hot path.
    pub async fn touch_session(&self, id: i64, now: Timestamp) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ? AND last_seen_at < ?")
            .bind(now)
            .bind(id)
            .bind(now - 60_000)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a session. Returns its id when it existed.
    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar("DELETE FROM sessions WHERE token_hash = ? RETURNING id")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn delete_expired_sessions(&self, now: Timestamp) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM sessions WHERE expires_at <= ?").bind(now).execute(&self.pool).await?.rows_affected())
    }
}
