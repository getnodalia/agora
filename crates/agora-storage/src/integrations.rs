//! Incoming webhooks and the notices they post. Server-readable, separate from encrypted
//! messages.

use agora_core::{ChannelId, UserId};
use agora_protocol::{Integration, IntegrationKind, IntegrationNotice};
use sqlx::Row;

use crate::{Result, Store};

/// The most notices kept per channel; older ones are removed as new ones arrive.
const NOTICES_KEPT: i64 = 1_000;

/// A notice an integration posts, already checked.
pub struct NewNotice<'a> {
    pub integration: i64,
    pub channel: ChannelId,
    pub hash: &'a [u8; 32],
    pub title: Option<&'a str>,
    pub text: &'a str,
    pub url: Option<&'a str>,
    pub now: i64,
}

fn kind_of(s: &str) -> IntegrationKind {
    match s {
        "hito" => IntegrationKind::Hito,
        _ => IntegrationKind::Webhook,
    }
}

fn kind_str(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Hito => "hito",
        IntegrationKind::Webhook => "webhook",
    }
}

fn integration(row: &sqlx::sqlite::SqliteRow) -> Result<Integration> {
    Ok(Integration {
        id: row.try_get("id")?,
        channel_id: ChannelId(row.try_get("channel_id")?),
        kind: kind_of(row.try_get::<&str, _>("kind")?),
        name: row.try_get("name")?,
        created_by: UserId(row.try_get("created_by")?),
        created_at: row.try_get("created_at")?,
    })
}

fn notice(row: &sqlx::sqlite::SqliteRow) -> Result<IntegrationNotice> {
    Ok(IntegrationNotice {
        id: row.try_get("id")?,
        integration_id: row.try_get("integration_id")?,
        channel_id: ChannelId(row.try_get("channel_id")?),
        integration_name: row.try_get("name")?,
        kind: kind_of(row.try_get::<&str, _>("kind")?),
        title: row.try_get("title")?,
        text: row.try_get("text")?,
        url: row.try_get("url")?,
        created_at: row.try_get("created_at")?,
    })
}

const NOTICE_SELECT: &str = "SELECT n.id, n.integration_id, n.channel_id, i.name, i.kind, n.title,
    n.text, n.url, n.created_at
    FROM integration_notices n JOIN integrations i ON i.id = n.integration_id";

impl Store {
    pub async fn create_integration(
        &self,
        channel: ChannelId,
        kind: IntegrationKind,
        name: &str,
        secret: &[u8; 32],
        by: UserId,
        now: i64,
    ) -> Result<Integration> {
        let row = sqlx::query(
            "INSERT INTO integrations (channel_id, kind, name, secret, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id, channel_id, kind, name, created_by, created_at",
        )
        .bind(channel.0)
        .bind(kind_str(kind))
        .bind(name)
        .bind(secret.as_slice())
        .bind(by.0)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        integration(&row)
    }

    pub async fn integrations(&self, channel: ChannelId) -> Result<Vec<Integration>> {
        let rows = sqlx::query(
            "SELECT id, channel_id, kind, name, created_by, created_at FROM integrations
             WHERE channel_id = ? ORDER BY id",
        )
        .bind(channel.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(integration).collect()
    }

    /// An integration and its secret, to check a request's signature.
    pub async fn integration_secret(&self, id: i64) -> Result<Option<(Integration, Vec<u8>)>> {
        let row = sqlx::query(
            "SELECT id, channel_id, kind, name, created_by, created_at, secret FROM integrations
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| Ok((integration(&row)?, row.try_get("secret")?)))
            .transpose()
    }

    pub async fn delete_integration(&self, id: i64) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM integrations WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            == 1)
    }

    /// Record a notice. `None` when the same body was already recorded for this integration.
    pub async fn record_notice(&self, input: NewNotice<'_>) -> Result<Option<IntegrationNotice>> {
        let mut tx = self.pool.begin().await?;
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT OR IGNORE INTO integration_notices
             (integration_id, channel_id, payload_hash, title, text, url, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(input.integration)
        .bind(input.channel.0)
        .bind(input.hash.as_slice())
        .bind(input.title)
        .bind(input.text)
        .bind(input.url)
        .bind(input.now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(id) = id else {
            tx.commit().await?;
            return Ok(None);
        };
        sqlx::query(
            "DELETE FROM integration_notices WHERE channel_id = ?1 AND id NOT IN
             (SELECT id FROM integration_notices WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2)",
        )
        .bind(input.channel.0)
        .bind(NOTICES_KEPT)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(&format!("{NOTICE_SELECT} WHERE n.id = ?"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(notice(&row)?))
    }

    /// The newest notices of a channel, oldest first.
    pub async fn notices(&self, channel: ChannelId, limit: u32) -> Result<Vec<IntegrationNotice>> {
        let rows = sqlx::query(&format!(
            "SELECT * FROM ({NOTICE_SELECT} WHERE n.channel_id = ? ORDER BY n.id DESC LIMIT ?)
             ORDER BY id"
        ))
        .bind(channel.0)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(notice).collect()
    }
}
