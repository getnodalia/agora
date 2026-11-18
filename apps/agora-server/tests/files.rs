//! Files in conversations: encrypted before upload, readable by those who can read the channel,
//! refused to everyone else.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_client::new_nonce;
use agora_core::content::{self, Content};
use agora_protocol::{CreateChannelRequest, CreateInviteRequest, ErrorCode};
use agora_server::config::Config;
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn a_file_reaches_the_channel_encrypted() {
    let (dir, db) = temp_db("files-flow");
    let server = Server::start(&db).await;
    let (ana, luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    luis.history(general.id).await;

    let marker = b"%PDF-1.7 contrato-secreto-Zq81";
    let sent = ana
        .e2e
        .send_file(
            general.id,
            "../Contrato final.pdf",
            "application/pdf",
            marker,
            "Firmado",
            &new_nonce(),
            None,
        )
        .await
        .unwrap();

    let got = luis.read(sent.clone()).await;
    let Content::File {
        attachment,
        caption,
    } = content::parse(&got.body)
    else {
        panic!("not a file: {:?}", got.body);
    };
    assert_eq!(caption, "Firmado");
    assert_eq!(attachment.name, "Contrato final.pdf");
    assert_eq!(attachment.size, marker.len() as u64);
    assert_eq!(luis.e2e.open_file(&attachment).await.unwrap(), marker);

    // Neither the stored bytes nor the database hold the file or its name.
    let stored = luis.download_file(attachment.file).await.unwrap();
    assert!(!contains(&stored, marker));
    server.stop().await;
    let mut raw = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        raw.extend(std::fs::read(entry.unwrap().path()).unwrap_or_default());
    }
    assert!(!contains(&raw, marker));
    assert!(!contains(&raw, b"Contrato final"));
}

#[tokio::test]
async fn people_outside_a_private_channel_cannot_upload_or_download() {
    let (_dir, db) = temp_db("files-private");
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
    let (sealed, _) = agora_client::files::seal(b"nominas").unwrap();
    let up = ana.upload_file(secret.id, sealed.clone()).await.unwrap();
    for err in [
        eva.download_file(up.id).await.err(),
        eva.upload_file(secret.id, sealed).await.err(),
        eva.download_file(up.id + 100).await.err(),
    ] {
        assert_eq!(err.and_then(|e| e.code()), Some(ErrorCode::NotFound));
    }
    assert!(luis.download_file(up.id).await.is_ok());
}

#[tokio::test]
async fn files_over_the_limit_are_refused() {
    let (_dir, db) = temp_db("files-limit");
    let mut config = Config {
        database: db,
        ..Config::default()
    };
    config.limits.max_file_bytes = 1024;
    let server = Server::start_with(config, 0).await;
    let (ana, _luis, _eva, ws) = team(&server).await;
    let general = ana.channels(ws.id).await.unwrap()[0].clone();
    let err = ana
        .upload_file(general.id, vec![0; 2048])
        .await
        .err()
        .and_then(|e| e.code());
    assert_eq!(err, Some(ErrorCode::Invalid));
    let err = ana
        .upload_file(general.id, vec![0; 4])
        .await
        .err()
        .and_then(|e| e.code());
    assert_eq!(err, Some(ErrorCode::Invalid));
    assert!(ana.upload_file(general.id, vec![0; 512]).await.is_ok());
}
