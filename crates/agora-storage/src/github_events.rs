//! Minimal server-readable GitHub event metadata; payloads and secrets are not stored.
use crate::{Result, Store};

pub struct NewGitHubEvent<'a> {
    pub workspace: WorkspaceId,
    pub delivery: &'a str,
    pub hash: &'a [u8; 32],
    pub event: &'a str,
    pub action: Option<&'a str>,
    pub object_number: Option<i64>,
    pub now: i64,
}
use agora_core::WorkspaceId;
use agora_protocol::GitHubEvent;
use sqlx::Row;

impl Store {
    /// Returns false for a duplicate delivery or body. Concurrent inserts are serialized by SQLite.
    pub async fn record_github_event(&self, input: NewGitHubEvent<'_>) -> Result<bool> {
        let NewGitHubEvent {
            workspace: ws,
            delivery,
            hash,
            event,
            action,
            object_number,
            now,
        } = input;
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query("INSERT OR IGNORE INTO github_event_seen (workspace_id, delivery, payload_hash) VALUES (?, ?, ?)")
            .bind(ws.0).bind(delivery).bind(hash.as_slice())
            .execute(&mut *tx).await?.rows_affected() == 1;
        if inserted {
            sqlx::query("INSERT INTO github_events (workspace_id, delivery, payload_hash, event, action, object_number, received_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(ws.0).bind(delivery).bind(hash.as_slice()).bind(event).bind(action).bind(object_number).bind(now)
                .execute(&mut *tx).await?;
            sqlx::query("DELETE FROM github_events WHERE workspace_id = ? AND id NOT IN (SELECT id FROM github_events WHERE workspace_id = ? ORDER BY id DESC LIMIT 5000)")
                .bind(ws.0).bind(ws.0).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(inserted)
    }
    pub async fn github_events(&self, ws: WorkspaceId) -> Result<Vec<GitHubEvent>> {
        let rows = sqlx::query("SELECT id, event, action, object_number, received_at FROM github_events WHERE workspace_id = ? ORDER BY id DESC LIMIT 100")
            .bind(ws.0).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok(GitHubEvent {
                    id: row.try_get("id")?,
                    event: row.try_get("event")?,
                    action: row.try_get("action")?,
                    object_number: row.try_get("object_number")?,
                    received_at: row.try_get("received_at")?,
                })
            })
            .collect()
    }
}
