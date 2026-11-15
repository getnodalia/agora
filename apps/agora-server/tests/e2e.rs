//! End-to-end encryption, checked against a real server: what the server stores, who can read
//! what, and what it refuses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_client::new_nonce;
use agora_protocol::{
    CommitRequest, CreateChannelRequest, CreateInviteRequest, EditMessageRequest, ErrorCode,
    PostMessageRequest,
};
use common::*;

async fn team(
    server: &Server,
) -> (
    Person,
    Person,
    agora_protocol::Workspace,
    agora_protocol::Channel,
) {
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
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    (ana, luis, ws, general)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn the_server_never_stores_or_sends_the_text() {
    let (dir, db) = temp_db("e2e-plaintext");
    let server = Server::start(&db).await;
    let (ana, luis, ws, general) = team(&server).await;
    let mut live = Live::open(&luis);
    live.ready().await;

    let marker = "contraseña-del-wifi-Kq7zP2";
    let sent = ana
        .say(general.id, &format!("La {marker} está en la nevera"))
        .await;
    ana.edit(&sent, &format!("Corrijo: {marker}!"))
        .await
        .unwrap();
    let dm = ana.open_direct(ws.id, luis.user.id).await.unwrap();
    ana.say(dm.id, marker).await;

    // Over the wire, events and history carry no text.
    let event = live
        .server_event(|e| match e {
            agora_protocol::ServerEvent::MessageCreated { message } => Some(message.clone()),
            _ => None,
        })
        .await;
    assert!(event.body.is_empty());
    let raw = luis
        .client
        .history(general.id, None, None, None)
        .await
        .unwrap();
    assert!(raw
        .messages
        .iter()
        .all(|m| m.encrypted && m.body.is_empty()));
    // Luis's device reads it all the same.
    assert_eq!(luis.read(event).await.body, format!("Corrijo: {marker}!"));

    // On disk: not in any table, not in the WAL, while running and once closed. Closing
    // checkpoints the WAL into the database and may delete it while the folder is read.
    let scan = || {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let Ok(bytes) = std::fs::read(entry.unwrap().path()) else {
                continue;
            };
            assert!(
                !contains(&bytes, marker.as_bytes()),
                "plaintext reached the database"
            );
        }
    };
    scan();
    server.stop().await;
    scan();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn unencrypted_and_foreign_messages_are_refused() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, _luis, ws, general) = team(&server).await;
    let plain = ana
        .post_message(
            general.id,
            &PostMessageRequest {
                body: "hola en claro".into(),
                ciphertext: None,
                mentions: Vec::new(),
                client_nonce: new_nonce(),
                thread_root_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(plain.code(), Some(ErrorCode::Invalid));
    let junk = ana
        .post_message(
            general.id,
            &PostMessageRequest {
                body: String::new(),
                ciphertext: Some("bm90IG1scw==".into()),
                mentions: Vec::new(),
                client_nonce: new_nonce(),
                thread_root_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(junk.code(), Some(ErrorCode::Invalid));

    // A real message for #general, replayed into another channel, is refused: wrong group.
    let other = ana
        .create_channel(
            ws.id,
            &CreateChannelRequest {
                name: "otro".into(),
                topic: String::new(),
                private: false,
            },
        )
        .await
        .unwrap();
    ana.say(general.id, "uno").await;
    ana.say(other.id, "dos").await;
    let entry = ana.mls_state(general.id, Some(0)).await.unwrap().entries;
    let ciphertext = entry
        .iter()
        .find(|e| e.message_id.is_some())
        .unwrap()
        .data
        .clone();
    let replay = ana
        .post_message(
            other.id,
            &PostMessageRequest {
                body: String::new(),
                ciphertext: Some(ciphertext.clone()),
                mentions: Vec::new(),
                client_nonce: new_nonce(),
                thread_root_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(replay.code(), Some(ErrorCode::Invalid));
    let edit = ana
        .edit_message(
            entry.iter().find_map(|e| e.message_id).unwrap(),
            &EditMessageRequest {
                body: "en claro".into(),
                ciphertext: None,
                mentions: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(edit.code(), Some(ErrorCode::Invalid));
    server.stop().await;
}

#[tokio::test]
async fn a_message_from_an_old_epoch_is_refused() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, luis, _ws, general) = team(&server).await;
    ana.say(general.id, "uno").await;
    let old = ana.mls_state(general.id, Some(0)).await.unwrap();
    let stale = old
        .entries
        .iter()
        .find(|e| e.message_id.is_some())
        .unwrap()
        .data
        .clone();
    // A new device joins: the epoch moves on, and the old ciphertext no longer fits.
    let _second = luis.another_device(&server).await;
    luis.say(general.id, "dos").await;
    let err = ana
        .post_message(
            general.id,
            &PostMessageRequest {
                body: String::new(),
                ciphertext: Some(stale),
                mentions: Vec::new(),
                client_nonce: new_nonce(),
                thread_root_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::StaleEpoch));
    // Ana's client catches up and sends anyway.
    assert_eq!(ana.say(general.id, "tres").await.body, "tres");
    server.stop().await;
}

#[tokio::test]
async fn someone_who_leaves_cannot_read_what_follows() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, luis, ws, _general) = team(&server).await;
    let room = ana
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
    ana.add_channel_member(room.id, luis.user.id).await.unwrap();
    let hello = ana.say(room.id, "hola, Luis").await;
    assert_eq!(luis.read(hello).await.body, "hola, Luis");
    assert_eq!(ana.e2e.members(room.id).await.len(), 2);

    luis.leave_channel(room.id).await.unwrap();
    let secret = ana.say(room.id, "esto ya no lo lee Luis").await;
    // Ana's device took Luis's out of the group before encrypting.
    let members = ana.e2e.members(room.id).await;
    assert!(members.iter().all(|m| m.user != luis.user.id));

    // Even back in the channel, Luis's device cannot open what was said while he was out.
    ana.add_channel_member(room.id, luis.user.id).await.unwrap();
    assert!(luis.read(secret).await.body.is_empty());
    let later = ana.say(room.id, "ya estás de vuelta").await;
    assert_eq!(luis.read(later).await.body, "ya estás de vuelta");
    server.stop().await;
}

#[tokio::test]
async fn devices_read_from_when_they_were_added() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, luis, _ws, general) = team(&server).await;
    let old = ana.say(general.id, "antes del portátil").await;

    // Luis signs in on a laptop. It never had the keys for what came before.
    let laptop = luis.another_device(&server).await;
    let page = laptop.history(general.id).await;
    let before = page.messages.iter().find(|m| m.id == old.id).unwrap();
    assert!(before.encrypted && before.body.is_empty());
    let new = ana.say(general.id, "después del portátil").await;
    assert_eq!(laptop.read(new.clone()).await.body, "después del portátil");
    // His first device still reads both.
    assert_eq!(luis.read(old).await.body, "antes del portátil");
    assert_eq!(luis.read(new).await.body, "después del portátil");

    // Logging the laptop out takes it out of the conversation at the next message.
    laptop.logout().await.unwrap();
    ana.say(general.id, "solo dispositivos activos").await;
    let members = ana.e2e.members(general.id).await;
    assert_eq!(members.iter().filter(|m| m.user == luis.user.id).count(), 1);
    server.stop().await;
}

#[tokio::test]
async fn a_device_added_while_offline_reads_on_its_return() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, luis, ws, _general) = team(&server).await;
    // Luis has never opened the conversation: Ana's device adds his from its key package.
    let dm = ana.open_direct(ws.id, luis.user.id).await.unwrap();
    let first = ana.say(dm.id, "¿estás?").await;
    let second = ana.say(dm.id, "te dejo esto aquí").await;
    assert_eq!(luis.read(first).await.body, "¿estás?");
    assert_eq!(luis.read(second).await.body, "te dejo esto aquí");
    // And search finds it in his own decrypted copy; the server cannot.
    let found = luis.e2e.search(ws.id, "DEJO").await.unwrap();
    assert_eq!(found.messages.len(), 1);
    assert_eq!(found.messages[0].body, "te dejo esto aquí");
    assert!(luis
        .client
        .search(ws.id, "dejo")
        .await
        .unwrap()
        .messages
        .is_empty());
    server.stop().await;
}

#[tokio::test]
async fn strangers_cannot_touch_a_conversations_encryption() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, _luis, ws, general) = team(&server).await;
    ana.say(general.id, "hola").await;
    let eva = Person::register(&server, "eva").await;
    let state = ana.mls_state(general.id, Some(0)).await.unwrap();
    let commit = state.entries[0].data.clone();
    for err in [
        eva.mls_state(general.id, None).await.err(),
        eva.mls_create_group(general.id, state.group_info.as_ref().unwrap())
            .await
            .err(),
        eva.mls_commit(
            general.id,
            &CommitRequest {
                commit: commit.clone(),
                group_info: state.group_info.clone().unwrap(),
                welcome: None,
                welcome_to: Vec::new(),
            },
        )
        .await
        .err(),
        eva.claim_key_packages(general.id, vec![state.devices[0].public_key.clone()])
            .await
            .err(),
        eva.lookup_messages(ws.id, &[]).await.err(),
    ] {
        assert_eq!(err.and_then(|e| e.code()), Some(ErrorCode::NotFound));
    }
    // A device key belongs to one account.
    let stolen = eva
        .register_device(&state.devices[0].public_key)
        .await
        .unwrap_err();
    assert_eq!(stolen.code(), Some(ErrorCode::Conflict));
    server.stop().await;
}
