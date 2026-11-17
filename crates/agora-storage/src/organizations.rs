//! Organizations and their workspace boundaries.
use crate::{rows, Result, Store, StoreError};
use agora_core::{OrganizationId, Role, UserId, WorkspaceId};
use agora_protocol::{Member, Organization, Workspace};
use sqlx::Row;

impl Store {
    pub async fn create_organization(
        &self,
        name: &str,
        owner: UserId,
        now: i64,
    ) -> Result<Organization> {
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO organizations (name, owner_id, created_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(owner.0)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO organization_members (organization_id, user_id, role, joined_at) VALUES (?, ?, 'owner', ?)")
            .bind(id).bind(owner.0).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Organization {
            id: OrganizationId(id),
            name: name.to_owned(),
            role: Role::Owner,
            created_at: now,
        })
    }
    pub async fn organizations_of(&self, user: UserId) -> Result<Vec<Organization>> {
        let all = sqlx::query("SELECT o.id, o.name, o.created_at, m.role FROM organizations o JOIN organization_members m ON m.organization_id = o.id WHERE m.user_id = ? ORDER BY o.id")
            .bind(user.0).fetch_all(&self.pool).await?;
        all.iter()
            .map(|r| {
                Ok(Organization {
                    id: OrganizationId(r.try_get("id")?),
                    name: r.try_get("name")?,
                    created_at: r.try_get("created_at")?,
                    role: rows::role(r.try_get("role")?)?,
                })
            })
            .collect()
    }
    pub async fn organization_role(
        &self,
        org: OrganizationId,
        user: UserId,
    ) -> Result<Option<Role>> {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?",
        )
        .bind(org.0)
        .bind(user.0)
        .fetch_optional(&self.pool)
        .await?;
        role.map(|s| rows::role(&s)).transpose()
    }
    pub async fn organization_members(&self, org: OrganizationId) -> Result<Vec<Member>> {
        let all = sqlx::query("SELECT u.id, u.username, u.display_name, m.role, m.joined_at FROM organization_members m JOIN users u ON u.id = m.user_id WHERE m.organization_id = ? ORDER BY u.id")
            .bind(org.0).fetch_all(&self.pool).await?;
        all.iter()
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
    pub async fn workspace_organization_owner(&self, ws: WorkspaceId) -> Result<Option<UserId>> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT o.owner_id FROM workspaces w JOIN organizations o ON o.id = w.organization_id WHERE w.id = ?")
            .bind(ws.0).fetch_optional(&self.pool).await?.map(UserId))
    }
    /// An owner can add an existing user or change a non-owner's role.
    pub async fn set_organization_member(
        &self,
        org: OrganizationId,
        owner: UserId,
        user: UserId,
        role: Role,
        now: i64,
    ) -> Result<bool> {
        if role == Role::Owner || user == owner {
            return Ok(false);
        }
        let changed = sqlx::query("INSERT INTO organization_members (organization_id, user_id, role, joined_at) SELECT ?, id, ?, ? FROM users WHERE id = ? AND EXISTS (SELECT 1 FROM organizations WHERE id = ? AND owner_id = ?) ON CONFLICT (organization_id, user_id) DO UPDATE SET role = excluded.role WHERE organization_members.role <> 'owner'")
            .bind(org.0).bind(role.as_str()).bind(now).bind(user.0).bind(org.0).bind(owner.0).execute(&self.pool).await?;
        Ok(changed.rows_affected() == 1)
    }
    /// Only return workspaces the caller can already access. Organization membership alone is not access.
    pub async fn organization_workspaces(
        &self,
        org: OrganizationId,
        user: UserId,
    ) -> Result<Vec<Workspace>> {
        let rows = sqlx::query("SELECT w.id, w.name, w.created_at, m.role FROM workspaces w JOIN workspace_members m ON m.workspace_id = w.id WHERE w.organization_id = ? AND m.user_id = ? ORDER BY w.id")
            .bind(org.0).bind(user.0).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|r| {
                Ok(Workspace {
                    id: WorkspaceId(r.try_get("id")?),
                    name: r.try_get("name")?,
                    created_at: r.try_get("created_at")?,
                    role: rows::role(r.try_get("role")?)?,
                })
            })
            .collect()
    }
    /// Create a workspace, its owner and its general channel atomically.
    pub async fn create_organization_workspace(
        &self,
        org: OrganizationId,
        owner: UserId,
        name: &str,
        now: i64,
    ) -> Result<Workspace> {
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar("INSERT INTO workspaces (name, created_by, created_at, organization_id) SELECT ?, ?, ?, id FROM organizations WHERE id = ? AND owner_id = ? RETURNING id")
            .bind(name).bind(owner.0).bind(now).bind(org.0).bind(owner.0).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        sqlx::query("INSERT INTO workspace_members (workspace_id, user_id, role, joined_at) VALUES (?, ?, 'owner', ?)")
            .bind(id).bind(owner.0).bind(now).execute(&mut *tx).await?;
        let channel: i64 = sqlx::query_scalar("INSERT INTO channels (workspace_id, name, topic, kind, is_default, created_by, created_at) VALUES (?, 'general', '', 'public', 1, ?, ?) RETURNING id")
            .bind(id).bind(owner.0).bind(now).fetch_one(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO channel_members (channel_id, user_id, joined_at) VALUES (?, ?, ?)",
        )
        .bind(channel)
        .bind(owner.0)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Workspace {
            id: WorkspaceId(id),
            name: name.to_owned(),
            role: Role::Owner,
            created_at: now,
        })
    }
}
