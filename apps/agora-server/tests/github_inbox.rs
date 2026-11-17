#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use common::*;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use sha2::Sha256;

const SECRET: &str = "a-long-unique-test-secret-at-least-32-bytes";
const DELIVERY: &str = "4c48d5d0-75d2-40c1-a26d-38c216f8e0ad";
fn signature(body: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

#[tokio::test]
async fn signed_events_are_scoped_deduplicated_and_persisted() {
    let (dir, db) = temp_db("github-inbox");
    let config = agora_server::Config {
        database: db.clone(),
        github_webhook: Some(agora_server::config::GitHubWebhook {
            workspace: 1,
            repository: 123,
            secret: SECRET.into(),
        }),
        ..Default::default()
    };
    assert!(!format!("{config:?}").contains(SECRET));
    let server = Server::start_with(config.clone(), 0).await;
    let (owner, _) = register(&server, "owner").await;
    let (stranger, _) = register(&server, "stranger").await;
    let ws = owner.create_workspace("Project").await.unwrap();
    assert_eq!(ws.id.0, 1);
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/integrations/github/webhook", server.url);
    let body =
        r#"{"action":"opened","repository":{"id":123},"issue":{"number":42,"title":"not stored"}}"#;
    let send = |body: &str, sig: String, delivery: &str| {
        http.post(&url)
            .header("x-github-event", "issues")
            .header("x-github-delivery", delivery)
            .header("x-hub-signature-256", sig)
            .body(body.to_owned())
            .send()
    };
    assert_eq!(
        send(body, signature(body, "wrong"), DELIVERY)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let other_repo = r#"{"repository":{"id":456}}"#;
    assert_eq!(
        send(other_repo, signature(other_repo, SECRET), DELIVERY)
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(body, signature(body, SECRET), DELIVERY)
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(
            body,
            signature(body, SECRET),
            "cf48d5d0-75d2-40c1-a26d-38c216f8e0ad"
        )
        .await
        .unwrap()
        .status(),
        StatusCode::ACCEPTED
    );
    let list = format!(
        "{}/api/v1/workspaces/{}/integrations/github/events",
        server.url, ws.id
    );
    assert_eq!(
        http.get(&list)
            .bearer_auth(stranger.token().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let token = owner.token().unwrap().to_owned();
    let response = http
        .get(&list)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!response.contains("not stored"));
    let events: Vec<agora_protocol::GitHubEvent> = serde_json::from_str(&response).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "issues");
    assert_eq!(events[0].action.as_deref(), Some("opened"));
    assert_eq!(events[0].object_number, Some(42));
    let port = server.port();
    server.stop().await;
    let server = Server::start_with(config, port).await;
    let events: Vec<agora_protocol::GitHubEvent> = http
        .get(&list)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    server.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
