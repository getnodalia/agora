//! Channels, private channels and direct conversations.

use std::collections::HashMap;

use agora_core::{ChannelId, ChannelKind, MessageId, Role, UserId, WorkspaceId};
use agora_protocol::{Channel, Timestamp, User};
use sqlx::Row;

use crate::{conflict, rows, Result, Store, StoreError};

/// A channel as stored, without anything that depends on who is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub topic: String,
    pub kind: ChannelKind,
    pub archived: bool,
    pub is_default: bool,
    pub created_by: Option<UserId>,
    pub created_at: Timestamp,
}

const CHANNEL_SELECT: &str = "
    SELECT c.id, c.workspace_id, c.name, c.topic, c.kind, c.archived, c.is_default, c.created_by,
           c.created_at,
           cm.user_id IS NOT NULL AS joined,
           (SELECT COUNT(*) FROM channel_members x WHERE x.channel_id = c.id) AS member_count,
           (SELECT MAX(m.id) FROM messages m WHERE m.channel_id = c.id AND m.deleted_at IS NULL) AS last_id,
           CASE WHEN cm.user_id IS NULL THEN 0 ELSE
             (SELECT COUNT(*) FROM messages m WHERE m.channel_id = c.id AND m.id > cm.last_read_id
                AND m.author_id <> ?1 AND m.deleted_at IS NULL) END AS unread,
           CASE WHEN cm.user_id IS NULL THEN 0 ELSE
             (SELECT COUNT(*) FROM mentions n WHERE n.channel_id = c.id AND n.user_id = ?1
                AND n.message_id > cm.last_read_id) END AS mention_count
    FROM channels c
    LEFT JOIN channel_members cm ON cm.channel_id = c.id AND cm.user_id = ?1";

fn channel_row(r: &sqlx::sqlite::SqliteRow) -> Result<ChannelRow> {
    Ok(ChannelRow {
        id: ChannelId(r.try_get("id")?),
        workspace_id: WorkspaceId(r.try_get("workspace_id")?),
        name: r.try_get("name")?,
        topic: r.try_get("topic")?,
        kind: rows::kind(r.try_get("kind")?)?,
        archived: r.try_get("archived")?,
        is_default: r.try_get("is_default")?,
        created_by: r.try_get::<Option<i64>, _>("created_by")?.map(UserId),
        created_at: r.try_get("created_at")?,
    })
}

fn channel_view(r: &sqlx::sqlite::SqliteRow) -> Result<Channel> {
    let row = channel_row(r)?;
    Ok(Channel {
        id: row.id,
        workspace_id: row.workspace_id,
        name: row.name,
        topic: row.topic,
        kind: row.kind,
        archived: row.archived,
        created_by: row.created_by,
        created_at: row.created_at,
        joined: r.try_get("joined")?,
        unread: rows::count(r.try_get("unread")?),
        mentions: rows::count(r.try_get("mention_count")?),
        last_message_id: rows::opt_message_id(r.try_get("last_id")?),
        member_count: rows::count(r.try_get("member_count")?),
        peer: None,
    })
}

impl Store {
    pub async fn channel(&self, id: ChannelId) -> Result<Option<ChannelRow>> {
        let row = sqlx::query(
            "SELECT id, workspace_id, name, topic, kind, archived, is_default, created_by, created_at
             FROM channels WHERE id = ?",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| channel_row(&r)).transpose()
    }

    /// Whether `user` is a member of the channel.
    pub async fn is_channel_member(&self, channel: ChannelId, user: UserId) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM channel_members WHERE channel_id = ? AND user_id = ?",
        )
        .bind(channel.0)
        .bind(user.0)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// The channels `user` sees in a workspace: the ones they belong to, plus (unless they are a
    /// guest) every public channel, each with their unread counts. One query, plus one for the
    /// peers of direct conversations.
    pub async fn channels_for(
        &self,
        workspace: WorkspaceId,
        user: UserId,
        role: Role,
    ) -> Result<Vec<Channel>> {
        let sql = format!(
            "{CHANNEL_SELECT} WHERE c.workspace_id = ?2 AND (cm.user_id IS NOT NULL OR (c.kind = 'public' AND ?3))
             ORDER BY c.kind, c.name"
        );
        let rows = sqlx::query(&sql)
            .bind(user.0)
            .bind(workspace.0)
            .bind(role != Role::Guest)
            .fetch_all(&self.pool)
            .await?;
        let mut channels = rows.iter().map(channel_view).collect::<Result<Vec<_>>>()?;
        let peers = self.direct_peers(workspace, user).await?;
        for c in &mut channels {
            if c.kind == ChannelKind::Direct {
                if let Some(peer) = peers.get(&c.id) {
                    c.name = peer.username.clone();
                    c.peer = Some(peer.clone());
                }
            }
        }
        Ok(channels)
    }

    /// One channel as `user` sees it (without checking that they may).
    pub async fn channel_view(&self, channel: ChannelId, user: UserId) -> Result<Option<Channel>> {
        let sql = format!("{CHANNEL_SELECT} WHERE c.id = ?2");
        let row = sqlx::query(&sql)
            .bind(user.0)
            .bind(channel.0)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(None) };
        let mut c = channel_view(&row)?;
        if c.kind == ChannelKind::Direct {
            if let Some(peer) = self.direct_peers(c.workspace_id, user).await?.remove(&c.id) {
                c.name = peer.username.clone();
                c.peer = Some(peer);
            }
        }
        Ok(Some(c))
    }

    async fn direct_peers(
        &self,
        workspace: WorkspaceId,
        user: UserId,
    ) -> Result<HashMap<ChannelId, User>> {
        let rows = sqlx::query(
            "SELECT other.channel_id, u.id, u.username, u.display_name
             FROM channel_members me
             JOIN channels c ON c.id = me.channel_id AND c.kind = 'direct' AND c.workspace_id = ?
             JOIN channel_members other ON other.channel_id = c.id AND other.user_id <> me.user_id
             JOIN users u ON u.id = other.user_id
             WHERE me.user_id = ?",
        )
        .bind(workspace.0)
        .bind(user.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((ChannelId(r.try_get("channel_id")?), rows::user(r, "")?)))
            .collect()
    }

    /// Create a public or private channel; its creator joins it.
    pub async fn create_channel(
        &self,
        workspace: WorkspaceId,
        name: &str,
        topic: &str,
        kind: ChannelKind,
        by: UserId,
        now: Timestamp,
    ) -> Result<ChannelId> {
        if kind == ChannelKind::Direct {
            return Err(StoreError::Corrupt(
                "direct conversations are opened, not created".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO channels (workspace_id, name, topic, kind, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(workspace.0)
        .bind(name)
        .bind(topic)
        .bind(kind.as_str())
        .bind(by.0)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(conflict)?;
        sqlx::query(
            "INSERT INTO channel_members (channel_id, user_id, joined_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(by.0)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ChannelId(id))
    }

    /// Find or create the direct conversation between two members of a workspace.
    pub async fn open_direct(
        &self,
        workspace: WorkspaceId,
        a: UserId,
        b: UserId,
        now: Timestamp,
    ) -> Result<ChannelId> {
        let key = format!("{}:{}", a.0.min(b.0), a.0.max(b.0));
        let mut tx = self.pool.begin().await?;
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM channels WHERE workspace_id = ? AND kind = 'direct' AND direct_key = ?",
        )
        .bind(workspace.0)
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?;
        let id = match existing {
            Some(id) => id,
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO channels (workspace_id, name, topic, kind, direct_key, created_by, created_at)
                     VALUES (?, ?, '', 'direct', ?, ?, ?) RETURNING id",
                )
                .bind(workspace.0)
                .bind(&key)
                .bind(&key)
                .bind(a.0)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
                for member in [a, b] {
                    sqlx::query("INSERT OR IGNORE INTO channel_members (channel_id, user_id, joined_at) VALUES (?, ?, ?)")
                        .bind(id)
                        .bind(member.0)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                }
                id
            }
        };
        tx.commit().await?;
        Ok(ChannelId(id))
    }

    /// Add a member, read up to the latest message. Returns false when they already were one.
    pub async fn join_channel(
        &self,
        channel: ChannelId,
        user: UserId,
        now: Timestamp,
    ) -> Result<bool> {
        let done = sqlx::query(
            "INSERT OR IGNORE INTO channel_members (channel_id, user_id, joined_at, last_read_id)
             VALUES (?1, ?2, ?3, COALESCE((SELECT MAX(id) FROM messages WHERE channel_id = ?1), 0))",
        )
        .bind(channel.0)
        .bind(user.0)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn leave_channel(&self, channel: ChannelId, user: UserId) -> Result<bool> {
        let done = sqlx::query("DELETE FROM channel_members WHERE channel_id = ? AND user_id = ?")
            .bind(channel.0)
            .bind(user.0)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn update_channel(
        &self,
        channel: ChannelId,
        name: Option<&str>,
        topic: Option<&str>,
        archived: Option<bool>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE channels SET name = COALESCE(?, name), topic = COALESCE(?, topic),
             archived = COALESCE(?, archived) WHERE id = ?",
        )
        .bind(name)
        .bind(topic)
        .bind(archived)
        .bind(channel.0)
        .execute(&self.pool)
        .await
        .map_err(conflict)?;
        Ok(())
    }

    /// Who may read a channel right now, and so receives its live events: its members, plus every
    /// non-guest member of the workspace when it is public.
    pub async fn channel_audience(&self, channel: &ChannelRow) -> Result<Vec<UserId>> {
        let ids: Vec<i64> = if channel.kind == ChannelKind::Public {
            sqlx::query_scalar(
                "SELECT user_id FROM workspace_members WHERE workspace_id = ? AND role <> 'guest'
                 UNION SELECT user_id FROM channel_members WHERE channel_id = ?",
            )
            .bind(channel.workspace_id.0)
            .bind(channel.id.0)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar("SELECT user_id FROM channel_members WHERE channel_id = ?")
                .bind(channel.id.0)
                .fetch_all(&self.pool)
                .await?
        };
        Ok(ids.into_iter().map(UserId).collect())
    }

    /// Move the read marker forward (never back). Returns the marker now stored.
    pub async fn mark_read(
        &self,
        channel: ChannelId,
        user: UserId,
        message: MessageId,
    ) -> Result<Option<MessageId>> {
        let v: Option<i64> = sqlx::query_scalar(
            "UPDATE channel_members
             SET last_read_id = MAX(last_read_id, MIN(?1, (SELECT COALESCE(MAX(id), 0) FROM messages WHERE channel_id = ?2))),
                 delivered_id = MAX(delivered_id, MIN(?1, (SELECT COALESCE(MAX(id), 0) FROM messages WHERE channel_id = ?2)))
             WHERE channel_id = ?2 AND user_id = ?3 RETURNING last_read_id",
        )
        .bind(message.0)
        .bind(channel.0)
        .bind(user.0)
        .fetch_optional(&self.pool)
        .await?;
        Ok(v.map(MessageId))
    }

    /// Move the delivery marker forward (never back, never past the channel's newest message).
    /// Returns the receipt now stored, or `None` when `user` is not a member.
    pub async fn mark_delivered(
        &self,
        channel: ChannelId,
        user: UserId,
        message: MessageId,
    ) -> Result<Option<Receipt>> {
        let row: Option<(i64, i64)> = sqlx::query_as(
            "UPDATE channel_members
             SET delivered_id = MAX(delivered_id, MIN(?1, (SELECT COALESCE(MAX(id), 0) FROM messages WHERE channel_id = ?2)))
             WHERE channel_id = ?2 AND user_id = ?3 RETURNING delivered_id, last_read_id",
        )
        .bind(message.0)
        .bind(channel.0)
        .bind(user.0)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(delivered, read)| Receipt {
            user,
            delivered: MessageId(delivered),
            read: MessageId(read),
        }))
    }

    /// Every member's receipt in a channel.
    pub async fn receipts(&self, channel: ChannelId) -> Result<Vec<Receipt>> {
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT user_id, delivered_id, last_read_id FROM channel_members
             WHERE channel_id = ? ORDER BY user_id",
        )
        .bind(channel.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(user, delivered, read)| Receipt {
                user: UserId(user),
                delivered: MessageId(delivered),
                read: MessageId(read),
            })
            .collect())
    }

    /// One member's receipt in a channel.
    pub async fn receipt(&self, channel: ChannelId, user: UserId) -> Result<Option<Receipt>> {
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT delivered_id, last_read_id FROM channel_members
             WHERE channel_id = ? AND user_id = ?",
        )
        .bind(channel.0)
        .bind(user.0)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(delivered, read)| Receipt {
            user,
            delivered: MessageId(delivered),
            read: MessageId(read),
        }))
    }
}

/// How far one member of a channel has received and read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub user: UserId,
    pub delivered: MessageId,
    pub read: MessageId,
}
