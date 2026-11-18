//! Delivery and read receipts: who has received and read a conversation, and who may know.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_core::MessageId;
use agora_protocol::{CreateChannelRequest, CreateInviteRequest, ErrorCode, Receipt, ServerEvent};
use common::*;

async fn team(server: &Server) -> (Person, Person, Person, agora_protocol::Workspace) {
    let ana = Person::register(server, "ana").await;
    let ws = ana.create_workspace("Laboratorio").await.unwrap();
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

fn of(receipts: &[Receipt], user: agora_core::UserId) -> Receipt {
    *receipts.iter().find(|r| r.user_id == user).unwrap()
}

#[tokio::test]
async fn delivery_and_reading_move_forward_and_reach_the_author() {
    let (_dir, db) = temp_db("receipts-flow");
    let server = Server::start(&db).await;
    let (ana, luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let mut live = Live::open(&ana);
    live.ready().await;

    let first = ana.say(general.id, "¿Comemos a las dos?").await;
    let second = ana.say(general.id, "Reservo mesa").await;

    luis.mark_delivered(general.id, second.id).await.unwrap();
    let moved = live
        .server_event(|e| match e {
            ServerEvent::Receipt {
                channel_id,
                receipt,
            } if *channel_id == general.id && receipt.user_id == luis.user.id => Some(*receipt),
            _ => None,
        })
        .await;
    assert_eq!(moved.delivered_id, second.id);
    assert!(moved.read_id < first.id);

    luis.mark_read(general.id, first.id).await.unwrap();
    let r = of(&ana.receipts(general.id).await.unwrap(), luis.user.id);
    assert_eq!((r.delivered_id, r.read_id), (second.id, first.id));

    // Markers never move back, and never past the newest message.
    luis.mark_delivered(general.id, first.id).await.unwrap();
    luis.mark_read(general.id, MessageId(second.id.0 + 1000))
        .await
        .unwrap();
    let r = of(&ana.receipts(general.id).await.unwrap(), luis.user.id);
    assert_eq!((r.delivered_id, r.read_id), (second.id, second.id));
}

#[tokio::test]
async fn reading_counts_as_delivered() {
    let (_dir, db) = temp_db("receipts-read");
    let server = Server::start(&db).await;
    let (ana, luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let sent = ana.say(general.id, "Hola").await;
    luis.mark_read(general.id, sent.id).await.unwrap();
    let r = of(&ana.receipts(general.id).await.unwrap(), luis.user.id);
    assert_eq!((r.delivered_id, r.read_id), (sent.id, sent.id));
}

#[tokio::test]
async fn people_outside_a_private_channel_learn_nothing() {
    let (_dir, db) = temp_db("receipts-private");
    let server = Server::start(&db).await;
    let (ana, luis, eva, ws) = team(&server).await;
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
    ana.add_channel_member(secret.id, luis.user.id)
        .await
        .unwrap();
    let sent = ana.say(secret.id, "Presupuesto").await;
    let mut eva_live = Live::open(&eva);
    eva_live.ready().await;

    luis.mark_delivered(secret.id, sent.id).await.unwrap();
    for err in [
        eva.receipts(secret.id).await.err(),
        eva.mark_delivered(secret.id, sent.id).await.err(),
    ] {
        assert_eq!(err.and_then(|e| e.code()), Some(ErrorCode::NotFound));
    }
    eva_live
        .assert_none(std::time::Duration::from_millis(500), |e| {
            matches!(e, ServerEvent::Receipt { .. })
        })
        .await;
}
