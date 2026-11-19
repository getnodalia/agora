//! Workspaces, their members and invitations.

use agora_core::{ChannelKind, Role, UserId, WorkspaceId};
use agora_protocol::{Member, Timestamp, Workspace};
use sqlx::{Row, Sqlite, Transaction};

use crate::{rows, Result, Store, StoreError};

/// What redeeming an invitation did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Joined {
    pub workspace_id: WorkspaceId,
    pub role: Role,
    /// False when the person was already a member (the invitation was not used up).
    pub new_member: bool,
}

/// An invitation to store. Only the hash of its code is kept.
#[derive(Clone, Copy, Debug)]
pub struct NewInvite<'a> {
    pub workspace: WorkspaceId,
    pub code_hash: &'a [u8],
    pub role: Role,
    pub by: UserId,
    pub max_uses: u32,
    pub expires_at: Timestamp,
    pub now: Timestamp,
}

impl Store {
    /// The stored PNG is metadata visible to workspace members, not encrypted content.
    pub async fn workspace_icon(&self, id: WorkspaceId) -> Result<Option<Vec<u8>>> {
        Ok(
            sqlx::query_scalar::<_, Option<Vec<u8>>>(
                "SELECT icon_png FROM workspaces WHERE id = ?",
            )
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await?
            .flatten(),
        )
    }

    /// Set or remove an icon. Authorization belongs to the caller.
    pub async fn set_workspace_icon(&self, id: WorkspaceId, icon: Option<&[u8]>) -> Result<()> {
        let result = sqlx::query("UPDATE workspaces SET icon_png = ? WHERE id = ?")
            .bind(icon)
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Create a workspace owned by `owner`, with a default `#general` channel.
    pub async fn create_workspace(
        &self,
        name: &str,
        owner: UserId,
        now: Timestamp,
    ) -> Result<WorkspaceId> {
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO workspaces (name, created_by, created_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(owner.0)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO workspace_members (workspace_id, user_id, role, joined_at) VALUES (?, ?, 'owner', ?)")
            .bind(id)
            .bind(owner.0)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let channel: i64 = sqlx::query_scalar(
            "INSERT INTO channels (workspace_id, name, topic, kind, is_default, created_by, created_at)
             VALUES (?, 'general', '', 'public', 1, ?, ?) RETURNING id",
        )
        .bind(id)
        .bind(owner.0)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO channel_members (channel_id, user_id, joined_at) VALUES (?, ?, ?)",
        )
        .bind(channel)
        .bind(owner.0)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(WorkspaceId(id))
    }

    /// Delete a workspace with everything in it: channels, messages, files, tasks, invitations,
    /// integrations and encrypted logs all go through the foreign keys' `ON DELETE CASCADE`.
    /// Returns whether a workspace existed.
    pub async fn delete_workspace(&self, id: WorkspaceId) -> Result<bool> {
        let done = sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn workspaces_of(&self, user: UserId) -> Result<Vec<Workspace>> {
        let rows = sqlx::query(
            "SELECT w.id, w.name, w.created_at, m.role FROM workspaces w
             JOIN workspace_members m ON m.workspace_id = w.id
             WHERE m.user_id = ? ORDER BY m.joined_at, w.id",
        )
        .bind(user.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Workspace {
                    id: WorkspaceId(r.try_get("id")?),
                    name: r.try_get("name")?,
                    role: rows::role(r.try_get("role")?)?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }

    /// The user's role in a workspace, or `None` when they are not a member.
    pub async fn role(&self, workspace: WorkspaceId, user: UserId) -> Result<Option<Role>> {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = ? AND user_id = ?",
        )
        .bind(workspace.0)
        .bind(user.0)
        .fetch_optional(&self.pool)
        .await?;
        role.map(|r| rows::role(&r)).transpose()
    }

    pub async fn workspace(&self, id: WorkspaceId, user: UserId) -> Result<Option<Workspace>> {
        Ok(self
            .workspaces_of(user)
            .await?
            .into_iter()
            .find(|w| w.id == id))
    }

    pub async fn members(&self, workspace: WorkspaceId) -> Result<Vec<Member>> {
        let rows = sqlx::query(
            "SELECT u.id, u.username, u.display_name, m.role, m.joined_at FROM workspace_members m
             JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ? ORDER BY u.display_name, u.id",
        )
        .bind(workspace.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Member {
                    user: rows::user(r, "")?,
                    role: rows::role(r.try_get("role")?)?,
                    joined_at: r.try_get("joined_at")?,
                    online: false,
                })
            })
            .collect()
    }

    pub async fn member_count(&self, workspace: WorkspaceId) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM workspace_members WHERE workspace_id = ?")
                .bind(workspace.0)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Everyone who shares at least one workspace with `user` (for presence).
    pub async fn colleagues(&self, user: UserId) -> Result<Vec<UserId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT b.user_id FROM workspace_members a
             JOIN workspace_members b ON b.workspace_id = a.workspace_id
             WHERE a.user_id = ? AND b.user_id <> ?",
        )
        .bind(user.0)
        .bind(user.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(ids.into_iter().map(UserId).collect())
    }

    pub async fn workspace_member_ids(&self, workspace: WorkspaceId) -> Result<Vec<UserId>> {
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT user_id FROM workspace_members WHERE workspace_id = ?")
                .bind(workspace.0)
                .fetch_all(&self.pool)
                .await?;
        Ok(ids.into_iter().map(UserId).collect())
    }

    pub async fn create_invite(&self, invite: NewInvite<'_>) -> Result<i64> {
        let NewInvite {
            workspace,
            code_hash,
            role,
            by,
            max_uses,
            expires_at,
            now,
        } = invite;
        Ok(sqlx::query_scalar(
            "INSERT INTO invites (workspace_id, code_hash, role, created_by, max_uses, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(workspace.0)
        .bind(code_hash)
        .bind(role.as_str())
        .bind(by.0)
        .bind(i64::from(max_uses))
        .bind(expires_at)
        .bind(now)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Use an invitation: add the user to its workspace (and its default channels) in one
    /// transaction, counting the use. Unknown codes are [`StoreError::NotFound`]; expired or used
    /// up ones are [`StoreError::InviteExhausted`].
    pub async fn redeem_invite(
        &self,
        code_hash: &[u8],
        user: UserId,
        now: Timestamp,
    ) -> Result<Joined> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT id, workspace_id, role, max_uses, uses, expires_at FROM invites WHERE code_hash = ?")
            .bind(code_hash)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        let invite: i64 = row.try_get("id")?;
        let workspace = WorkspaceId(row.try_get("workspace_id")?);
        let role = rows::role(row.try_get("role")?)?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = ? AND user_id = ?",
        )
        .bind(workspace.0)
        .bind(user.0)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            return Ok(Joined {
                workspace_id: workspace,
                role: rows::role(&existing)?,
                new_member: false,
            });
        }
        let uses: i64 = row.try_get("uses")?;
        let max_uses: i64 = row.try_get("max_uses")?;
        let expires_at: i64 = row.try_get("expires_at")?;
        if uses >= max_uses || expires_at <= now {
            return Err(StoreError::InviteExhausted);
        }
        sqlx::query("UPDATE invites SET uses = uses + 1 WHERE id = ?")
            .bind(invite)
            .execute(&mut *tx)
            .await?;
        add_member(&mut tx, workspace, user, role, now).await?;
        tx.commit().await?;
        Ok(Joined {
            workspace_id: workspace,
            role,
            new_member: true,
        })
    }

    /// Atomically consume an invitation, create an account and join its workspace.
    /// Any failure rolls back all three operations, including the invitation use.
    pub async fn create_invited_user(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
        code_hash: &[u8],
        now: Timestamp,
    ) -> Result<(agora_protocol::User, Joined)> {
        let mut tx = self.pool.begin().await?;
        // The conditional write takes SQLite's write lock before any reads, serializing
        // competing registrations and rechecking expiry after password hashing.
        let row = sqlx::query(
            "UPDATE invites SET uses = uses + 1
             WHERE code_hash = ? AND uses < max_uses AND expires_at > ?
             RETURNING workspace_id, role",
        )
        .bind(code_hash)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::InviteExhausted)?;
        let workspace = WorkspaceId(row.try_get("workspace_id")?);
        let role = rows::role(row.try_get("role")?)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, display_name, password_hash, created_at)
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(crate::conflict)?;
        let user = agora_protocol::User {
            id: UserId(id),
            username: username.to_owned(),
            display_name: display_name.to_owned(),
        };
        add_member(&mut tx, workspace, user.id, role, now).await?;
        tx.commit().await?;
        Ok((
            user,
            Joined {
                workspace_id: workspace,
                role,
                new_member: true,
            },
        ))
    }

    /// Add a member directly (used by tests and administration tools).
    pub async fn add_member(
        &self,
        workspace: WorkspaceId,
        user: UserId,
        role: Role,
        now: Timestamp,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        add_member(&mut tx, workspace, user, role, now).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn add_member(
    tx: &mut Transaction<'_, Sqlite>,
    workspace: WorkspaceId,
    user: UserId,
    role: Role,
    now: Timestamp,
) -> Result<()> {
    sqlx::query("INSERT INTO workspace_members (workspace_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)")
        .bind(workspace.0)
        .bind(user.0)
        .bind(role.as_str())
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(crate::conflict)?;
    // A company workspace invitation also admits the invitee to the company, without
    // changing any existing company role. Only the company owner can issue these invites.
    sqlx::query("INSERT OR IGNORE INTO organization_members (organization_id, user_id, role, joined_at) SELECT organization_id, ?, ?, ? FROM workspaces WHERE id = ? AND organization_id IS NOT NULL")
        .bind(user.0).bind(if role == Role::Guest { "guest" } else { "member" }).bind(now).bind(workspace.0).execute(&mut **tx).await?;
    // Members (not guests) land in the default channels, read up to now.
    if role != Role::Guest {
        sqlx::query(
            "INSERT OR IGNORE INTO channel_members (channel_id, user_id, joined_at, last_read_id)
             SELECT c.id, ?, ?, COALESCE((SELECT MAX(id) FROM messages WHERE channel_id = c.id), 0)
             FROM channels c WHERE c.workspace_id = ? AND c.is_default = 1 AND c.kind = ?",
        )
        .bind(user.0)
        .bind(now)
        .bind(workspace.0)
        .bind(ChannelKind::Public.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

impl Store {
    /// Whether an invitation exists, has not expired and has uses left.
    pub async fn invite_usable(&self, code_hash: &[u8], now: Timestamp) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invites WHERE code_hash = ? AND uses < max_uses AND expires_at > ?")
            .bind(code_hash)
            .bind(now)
            .fetch_one(&self.pool)
            .await?;
        Ok(n > 0)
    }
}
