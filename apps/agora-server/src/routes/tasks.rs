//! Workspace tasks are explicitly server-readable; they do not access MLS messages.
use crate::{
    auth::{now_ms, Auth},
    error::{AppError, AppResult},
    state::AppState,
};
use agora_core::{Role, UserId, WorkspaceId};
use agora_protocol::{CreateTaskRequest, Task, UpdateTaskRequest};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

async fn access(state: &AppState, ws: WorkspaceId, user: UserId) -> AppResult<()> {
    let role = super::workspace_role(state, ws, user).await?;
    if role == Role::Guest {
        return Err(AppError::not_found());
    }
    Ok(())
}
async fn validate(
    state: &AppState,
    ws: WorkspaceId,
    title: &str,
    assignee: Option<UserId>,
    due: Option<i64>,
) -> AppResult<()> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > 200 || t.chars().any(char::is_control) {
        return Err(AppError::invalid(
            "title must be 1–200 characters without control characters",
        ));
    }
    if due.is_some_and(|d| d < 0) {
        return Err(AppError::invalid("due_at must be non-negative"));
    }
    if let Some(id) = assignee {
        if !matches!(
            state.store.role(ws, id).await?,
            Some(Role::Owner | Role::Admin | Role::Member)
        ) {
            return Err(AppError::invalid(
                "assignee must be a non-guest workspace member",
            ));
        }
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct Pagination {
    #[serde(default)]
    after: i64,
}
pub async fn list(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(page): Query<Pagination>,
) -> AppResult<Json<Vec<Task>>> {
    let ws = WorkspaceId(id);
    access(&state, ws, auth.user.id).await?;
    if page.after < 0 {
        return Err(AppError::invalid("after must be non-negative"));
    }
    Ok(Json(state.store.tasks(ws, page.after).await?))
}
pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateTaskRequest>,
) -> AppResult<(StatusCode, Json<Task>)> {
    let ws = WorkspaceId(id);
    access(&state, ws, auth.user.id).await?;
    super::check_write(&state, auth.user.id)?;
    if !req.acknowledge_server_readable {
        return Err(AppError::invalid(
            "task data is visible to the server and workspace members",
        ));
    }
    validate(&state, ws, &req.title, req.assignee_id, req.due_at).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .store
                .create_task(ws, auth.user.id, &req, now_ms())
                .await?,
        ),
    ))
}
pub async fn update(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, task_id)): Path<(i64, i64)>,
    Json(req): Json<UpdateTaskRequest>,
) -> AppResult<Json<Task>> {
    let ws = WorkspaceId(id);
    access(&state, ws, auth.user.id).await?;
    super::check_write(&state, auth.user.id)?;
    validate(&state, ws, &req.title, req.assignee_id, req.due_at).await?;
    if let Some(task) = state.store.update_task(ws, task_id, &req, now_ms()).await? {
        return Ok(Json(task));
    }
    if state.store.task_exists(ws, task_id).await? {
        return Err(AppError::conflict("task changed; reload before editing"));
    }
    Err(AppError::not_found())
}
