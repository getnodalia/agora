//! Deleting a workspace: only its owner, only with its name typed, and everything in it goes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_protocol::{
    CreateIntegrationRequest, CreateInviteRequest, ErrorCode, IntegrationKind, ServerEvent,
};
use common::*;

fn code_of(error: agora_client::ClientError) -> ErrorCode {
    match error {
        agora_client::ClientError::Api { error, .. } => error.code,
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[tokio::test]
async fn only_the_owner_can_delete_and_it_takes_everything_with_it() {
    let (dir, db) = temp_db("workspace-delete");
    let server = Server::start(&db).await;
    let ana = Person::register(&server, "ana").await;
    let ws = ana.create_workspace("Estudio").await.unwrap();
    let other = ana.create_workspace("Otro").await.unwrap();
    let general = ana.channels(ws.id).await.unwrap()[0].id;
    ana.say(general, "hola").await;
    ana.create_integration(
        general,
        &CreateIntegrationRequest {
            kind: IntegrationKind::Webhook,
            name: "ci".into(),
        },
    )
    .await
    .unwrap();
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
    let luis = Person::register(&server, "luis").await;
    server
        .state
        .store
        .add_member(ws.id, luis.user.id, agora_core::Role::Admin, 1)
        .await
        .unwrap();
    let eva = Person::register(&server, "eva").await;

    // Not a member: the workspace does not exist from here. An admin is refused.
    assert_eq!(
        code_of(eva.delete_workspace(ws.id, "Estudio").await.unwrap_err()),
        ErrorCode::NotFound
    );
    assert_eq!(
        code_of(luis.delete_workspace(ws.id, "Estudio").await.unwrap_err()),
        ErrorCode::Forbidden
    );
    // The owner must type the exact name.
    for wrong in ["", "estudio", "Otro", "Estudio 2"] {
        assert_eq!(
            code_of(ana.delete_workspace(ws.id, wrong).await.unwrap_err()),
            ErrorCode::Invalid,
            "{wrong:?}"
        );
    }
    assert_eq!(ana.workspaces().await.unwrap().len(), 2);

    let mut live = Live::open(&luis);
    live.ready().await;
    ana.delete_workspace(ws.id, "Estudio").await.unwrap();
    let removed = live
        .server_event(|e| match e {
            ServerEvent::WorkspaceRemoved { workspace_id } => Some(*workspace_id),
            _ => None,
        })
        .await;
    assert_eq!(removed, ws.id);

    // Gone for everyone, with its channels, messages and integrations; the other one stays.
    let left: Vec<_> = ana
        .workspaces()
        .await
        .unwrap()
        .iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(left, vec![other.id]);
    assert!(luis.workspaces().await.unwrap().is_empty());
    assert!(ana.client.history(general, None, None, None).await.is_err());
    assert!(server.state.store.channel(general).await.unwrap().is_none());
    assert!(!server
        .state
        .store
        .invite_usable(&agora_server::auth::token_hash(&invite.code), 1)
        .await
        .unwrap());
    assert_eq!(
        code_of(ana.delete_workspace(ws.id, "Estudio").await.unwrap_err()),
        ErrorCode::NotFound
    );
    server.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}
