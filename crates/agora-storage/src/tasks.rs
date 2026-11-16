//! Server-readable workspace tasks. Access is checked by the HTTP layer.

use agora_core::{UserId, WorkspaceId};
use agora_protocol::{CreateTaskRequest, Task, TaskPriority, TaskStatus, UpdateTaskRequest};
use sqlx::{sqlite::SqliteRow, Row};

use crate::{Result, Store, StoreError};

fn status(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
    }
}
fn priority(p: TaskPriority) -> &'static str {
    match p {
        TaskPriority::Low => "low",
        TaskPriority::Normal => "normal",
        TaskPriority::High => "high",
    }
}
fn row(r: &SqliteRow) -> Result<Task> {
    let status = match r.try_get::<&str, _>("status")? {
        "todo" => TaskStatus::Todo,
        "in_progress" => TaskStatus::InProgress,
        "done" => TaskStatus::Done,
        _ => return Err(StoreError::Corrupt("invalid task status".into())),
    };
    let priority = match r.try_get::<&str, _>("priority")? {
        "low" => TaskPriority::Low,
        "normal" => TaskPriority::Normal,
        "high" => TaskPriority::High,
        _ => return Err(StoreError::Corrupt("invalid task priority".into())),
    };
    Ok(Task {
        id: r.try_get("id")?,
        workspace_id: WorkspaceId(r.try_get("workspace_id")?),
        title: r.try_get("title")?,
        status,
        priority,
        assignee_id: r.try_get::<Option<i64>, _>("assignee_id")?.map(UserId),
        due_at: r.try_get("due_at")?,
        created_by: UserId(r.try_get("created_by")?),
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        revision: r.try_get("revision")?,
    })
}

impl Store {
    pub async fn tasks(&self, ws: WorkspaceId, after: i64) -> Result<Vec<Task>> {
        let rows = sqlx::query(
            "SELECT * FROM tasks WHERE workspace_id = ? AND id > ? ORDER BY id LIMIT 100",
        )
        .bind(ws.0)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row).collect()
    }
    pub async fn create_task(
        &self,
        ws: WorkspaceId,
        by: UserId,
        req: &CreateTaskRequest,
        now: i64,
    ) -> Result<Task> {
        let r = sqlx::query("INSERT INTO tasks (workspace_id, title, status, priority, assignee_id, due_at, created_by, created_at, updated_at) VALUES (?, ?, 'todo', ?, ?, ?, ?, ?, ?) RETURNING *")
            .bind(ws.0).bind(req.title.trim()).bind(priority(req.priority)).bind(req.assignee_id.map(|id| id.0))
            .bind(req.due_at).bind(by.0).bind(now).bind(now).fetch_one(&self.pool).await?;
        row(&r)
    }
    pub async fn update_task(
        &self,
        ws: WorkspaceId,
        id: i64,
        req: &UpdateTaskRequest,
        now: i64,
    ) -> Result<Option<Task>> {
        let r = sqlx::query("UPDATE tasks SET title = ?, status = ?, priority = ?, assignee_id = ?, due_at = ?, updated_at = ?, revision = revision + 1 WHERE workspace_id = ? AND id = ? AND revision = ? RETURNING *")
            .bind(req.title.trim()).bind(status(req.status)).bind(priority(req.priority)).bind(req.assignee_id.map(|id| id.0))
            .bind(req.due_at).bind(now).bind(ws.0).bind(id).bind(req.revision).fetch_optional(&self.pool).await?;
        r.as_ref().map(row).transpose()
    }
    pub async fn task_exists(&self, ws: WorkspaceId, id: i64) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE workspace_id = ? AND id = ?)",
        )
        .bind(ws.0)
        .bind(id)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }
}
