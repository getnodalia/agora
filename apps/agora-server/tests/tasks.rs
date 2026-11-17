#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use agora_protocol::{
    CreateInviteRequest, CreateTaskRequest, Task, TaskPriority, TaskStatus, UpdateTaskRequest,
};
use common::*;
use reqwest::StatusCode;

#[tokio::test]
async fn tasks_are_workspace_scoped_persistent_and_conflict_safe() {
    let (dir, db) = temp_db("tasks");
    let server = Server::start(&db).await;
    let (ana, _) = register(&server, "ana").await;
    let (outsider, outsider_user) = register(&server, "outsider").await;
    let ws = ana.create_workspace("Research").await.unwrap();
    let invite = ana
        .create_invite(
            ws.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: None,
            },
        )
        .await
        .unwrap();
    let (member, member_user) = register(&server, "member").await;
    member.join_workspace(&invite.code).await.unwrap();
    let guest_invite = ana
        .create_invite(
            ws.id,
            &CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
                role: Some(agora_core::Role::Guest),
            },
        )
        .await
        .unwrap();
    let (guest, _) = register(&server, "guest").await;
    guest.join_workspace(&guest_invite.code).await.unwrap();
    let token = ana.token().unwrap().to_string();
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/workspaces/{}/tasks", server.url, ws.id);
    assert_eq!(
        http.get(&url)
            .bearer_auth(guest.token().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut req = CreateTaskRequest {
        title: "Review experiment".into(),
        priority: TaskPriority::High,
        assignee_id: Some(member_user.id),
        due_at: Some(1_800_000_000_000),
        acknowledge_server_readable: false,
    };
    assert_eq!(
        http.post(&url)
            .bearer_auth(&token)
            .json(&req)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    req.acknowledge_server_readable = true;
    assert_eq!(
        http.post(&url)
            .bearer_auth(outsider.token().unwrap())
            .json(&req)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    req.assignee_id = Some(outsider_user.id);
    assert_eq!(
        http.post(&url)
            .bearer_auth(&token)
            .json(&req)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    req.assignee_id = Some(member_user.id);
    assert_eq!(
        http.post(&url)
            .bearer_auth(guest.token().unwrap())
            .json(&req)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let res = http
        .post(&url)
        .bearer_auth(&token)
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: Task = res.json().await.unwrap();
    assert_eq!(task.assignee_id, Some(member_user.id));
    assert_eq!(member.tasks(ws.id, 0).await.unwrap(), vec![task.clone()]);
    let update = UpdateTaskRequest {
        title: task.title.clone(),
        status: TaskStatus::InProgress,
        priority: TaskPriority::High,
        assignee_id: task.assignee_id,
        due_at: task.due_at,
        revision: task.revision,
    };
    let task_url = format!("{url}/{}", task.id);
    let changed = member.update_task(ws.id, task.id, &update).await.unwrap();
    assert_eq!(changed.revision, 2);
    assert_eq!(
        http.put(&task_url)
            .bearer_auth(&token)
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        http.get(&url)
            .bearer_auth(outsider.token().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let port = server.port();
    server.stop().await;
    let server = Server::start_with(
        agora_server::Config {
            database: db,
            ..Default::default()
        },
        port,
    )
    .await;
    let tasks: Vec<Task> = http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tasks, vec![changed]);
    server.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn existing_database_migrates_without_losing_workspace_data() {
    let (dir, db) = temp_db("tasks-migration");
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
        .tasks(agora_core::WorkspaceId(1), 0)
        .await
        .unwrap()
        .is_empty());
    store.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}
