#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use agora_core::Role;
use agora_protocol::{
    CreateInviteRequest, CreateOrganizationRequest, CreateWorkspaceRequest, Organization,
    SetOrganizationMemberRequest, Workspace,
};
use common::*;
use reqwest::StatusCode;

#[tokio::test]
async fn company_roles_and_workspace_access_are_separate() {
    let server = Server::start("sqlite::memory:").await;
    let (owner, owner_user) = register(&server, "owner").await;
    let (member, member_user) = register(&server, "member").await;
    let (outsider, outsider_user) = register(&server, "outsider").await;
    let old = owner.create_workspace("Existing workspace").await.unwrap();
    let http = reqwest::Client::new();
    let orgs = format!("{}/api/v1/organizations", server.url);
    let org: Organization = http
        .post(&orgs)
        .bearer_auth(owner.token().unwrap())
        .json(&CreateOrganizationRequest {
            name: "Nodalia demo".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let base = format!("{orgs}/{}", org.id);
    let ws_url = format!("{base}/workspaces");
    let body = CreateWorkspaceRequest {
        name: "Engineering".into(),
    };
    assert_eq!(
        http.post(&ws_url)
            .bearer_auth(outsider.token().unwrap())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let first: Workspace = http
        .post(&ws_url)
        .bearer_auth(owner.token().unwrap())
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Workspace = http
        .post(&ws_url)
        .bearer_auth(owner.token().unwrap())
        .json(&CreateWorkspaceRequest {
            name: "Research".into(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(owner.organizations().await.unwrap(), vec![org.clone()]);
    let all = owner.organization_workspaces(org.id).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(!all.iter().any(|w| w.id == old.id));
    let member_url = format!("{base}/members/{}", member_user.id);
    assert_eq!(
        http.get(format!("{base}/members"))
            .bearer_auth(outsider.token().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.put(&member_url)
            .bearer_auth(outsider.token().unwrap())
            .json(&SetOrganizationMemberRequest { role: Role::Admin })
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        http.put(&member_url)
            .bearer_auth(owner.token().unwrap())
            .json(&SetOrganizationMemberRequest { role: Role::Admin })
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let all: Vec<Workspace> = http
        .get(&ws_url)
        .bearer_auth(member.token().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(all.is_empty());
    assert_eq!(
        http.post(&ws_url)
            .bearer_auth(member.token().unwrap())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.put(format!("{base}/members/{}", owner_user.id))
            .bearer_auth(owner.token().unwrap())
            .json(&SetOrganizationMemberRequest { role: Role::Member })
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.put(&member_url)
            .bearer_auth(owner.token().unwrap())
            .json(&SetOrganizationMemberRequest { role: Role::Owner })
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let invite = owner
        .create_invite(
            first.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: None,
            },
        )
        .await
        .unwrap();
    member.join_workspace(&invite.code).await.unwrap();
    let all: Vec<Workspace> = http
        .get(&ws_url)
        .bearer_auth(member.token().unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, first.id);
    assert!(member.channels(second.id).await.is_err());
    assert_eq!(
        server
            .state
            .store
            .organization_role(org.id, member_user.id)
            .await
            .unwrap(),
        Some(Role::Admin)
    );
    agora_storage::sqlx::query(
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = ? AND user_id = ?",
    )
    .bind(first.id.0)
    .bind(member_user.id.0)
    .execute(server.state.store.pool())
    .await
    .unwrap();
    let outsider_invite = owner
        .create_invite(
            second.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: Some(Role::Guest),
            },
        )
        .await
        .unwrap();
    outsider
        .join_workspace(&outsider_invite.code)
        .await
        .unwrap();
    let registration_invite = owner
        .create_invite(
            first.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: None,
            },
        )
        .await
        .unwrap();
    let (_, newcomer) =
        register_with_invite(&server, "newcomer", Some(&registration_invite.code)).await;
    assert_eq!(
        server
            .state
            .store
            .organization_role(org.id, newcomer.id)
            .await
            .unwrap(),
        Some(Role::Member)
    );
    assert_eq!(
        server
            .state
            .store
            .organization_role(org.id, outsider_user.id)
            .await
            .unwrap(),
        Some(Role::Guest)
    );
    assert!(outsider.channels(first.id).await.is_err());
    assert!(member
        .create_invite(
            first.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: None
            }
        )
        .await
        .is_err());
    server.stop().await;
}

#[tokio::test]
async fn legacy_workspaces_survive_organization_migration() {
    let (dir, db) = temp_db("org-migration");
    let migrations = dir.join("old-migrations");
    std::fs::create_dir_all(&migrations).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/agora-storage/migrations");
    for name in [
        "0001_initial.sql",
        "0002_end_to_end.sql",
        "0003_server_initialization.sql",
    ] {
        std::fs::copy(source.join(name), migrations.join(name)).unwrap();
    }
    let options = db
        .parse::<agora_storage::sqlx::sqlite::SqliteConnectOptions>()
        .unwrap()
        .create_if_missing(true);
    let pool = agora_storage::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    agora_storage::sqlx::migrate::Migrator::new(migrations.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    agora_storage::sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES (1, 'ana', 'Ana', 'hash', 1)").execute(&pool).await.unwrap();
    agora_storage::sqlx::query(
        "INSERT INTO workspaces (id, name, created_by, created_at) VALUES (1, 'Existing', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    agora_storage::sqlx::query("INSERT INTO workspace_members (workspace_id, user_id, role, joined_at) VALUES (1, 1, 'owner', 1)").execute(&pool).await.unwrap();
    agora_storage::sqlx::query("INSERT INTO sessions (id, token_hash, user_id, created_at, expires_at, last_seen_at) VALUES (1, X'0102', 1, 1, 9999999999999, 1)").execute(&pool).await.unwrap();
    agora_storage::sqlx::query("INSERT INTO channels (id, workspace_id, name, topic, kind, is_default, created_by, created_at) VALUES (1, 1, 'general', '', 'public', 1, 1, 1)").execute(&pool).await.unwrap();
    agora_storage::sqlx::query("INSERT INTO messages (id, channel_id, author_id, body, client_nonce, created_at) VALUES (1, 1, 1, 'existing message', 'nonce', 1)").execute(&pool).await.unwrap();
    pool.close().await;
    let store = agora_storage::Store::open(&db).await.unwrap();
    assert_eq!(
        store
            .workspace(agora_core::WorkspaceId(1), agora_core::UserId(1))
            .await
            .unwrap()
            .unwrap()
            .name,
        "Existing"
    );
    assert!(store
        .organizations_of(agora_core::UserId(1))
        .await
        .unwrap()
        .is_empty());
    let session_count: i64 = agora_storage::sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE id = 1 AND token_hash = X'0102'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let message: String =
        agora_storage::sqlx::query_scalar("SELECT body FROM messages WHERE id = 1")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(session_count, 1);
    assert_eq!(message, "existing message");
    store.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}
