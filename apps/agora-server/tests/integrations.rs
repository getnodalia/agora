//! Integrations: another app posts signed notices to a channel. Only admins add them, only
//! people who can read the channel see them, and an unsigned or replayed request changes nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_protocol::{
    CreateChannelRequest, CreateIntegrationRequest, CreateInviteRequest, ErrorCode, IntegrationKind,
};
use common::*;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use sha2::Sha256;

fn sign(body: &str, key: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

async fn post(url: &str, header: &str, signature: &str, body: &str) -> StatusCode {
    reqwest::Client::new()
        .post(url)
        .header(header, signature)
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap()
        .status()
}

async fn team(server: &Server) -> (Person, Person, Person, agora_protocol::Workspace) {
    let ana = Person::register(server, "ana").await;
    let ws = ana.create_workspace("Estudio").await.unwrap();
    let invite = ana
        .create_invite(
            ws.id,
            &CreateInviteRequest {
                max_uses: Some(5),
                expires_in_hours: None,
                role: None,
            },
        )
        .await
        .unwrap();
    let luis = Person::register(server, "luis").await;
    luis.join_workspace(&invite.code).await.unwrap();
    let eva = Person::register(server, "eva").await;
    eva.join_workspace(&invite.code).await.unwrap();
    (ana, luis, eva, ws)
}

#[tokio::test]
async fn a_signed_hito_notice_reaches_the_channel_once() {
    let (_dir, db) = temp_db("integrations-flow");
    let server = Server::start(&db).await;
    let (ana, luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();

    let created = ana
        .create_integration(
            general.id,
            &CreateIntegrationRequest {
                kind: IntegrationKind::Hito,
                name: "Lanzamiento".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(created.secret.len(), 64);
    let url = ana.hook_url(&created);
    assert!(url.ends_with(&format!("/api/v1/hooks/{}", created.integration.id)));

    let body = r#"{"event":"task.completed","workspace_id":1,"project":{"id":3,"name":"Web nueva"},"activity":{},"text":"Marta completó «Portada»","server_url":"https://hito.example/p/3"}"#;
    let sig = sign(body, &created.secret);
    assert_eq!(
        post(&url, "x-hito-signature", &sig, body).await,
        StatusCode::ACCEPTED
    );
    // The same request again (a retry or a replay) is accepted and adds nothing.
    assert_eq!(
        post(&url, "x-hito-signature", &sig, body).await,
        StatusCode::ACCEPTED
    );

    let notices = luis.notices(general.id).await.unwrap();
    assert_eq!(notices.len(), 1);
    let notice = &notices[0];
    assert_eq!(notice.text, "Marta completó «Portada»");
    assert_eq!(notice.title.as_deref(), Some("Web nueva"));
    assert_eq!(notice.url.as_deref(), Some("https://hito.example/p/3"));
    assert_eq!(notice.integration_name, "Lanzamiento");
    assert_eq!(notice.kind, IntegrationKind::Hito);

    // A plain webhook, signed in Ágora's own header; a link that is not a web address is dropped.
    let body = r#"{"text":"Build 41 passed","title":"CI","url":"javascript:alert(1)"}"#;
    assert_eq!(
        post(
            &url,
            "x-agora-signature",
            &sign(body, &created.secret),
            body
        )
        .await,
        StatusCode::ACCEPTED
    );
    let notices = luis.notices(general.id).await.unwrap();
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[1].url, None);
    server.stop().await;
}

#[tokio::test]
async fn unsigned_and_wrongly_signed_requests_are_refused() {
    let (_dir, db) = temp_db("integrations-signature");
    let server = Server::start(&db).await;
    let (ana, _luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let created = ana
        .create_integration(
            general.id,
            &CreateIntegrationRequest {
                kind: IntegrationKind::Webhook,
                name: "CI".into(),
            },
        )
        .await
        .unwrap();
    let url = ana.hook_url(&created);
    let body = r#"{"text":"hola"}"#;
    assert_eq!(
        post(&url, "x-agora-signature", "", body).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&url, "x-agora-signature", &sign(body, "otra clave"), body).await,
        StatusCode::UNAUTHORIZED
    );
    // A signature for another body does not carry over.
    assert_eq!(
        post(
            &url,
            "x-agora-signature",
            &sign(r#"{"text":"adiós"}"#, &created.secret),
            body
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    // An integration that does not exist answers the same as a wrong signature.
    let missing = url.replace(
        &format!("/hooks/{}", created.integration.id),
        "/hooks/999999",
    );
    assert_eq!(
        post(
            &missing,
            "x-agora-signature",
            &sign(body, &created.secret),
            body
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert!(ana.notices(general.id).await.unwrap().is_empty());

    // Once removed, the address stops working.
    ana.delete_integration(created.integration.id)
        .await
        .unwrap();
    assert_eq!(
        post(
            &url,
            "x-agora-signature",
            &sign(body, &created.secret),
            body
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    server.stop().await;
}

#[tokio::test]
async fn only_admins_manage_and_only_readers_see() {
    let (_dir, db) = temp_db("integrations-access");
    let server = Server::start(&db).await;
    let (ana, luis, eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let request = CreateIntegrationRequest {
        kind: IntegrationKind::Webhook,
        name: "CI".into(),
    };

    // A member is not an admin.
    let refused = luis
        .create_integration(general.id, &request)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::Forbidden));

    let secret = ana
        .create_channel(
            ws.id,
            &CreateChannelRequest {
                name: "dirección".into(),
                topic: String::new(),
                private: true,
            },
        )
        .await
        .unwrap();
    let created = ana.create_integration(secret.id, &request).await.unwrap();

    // Someone outside the private channel sees nothing, not even that it exists.
    for result in [
        eva.integrations(secret.id).await.map(|_| ()),
        eva.notices(secret.id).await.map(|_| ()),
        eva.delete_integration(created.integration.id).await,
        eva.create_integration(secret.id, &request)
            .await
            .map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().code(), Some(ErrorCode::NotFound));
    }

    // A member who can read it sees the integration but cannot remove it.
    ana.add_channel_member(secret.id, luis.user.id)
        .await
        .unwrap();
    assert_eq!(luis.integrations(secret.id).await.unwrap().len(), 1);
    assert_eq!(
        luis.delete_integration(created.integration.id)
            .await
            .unwrap_err()
            .code(),
        Some(ErrorCode::Forbidden)
    );

    // Names are checked.
    let empty = ana
        .create_integration(
            general.id,
            &CreateIntegrationRequest {
                kind: IntegrationKind::Webhook,
                name: "  ".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(empty.code(), Some(ErrorCode::Invalid));
    server.stop().await;
}

#[tokio::test]
async fn an_admin_can_send_a_test_notice_and_nobody_else_can() {
    let (_dir, db) = temp_db("integrations-test-notice");
    let server = Server::start(&db).await;
    let (ana, luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let created = ana
        .create_integration(
            general.id,
            &CreateIntegrationRequest {
                kind: IntegrationKind::Hito,
                name: "Lanzamiento".into(),
            },
        )
        .await
        .unwrap();
    let id = created.integration.id;
    let stranger = Person::register(&server, "marta").await;

    assert_eq!(
        luis.test_integration(id).await.unwrap_err().code(),
        Some(ErrorCode::Forbidden)
    );
    assert_eq!(
        stranger.test_integration(id).await.unwrap_err().code(),
        Some(ErrorCode::NotFound)
    );
    assert!(luis.notices(general.id).await.unwrap().is_empty());

    let mut live = Live::open(&luis);
    live.ready().await;
    ana.test_integration(id).await.unwrap();
    let seen = live
        .server_event(|e| match e {
            agora_protocol::ServerEvent::IntegrationNotice { notice } => Some(notice.clone()),
            _ => None,
        })
        .await;
    assert_eq!(seen.integration_id, id);
    assert_eq!(seen.title.as_deref(), Some("Test notice"));
    // Testing twice shows two notices: each test is its own.
    ana.test_integration(id).await.unwrap();
    assert_eq!(luis.notices(general.id).await.unwrap().len(), 2);
    server.stop().await;
}
