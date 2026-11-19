//! Password recovery: the recovery key, the operator's reset code and the password change they
//! lead to. Callers hash the codes; plaintext never reaches this module.

use agora_core::UserId;
use agora_protocol::Timestamp;
use sqlx::Row;

use crate::{Result, Store};

/// Which kind of code a reset used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// The recovery key the person keeps. The second factor still applies.
    Key,
    /// A code issued by whoever runs the server; it may also remove the second factor.
    Operator { remove_two_step: bool },
}

impl Store {
    /// Replace the account's recovery key.
    pub async fn set_recovery_key(
        &self,
        user: UserId,
        hash: &[u8; 32],
        now: Timestamp,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO recovery_keys (user_id, key_hash, created_at) VALUES (?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET key_hash = excluded.key_hash, created_at = excluded.created_at",
        )
        .bind(user.0)
        .bind(hash.as_slice())
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn has_recovery_key(&self, user: UserId) -> Result<bool> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recovery_keys WHERE user_id = ?")
                .bind(user.0)
                .fetch_one(&self.pool)
                .await?
                > 0,
        )
    }

    /// Issue a one-use reset code, replacing any earlier one.
    pub async fn set_reset_code(
        &self,
        user: UserId,
        hash: &[u8; 32],
        expires_at: Timestamp,
        remove_two_step: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO reset_codes (user_id, code_hash, expires_at, remove_two_step) VALUES (?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET code_hash = excluded.code_hash,
                 expires_at = excluded.expires_at, remove_two_step = excluded.remove_two_step",
        )
        .bind(user.0)
        .bind(hash.as_slice())
        .bind(expires_at)
        .bind(remove_two_step)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// What kind of reset these hashes would be, without using them up.
    pub async fn peek_reset(
        &self,
        user: UserId,
        operator_hash: &[u8; 32],
        key_hash: &[u8; 32],
        now: Timestamp,
    ) -> Result<Option<ResetKind>> {
        let row = sqlx::query(
            "SELECT remove_two_step FROM reset_codes WHERE user_id = ? AND code_hash = ? AND expires_at > ?",
        )
        .bind(user.0)
        .bind(operator_hash.as_slice())
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            return Ok(Some(ResetKind::Operator {
                remove_two_step: row.try_get::<bool, _>("remove_two_step")?,
            }));
        }
        let found: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM recovery_keys WHERE user_id = ? AND key_hash = ?",
        )
        .bind(user.0)
        .bind(key_hash.as_slice())
        .fetch_one(&self.pool)
        .await?;
        Ok((found > 0).then_some(ResetKind::Key))
    }

    /// Use up the code and set the new password in one step. Every session is signed out, a
    /// used recovery key stops working, and an operator code may also remove the second factor.
    /// `None` when neither hash matches (or the code has already been used).
    pub async fn reset_password(
        &self,
        user: UserId,
        operator_hash: &[u8; 32],
        key_hash: &[u8; 32],
        new_password_hash: &str,
        now: Timestamp,
    ) -> Result<Option<(ResetKind, Vec<i64>)>> {
        let mut tx = self.pool.begin().await?;
        let operator: Option<bool> = sqlx::query_scalar(
            "DELETE FROM reset_codes WHERE user_id = ? AND code_hash = ? AND expires_at > ? RETURNING remove_two_step",
        )
        .bind(user.0)
        .bind(operator_hash.as_slice())
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let kind = if let Some(remove_two_step) = operator {
            ResetKind::Operator { remove_two_step }
        } else {
            let used = sqlx::query("DELETE FROM recovery_keys WHERE user_id = ? AND key_hash = ?")
                .bind(user.0)
                .bind(key_hash.as_slice())
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if used == 0 {
                return Ok(None);
            }
            ResetKind::Key
        };
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(new_password_hash)
            .bind(user.0)
            .execute(&mut *tx)
            .await?;
        if kind
            == (ResetKind::Operator {
                remove_two_step: true,
            })
        {
            sqlx::query("DELETE FROM mfa_totp WHERE user_id = ?")
                .bind(user.0)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM reset_codes WHERE user_id = ?")
            .bind(user.0)
            .execute(&mut *tx)
            .await?;
        let sessions: Vec<i64> =
            sqlx::query_scalar("DELETE FROM sessions WHERE user_id = ? RETURNING id")
                .bind(user.0)
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(Some((kind, sessions)))
    }

    /// Change the password of a signed-in account and sign out every other session.
    pub async fn change_password(
        &self,
        user: UserId,
        new_password_hash: &str,
        keep_session: i64,
    ) -> Result<Vec<i64>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(new_password_hash)
            .bind(user.0)
            .execute(&mut *tx)
            .await?;
        let ids: Vec<i64> =
            sqlx::query_scalar("DELETE FROM sessions WHERE user_id = ? AND id <> ? RETURNING id")
                .bind(user.0)
                .bind(keep_session)
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(ids)
    }

    /// An account by username, for the operator's command.
    pub async fn user_id_by_username(&self, username: &str) -> Result<Option<UserId>> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ?")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?
                .map(UserId),
        )
    }
}
