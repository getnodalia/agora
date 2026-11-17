#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use agora_protocol::{CreateTaskRequest, TaskPriority};
use common::*;
use reqwest::StatusCode;

#[tokio::test]
async fn plaintext_task_routes_are_unavailable_and_existing_data_survives() {
    let (dir, db) = temp_db("disabled-tasks");
    let server = Server::start(&db).await;
    let (owner, user) = register(&server, "owner").await;
    let workspace = owner.create_workspace("Research").await.unwrap();
    let token = owner.token().unwrap().to_owned();
    let task = CreateTaskRequest {
        title: "Existing task".into(),
        priority: TaskPriority::Normal,
        assignee_id: None,
        due_at: None,
        acknowledge_server_readable: true,
    };
    // Seed an existing row as a previous installation would have stored it.
    let stored = server
        .state
        .store
        .create_task(workspace.id, user.id, &task, 1)
        .await
        .unwrap();
    let url = format!("{}/api/v1/workspaces/{}/tasks", server.url, workspace.id);
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(&token)
            .json(&task)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.put(format!("{url}/{}", stored.id))
            .bearer_auth(&token)
            .json(&task)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.state.store.tasks(workspace.id, 0).await.unwrap(),
        vec![stored.clone()]
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
    assert_eq!(
        server.state.store.tasks(workspace.id, 0).await.unwrap(),
        vec![stored]
    );
    assert_eq!(
        http.get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
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
