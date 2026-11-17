#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use agora_core::Role;
use common::*;
use reqwest::StatusCode;

const ICON: &[u8] =
    include_bytes!("../../../assets/icons/hicolor/256x256/apps/dev.danielmiguelt.Agora.png");

#[tokio::test]
async fn icons_are_bounded_authorized_and_survive_restart() {
    let (dir, db) = temp_db("workspace-icon");
    let server = Server::start(&db).await;
    let (owner, _) = register(&server, "owner").await;
    let (member, member_user) = register(&server, "member").await;
    let (stranger, _) = register(&server, "stranger").await;
    let ws = owner.create_workspace("Workspace").await.unwrap();
    server
        .state
        .store
        .add_member(ws.id, member_user.id, Role::Member, 1)
        .await
        .unwrap();
    let url = format!("{}/api/v1/workspaces/{}/icon", server.url, ws.id);
    let http = reqwest::Client::new();
    let owner_token = owner.token().unwrap();
    let member_token = member.token().unwrap();
    let stranger_token = stranger.token().unwrap();
    assert_eq!(
        http.get(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.put(&url)
            .bearer_auth(stranger_token)
            .body(ICON)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.put(&url)
            .bearer_auth(member_token)
            .body(ICON)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.put(&url)
            .bearer_auth(owner_token)
            .body("invalid")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.put(&url)
            .bearer_auth(owner_token)
            .body(vec![0; 128 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        http.put(&url)
            .bearer_auth(owner_token)
            .body(ICON)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let mut too_wide = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut too_wide, 513, 1);
        encoder.set_color(png::ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![0; 513 * 4]).unwrap();
    }
    assert_eq!(
        http.put(&url)
            .bearer_auth(owner_token)
            .body(too_wide)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = http
        .get(&url)
        .bearer_auth(member_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[reqwest::header::CONTENT_TYPE],
        "image/png"
    );
    assert_eq!(response.bytes().await.unwrap().as_ref(), ICON);
    assert_eq!(
        http.get(&url)
            .bearer_auth(stranger_token)
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
    assert_eq!(
        http.get(&url)
            .bearer_auth(member_token)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        ICON
    );
    assert_eq!(
        http.delete(&url)
            .bearer_auth(member_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.delete(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        http.get(&url)
            .bearer_auth(owner_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
