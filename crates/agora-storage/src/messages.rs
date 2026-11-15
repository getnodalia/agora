//! Messages, threads, reactions, mentions and search.

use std::collections::HashMap;

use agora_core::{ChannelId, MessageId, UserId, WorkspaceId};
use agora_protocol::{Message, Reaction, Timestamp};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::{rows, Result, Store, StoreError};

const MESSAGE_SELECT: &str = "
    SELECT m.id, m.channel_id, m.thread_root_id, m.body, m.client_nonce, m.created_at, m.edited_at,
           m.deleted_at, m.reply_count, m.last_reply_at,
           u.id AS u_id, u.username AS u_username, u.display_name AS u_display_name
    FROM messages m JOIN users u ON u.id = m.author_id";

/// Reactions listed per emoji: at most this many user ids.
const REACTION_USERS_MAX: usize = 50;

fn message(r: &SqliteRow) -> Result<Message> {
    let deleted = r.try_get::<Option<i64>, _>("deleted_at")?.is_some();
    Ok(Message {
        id: MessageId(r.try_get("id")?),
        channel_id: ChannelId(r.try_get("channel_id")?),
        author: rows::user(r, "u_")?,
        thread_root_id: rows::opt_message_id(r.try_get("thread_root_id")?),
        body: if deleted { String::new() } else { r.try_get("body")? },
        created_at: r.try_get("created_at")?,
        edited_at: r.try_get("edited_at")?,
        deleted,
        reply_count: rows::count(r.try_get("reply_count")?),
        last_reply_at: r.try_get("last_reply_at")?,
        reactions: Vec::new(),
        client_nonce: Some(r.try_get("client_nonce")?),
    })
}

/// A new message to store.
#[derive(Clone, Debug)]
pub struct NewMessage<'a> {
    pub channel: ChannelId,
    pub author: UserId,
    pub thread_root: Option<MessageId>,
    pub body: &'a str,
    pub nonce: &'a str,
    pub mentioned: &'a [UserId],
    pub now: Timestamp,
}

/// What [`Store::insert_message`] did.
#[derive(Clone, Debug)]
pub enum Inserted {
    /// A new message.
    Created(Message),
    /// The author already sent a message with this nonce; this is it, unchanged.
    Existing(Message),
}

impl Store {
    /// Store a message, once. If the author already sent one with the same nonce (a retry after a
    /// lost response), nothing is written and the stored message comes back as
    /// [`Inserted::Existing`]. A reply bumps its root's counters in the same transaction.
    pub async fn insert_message(&self, new: NewMessage<'_>) -> Result<Inserted> {
        let mut tx = self.pool.begin().await?;
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM messages WHERE author_id = ? AND client_nonce = ?")
            .bind(new.author.0)
            .bind(new.nonce)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(id) = existing {
            tx.rollback().await?;
            let message = self.message(MessageId(id)).await?.ok_or(StoreError::NotFound)?;
            return Ok(Inserted::Existing(message));
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO messages (channel_id, author_id, thread_root_id, body, client_nonce, created_at)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(new.channel.0)
        .bind(new.author.0)
        .bind(new.thread_root.map(|m| m.0))
        .bind(new.body)
        .bind(new.nonce)
        .bind(new.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(crate::conflict)?;
        if let Some(root) = new.thread_root {
            sqlx::query("UPDATE messages SET reply_count = reply_count + 1, last_reply_at = ? WHERE id = ?")
                .bind(new.now)
                .bind(root.0)
                .execute(&mut *tx)
                .await?;
        }
        for user in new.mentioned {
            sqlx::query("INSERT OR IGNORE INTO mentions (message_id, user_id, channel_id) VALUES (?, ?, ?)")
                .bind(id)
                .bind(user.0)
                .bind(new.channel.0)
                .execute(&mut *tx)
                .await?;
        }
        // The author has read what they wrote.
        sqlx::query("UPDATE channel_members SET last_read_id = MAX(last_read_id, ?) WHERE channel_id = ? AND user_id = ?")
            .bind(id)
            .bind(new.channel.0)
            .bind(new.author.0)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let message = self.message(MessageId(id)).await?.ok_or(StoreError::NotFound)?;
        Ok(Inserted::Created(message))
    }

    pub async fn message(&self, id: MessageId) -> Result<Option<Message>> {
        let sql = format!("{MESSAGE_SELECT} WHERE m.id = ?");
        let row = sqlx::query(&sql).bind(id.0).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(None) };
        let mut messages = vec![message(&row)?];
        self.attach_reactions(&mut messages).await?;
        Ok(messages.pop())
    }

    /// A page of top-level messages (replies live in threads), oldest first. `before` pages
    /// backwards from a message; `after` returns what arrived since one (for catching up after a
    /// reconnection). With neither, the latest page.
    pub async fn history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        after: Option<MessageId>,
        limit: u32,
    ) -> Result<(Vec<Message>, bool)> {
        let limit = i64::from(limit.max(1));
        let rows = if let Some(after) = after {
            let sql = format!("{MESSAGE_SELECT} WHERE m.channel_id = ? AND m.thread_root_id IS NULL AND m.id > ? ORDER BY m.id ASC LIMIT ?");
            sqlx::query(&sql).bind(channel.0).bind(after.0).bind(limit + 1).fetch_all(&self.pool).await?
        } else {
            let sql = format!("{MESSAGE_SELECT} WHERE m.channel_id = ? AND m.thread_root_id IS NULL AND m.id < ? ORDER BY m.id DESC LIMIT ?");
            sqlx::query(&sql)
                .bind(channel.0)
                .bind(before.map_or(i64::MAX, |b| b.0))
                .bind(limit + 1)
                .fetch_all(&self.pool)
                .await?
        };
        let has_more = rows.len() as i64 > limit;
        let mut messages = rows.iter().take(limit as usize).map(message).collect::<Result<Vec<_>>>()?;
        if after.is_none() {
            messages.reverse();
        }
        self.attach_reactions(&mut messages).await?;
        Ok((messages, has_more))
    }

    /// Every reply in a thread, oldest first (threads are bounded by the reply rate limit).
    pub async fn replies(&self, root: MessageId, limit: u32) -> Result<Vec<Message>> {
        let sql = format!("{MESSAGE_SELECT} WHERE m.thread_root_id = ? ORDER BY m.id ASC LIMIT ?");
        let rows = sqlx::query(&sql).bind(root.0).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        let mut messages = rows.iter().map(message).collect::<Result<Vec<_>>>()?;
        self.attach_reactions(&mut messages).await?;
        Ok(messages)
    }

    /// The author of a message and whether it is deleted (for permission checks).
    pub async fn message_meta(&self, id: MessageId) -> Result<Option<(ChannelId, UserId, Option<MessageId>, bool)>> {
        let row = sqlx::query("SELECT channel_id, author_id, thread_root_id, deleted_at FROM messages WHERE id = ?")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| {
            Ok((
                ChannelId(r.try_get("channel_id")?),
                UserId(r.try_get("author_id")?),
                rows::opt_message_id(r.try_get("thread_root_id")?),
                r.try_get::<Option<i64>, _>("deleted_at")?.is_some(),
            ))
        })
        .transpose()
    }

    pub async fn edit_message(&self, id: MessageId, body: &str, mentioned: &[UserId], now: Timestamp) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let channel: i64 = sqlx::query_scalar("UPDATE messages SET body = ?, edited_at = ? WHERE id = ? AND deleted_at IS NULL RETURNING channel_id")
            .bind(body)
            .bind(now)
            .bind(id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        sqlx::query("DELETE FROM mentions WHERE message_id = ?").bind(id.0).execute(&mut *tx).await?;
        for user in mentioned {
            sqlx::query("INSERT OR IGNORE INTO mentions (message_id, user_id, channel_id) VALUES (?, ?, ?)")
                .bind(id.0)
                .bind(user.0)
                .bind(channel)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Delete a message: its text is erased, its row stays so threads keep their shape.
    pub async fn delete_message(&self, id: MessageId, now: Timestamp) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let root: Option<Option<i64>> = sqlx::query_scalar(
            "UPDATE messages SET body = '', deleted_at = ? WHERE id = ? AND deleted_at IS NULL RETURNING thread_root_id",
        )
        .bind(now)
        .bind(id.0)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(root) = root else {
            return Err(StoreError::NotFound);
        };
        if let Some(root) = root {
            sqlx::query("UPDATE messages SET reply_count = MAX(reply_count - 1, 0) WHERE id = ?")
                .bind(root)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM mentions WHERE message_id = ?").bind(id.0).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM reactions WHERE message_id = ?").bind(id.0).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Add the reaction if the user has not made it, remove it if they have. Returns the
    /// message's reactions afterwards.
    pub async fn toggle_reaction(&self, message: MessageId, user: UserId, emoji: &str, now: Timestamp) -> Result<Vec<Reaction>> {
        let removed = sqlx::query("DELETE FROM reactions WHERE message_id = ? AND user_id = ? AND emoji = ?")
            .bind(message.0)
            .bind(user.0)
            .bind(emoji)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if removed == 0 {
            sqlx::query("INSERT INTO reactions (message_id, user_id, emoji, created_at) VALUES (?, ?, ?, ?)")
                .bind(message.0)
                .bind(user.0)
                .bind(emoji)
                .bind(now)
                .execute(&self.pool)
                .await?;
        }
        Ok(self.reactions(&[message]).await?.remove(&message).unwrap_or_default())
    }

    /// Distinct emoji a message carries.
    pub async fn reaction_kinds(&self, message: MessageId) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(DISTINCT emoji) FROM reactions WHERE message_id = ?")
            .bind(message.0)
            .fetch_one(&self.pool)
            .await?)
    }

    async fn reactions(&self, ids: &[MessageId]) -> Result<HashMap<MessageId, Vec<Reaction>>> {
        let mut out: HashMap<MessageId, Vec<Reaction>> = HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let mut q: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT message_id, emoji, user_id FROM reactions WHERE message_id IN (");
        let mut sep = q.separated(", ");
        for id in ids {
            sep.push_bind(id.0);
        }
        q.push(") ORDER BY message_id, created_at, user_id");
        for r in q.build().fetch_all(&self.pool).await? {
            let id = MessageId(r.try_get("message_id")?);
            let emoji: String = r.try_get("emoji")?;
            let user = UserId(r.try_get("user_id")?);
            let list = out.entry(id).or_default();
            match list.iter_mut().find(|x| x.emoji == emoji) {
                Some(x) => {
                    x.count = x.count.saturating_add(1);
                    if x.users.len() < REACTION_USERS_MAX {
                        x.users.push(user);
                    }
                }
                None => list.push(Reaction { emoji, count: 1, users: vec![user] }),
            }
        }
        Ok(out)
    }

    async fn attach_reactions(&self, messages: &mut [Message]) -> Result<()> {
        let ids: Vec<MessageId> = messages.iter().filter(|m| !m.deleted).map(|m| m.id).collect();
        let mut all = self.reactions(&ids).await?;
        for m in messages {
            m.reactions = all.remove(&m.id).unwrap_or_default();
        }
        Ok(())
    }

    /// Messages containing `query` (case-insensitive for ASCII) in the channels of `workspace`
    /// that `user` may read, newest first.
    pub async fn search(&self, workspace: WorkspaceId, user: UserId, guest: bool, query: &str, limit: u32) -> Result<Vec<Message>> {
        let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let sql = format!(
            "{MESSAGE_SELECT} JOIN channels c ON c.id = m.channel_id
             WHERE c.workspace_id = ? AND m.deleted_at IS NULL AND m.body LIKE ? ESCAPE '\\'
               AND (EXISTS (SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.user_id = ?)
                    OR (c.kind = 'public' AND NOT ?))
             ORDER BY m.id DESC LIMIT ?"
        );
        let rows = sqlx::query(&sql)
            .bind(workspace.0)
            .bind(pattern)
            .bind(user.0)
            .bind(guest)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?;
        let mut messages = rows.iter().map(message).collect::<Result<Vec<_>>>()?;
        self.attach_reactions(&mut messages).await?;
        Ok(messages)
    }

    /// Users of a workspace with the given usernames (to resolve `@mentions`).
    pub async fn resolve_usernames(&self, workspace: WorkspaceId, names: &[String]) -> Result<Vec<UserId>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let mut q: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT u.id FROM users u JOIN workspace_members w ON w.user_id = u.id WHERE w.workspace_id = ",
        );
        q.push_bind(workspace.0);
        q.push(" AND u.username IN (");
        let mut sep = q.separated(", ");
        for n in names.iter().take(50) {
            sep.push_bind(n);
        }
        q.push(")");
        let ids: Vec<i64> = q.build_query_scalar().fetch_all(&self.pool).await?;
        Ok(ids.into_iter().map(UserId).collect())
    }
}
