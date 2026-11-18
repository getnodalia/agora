//! Calls: who is in a channel's call, and their audio and video passing through the server
//! sealed, to the others in the call only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use agora_client::RealtimeEvent;
use agora_core::{ChannelId, UserId};
use agora_e2e::call::{Opener, Sealer, Stream};
use agora_protocol::media::{Down, Kind, Up};
use agora_protocol::{
    CallLeftReason, CallParticipant, ClientFrame, CreateChannelRequest, CreateInviteRequest,
    ServerEvent,
};
use common::*;

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

async fn call_of(live: &mut Live, channel: ChannelId) -> Vec<CallParticipant> {
    live.server_event(|e| match e {
        ServerEvent::CallChanged {
            channel_id,
            participants,
        } if *channel_id == channel => Some(participants.clone()),
        _ => None,
    })
    .await
}

async fn media(live: &mut Live) -> Vec<u8> {
    live.wait(|e| match e {
        RealtimeEvent::Media(bytes) => Some(bytes.clone()),
        _ => None,
    })
    .await
}

fn users(people: &[CallParticipant]) -> Vec<UserId> {
    let mut ids: Vec<UserId> = people.iter().map(|p| p.user_id).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_call_carries_sealed_media_to_the_others_in_it_only() {
    let (_dir, db) = temp_db("calls-flow");
    let server = Server::start(&db).await;
    let (ana, luis, eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    // The conversation's group exists once someone writes; the others catch up.
    ana.say(general.id, "¿Llamada?").await;
    luis.history(general.id).await;
    eva.history(general.id).await;

    let mut ana_live = Live::open(&ana);
    let mut luis_live = Live::open(&luis);
    let mut eva_live = Live::open(&eva);
    for live in [&mut ana_live, &mut luis_live, &mut eva_live] {
        live.ready().await;
    }

    ana_live.conn.send(ClientFrame::CallJoin {
        channel_id: general.id,
        audio: true,
        video: false,
    });
    // Everyone who can read the channel sees the call start.
    let people = call_of(&mut eva_live, general.id).await;
    assert_eq!(users(&people), vec![ana.user.id]);
    assert!(people[0].audio && !people[0].video);
    call_of(&mut ana_live, general.id).await;

    luis_live.conn.send(ClientFrame::CallJoin {
        channel_id: general.id,
        audio: true,
        video: true,
    });
    let people = call_of(&mut ana_live, general.id).await;
    assert_eq!(users(&people), vec![ana.user.id, luis.user.id]);

    // Ana seals a frame with the conversation's call key; Luis opens it with his.
    let mut sealer = Sealer::new(
        general.id,
        ana.user.id,
        ana.e2e.call_key(general.id).await.unwrap(),
    );
    let sealed = sealer.seal(Stream::Audio, b"hola, equipo").unwrap();
    ana_live.conn.send_media(
        Up {
            kind: Kind::Audio,
            channel: general.id,
            payload: &sealed,
        }
        .encode(),
    );
    let bytes = media(&mut luis_live).await;
    let down = Down::parse(&bytes).unwrap();
    assert_eq!(down.sender, ana.user.id);
    assert_eq!(down.kind, Kind::Audio);
    assert_eq!(down.payload, &sealed[..]);
    assert!(!bytes.windows(4).any(|w| w == b"hola"));
    let mut opener = Opener::new(general.id);
    opener.add_key(luis.e2e.call_key(general.id).await.unwrap());
    assert_eq!(
        opener
            .open(down.sender, Stream::Audio, down.payload)
            .unwrap(),
        b"hola, equipo"
    );

    // Eva can read the channel but is not in the call: nothing reaches her, and what she sends
    // goes nowhere.
    eva_live.conn.send_media(
        Up {
            kind: Kind::Video,
            channel: general.id,
            payload: b"intrusa",
        }
        .encode(),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, eva_live.rx.recv()).await {
        assert!(!matches!(event, RealtimeEvent::Media(_)));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, ana_live.rx.recv()).await {
        assert!(!matches!(event, RealtimeEvent::Media(_)));
    }

    // Someone connecting now learns the call is on.
    let mut eva_again = Live::open(&eva);
    eva_again.ready().await;
    assert_eq!(users(&call_of(&mut eva_again, general.id).await).len(), 2);

    // Muting is announced; leaving by closing the connection ends Luis's part.
    luis_live.conn.send(ClientFrame::CallUpdate {
        channel_id: general.id,
        audio: false,
        video: true,
    });
    let people = call_of(&mut ana_live, general.id).await;
    assert!(people.iter().any(|p| p.user_id == luis.user.id && !p.audio));
    drop(luis_live);
    let people = call_of(&mut ana_live, general.id).await;
    assert_eq!(users(&people), vec![ana.user.id]);

    ana_live.conn.send(ClientFrame::CallLeave {
        channel_id: general.id,
    });
    // Eva's connection saw every change; the last says the call ended.
    while !call_of(&mut eva_live, general.id).await.is_empty() {}
    server.stop().await;
}

#[tokio::test]
async fn only_members_join_and_outsiders_never_hear_of_a_call() {
    let (_dir, db) = temp_db("calls-access");
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

    let mut ana_live = Live::open(&ana);
    let mut eva_live = Live::open(&eva);
    ana_live.ready().await;
    eva_live.ready().await;

    // Eva is not in the private channel: refused, as if it did not exist.
    eva_live.conn.send(ClientFrame::CallJoin {
        channel_id: secret.id,
        audio: true,
        video: true,
    });
    let reason = eva_live
        .server_event(|e| match e {
            ServerEvent::CallLeft { channel_id, reason } if *channel_id == secret.id => {
                Some(*reason)
            }
            _ => None,
        })
        .await;
    assert_eq!(reason, CallLeftReason::NoAccess);

    // A call in it is never announced to her.
    ana_live.conn.send(ClientFrame::CallJoin {
        channel_id: secret.id,
        audio: true,
        video: false,
    });
    call_of(&mut ana_live, secret.id).await;
    eva_live
        .assert_none(Duration::from_millis(500), |e| {
            matches!(e, ServerEvent::CallChanged { .. })
        })
        .await;
    let mut eva_again = Live::open(&eva);
    eva_again.ready().await;
    eva_again
        .assert_none(Duration::from_millis(300), |e| {
            matches!(e, ServerEvent::CallChanged { .. })
        })
        .await;

    // Joining from a second device moves Ana there.
    let ana_laptop = ana.another_device(&server).await;
    let mut laptop_live = Live::open(&ana_laptop);
    laptop_live.ready().await;
    call_of(&mut laptop_live, secret.id).await;
    laptop_live.conn.send(ClientFrame::CallJoin {
        channel_id: secret.id,
        audio: true,
        video: true,
    });
    let reason = ana_live
        .server_event(|e| match e {
            ServerEvent::CallLeft { channel_id, reason } if *channel_id == secret.id => {
                Some(*reason)
            }
            _ => None,
        })
        .await;
    assert_eq!(reason, CallLeftReason::OtherDevice);
    let people = call_of(&mut laptop_live, secret.id).await;
    assert_eq!(people.len(), 1);
    assert!(people[0].video);
    server.stop().await;
}
