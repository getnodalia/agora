use crate::{
    auth::{now_ms, Auth},
    error::{AppError, AppResult},
    state::AppState,
};
use agora_core::{validate, OrganizationId, Role, UserId};
use agora_protocol::{
    CreateOrganizationRequest, CreateWorkspaceRequest, Member, Organization,
    SetOrganizationMemberRequest, Workspace,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

const ORGANIZATIONS_MAX: usize = 50;
async fn role(state: &AppState, org: OrganizationId, user: UserId) -> AppResult<Role> {
    state
        .store
        .organization_role(org, user)
        .await?
        .ok_or_else(AppError::not_found)
}
async fn owner(state: &AppState, org: OrganizationId, user: UserId) -> AppResult<()> {
    if role(state, org, user).await? != Role::Owner {
        return Err(AppError::forbidden(
            "only the organization owner can do that",
        ));
    }
    Ok(())
}
pub async fn list(State(state): State<AppState>, auth: Auth) -> AppResult<Json<Vec<Organization>>> {
    Ok(Json(state.store.organizations_of(auth.user.id).await?))
}
pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<CreateOrganizationRequest>,
) -> AppResult<(StatusCode, Json<Organization>)> {
    super::check_write(&state, auth.user.id)?;
    let name = validate::workspace_name(&req.name)?;
    if state
        .store
        .organizations_of(auth.user.id)
        .await?
        .iter()
        .filter(|o| o.role == Role::Owner)
        .count()
        >= ORGANIZATIONS_MAX
    {
        return Err(AppError::forbidden("too many organizations"));
    }
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .store
                .create_organization(&name, auth.user.id, now_ms())
                .await?,
        ),
    ))
}
pub async fn members(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Member>>> {
    let org = OrganizationId(id);
    owner(&state, org, auth.user.id).await?;
    Ok(Json(state.store.organization_members(org).await?))
}
pub async fn set_member(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, user)): Path<(i64, i64)>,
    Json(req): Json<SetOrganizationMemberRequest>,
) -> AppResult<StatusCode> {
    let org = OrganizationId(id);
    owner(&state, org, auth.user.id).await?;
    super::check_write(&state, auth.user.id)?;
    if req.role == Role::Owner || auth.user.id == UserId(user) {
        return Err(AppError::invalid(
            "the owner role cannot be assigned or changed",
        ));
    }
    if !state
        .store
        .set_organization_member(org, auth.user.id, UserId(user), req.role, now_ms())
        .await?
    {
        return Err(AppError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
pub async fn workspaces(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<Workspace>>> {
    let org = OrganizationId(id);
    role(&state, org, auth.user.id).await?;
    Ok(Json(
        state
            .store
            .organization_workspaces(org, auth.user.id)
            .await?,
    ))
}
pub async fn create_workspace(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> AppResult<(StatusCode, Json<Workspace>)> {
    let org = OrganizationId(id);
    owner(&state, org, auth.user.id).await?;
    super::check_write(&state, auth.user.id)?;
    let name = validate::workspace_name(&req.name)?;
    if state
        .store
        .workspaces_of(auth.user.id)
        .await?
        .iter()
        .filter(|w| w.role == Role::Owner)
        .count()
        >= 50
    {
        return Err(AppError::forbidden("too many workspaces"));
    }
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .store
                .create_organization_workspace(org, auth.user.id, &name, now_ms())
                .await?,
        ),
    ))
}
