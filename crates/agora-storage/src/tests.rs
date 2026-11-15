use agora_core::{ChannelKind, MessageId, Role};

use super::*;
use crate::messages::{Inserted, NewMessage};

async fn store() -> Store {
    Store::open("sqlite::memory:").await.unwrap()
}

#[tokio::test]
async fn usernames_are_unique() {
    let s = store().await;
    s.create_user("ana", "Ana", "h", 1).await.unwrap();
    assert!(matches!(
        s.create_user("ana", "Otra", "h", 2).await,
        Err(StoreError::Conflict)
    ));
}

#[tokio::test]
async fn a_new_workspace_has_general_and_its_owner() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    assert_eq!(s.role(ws, ana.id).await.unwrap(), Some(Role::Owner));
    let channels = s.channels_for(ws, ana.id, Role::Owner).await.unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "general");
    assert!(channels[0].joined);
}

#[tokio::test]
async fn retried_nonce_returns_the_same_message() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    let ch = s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].id;
    let new = NewMessage {
        channel: ch,
        author: ana.id,
        thread_root: None,
        body: "hola",
        nonce: "n1",
        mentioned: &[],
        ciphertext: None,
        now: 5,
    };
    let Inserted::Created(first) = s.insert_message(new.clone()).await.unwrap() else {
        panic!()
    };
    let Inserted::Existing(again) = s
        .insert_message(NewMessage {
            body: "changed",
            ..new
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(first, again);
    assert_eq!(s.history(ch, None, None, 50).await.unwrap().0.len(), 1);
}

#[tokio::test]
async fn replies_update_their_root_and_stay_out_of_history() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    let ch = s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].id;
    let msg = |nonce: &'static str, root: Option<MessageId>| NewMessage {
        channel: ch,
        author: ana.id,
        thread_root: root,
        body: "x",
        nonce,
        mentioned: &[],
        ciphertext: None,
        now: 9,
    };
    let Inserted::Created(root) = s.insert_message(msg("a", None)).await.unwrap() else {
        panic!()
    };
    s.insert_message(msg("b", Some(root.id))).await.unwrap();
    s.insert_message(msg("c", Some(root.id))).await.unwrap();
    let (history, _) = s.history(ch, None, None, 50).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].reply_count, 2);
    assert_eq!(s.replies(root.id, 100).await.unwrap().len(), 2);
}

#[tokio::test]
async fn history_pages_backwards_and_catches_up_forwards() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    let ch = s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].id;
    let mut ids = Vec::new();
    for i in 0..10 {
        let nonce = format!("n{i}");
        let Inserted::Created(m) = s
            .insert_message(NewMessage {
                channel: ch,
                author: ana.id,
                thread_root: None,
                body: "x",
                nonce: &nonce,
                mentioned: &[],
                ciphertext: None,
                now: i,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        ids.push(m.id);
    }
    let (latest, more) = s.history(ch, None, None, 4).await.unwrap();
    assert!(more);
    assert_eq!(latest.iter().map(|m| m.id).collect::<Vec<_>>(), ids[6..]);
    let (older, _) = s.history(ch, Some(ids[6]), None, 4).await.unwrap();
    assert_eq!(older.iter().map(|m| m.id).collect::<Vec<_>>(), ids[2..6]);
    let (since, more) = s.history(ch, None, Some(ids[7]), 50).await.unwrap();
    assert!(!more);
    assert_eq!(since.iter().map(|m| m.id).collect::<Vec<_>>(), ids[8..]);
}

#[tokio::test]
async fn private_channels_are_hidden_from_non_members() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let luis = s.create_user("luis", "Luis", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    s.add_member(ws, luis.id, Role::Member, 2).await.unwrap();
    let secret = s
        .create_channel(ws, "secret", "", ChannelKind::Private, ana.id, 3)
        .await
        .unwrap();
    let seen = s.channels_for(ws, luis.id, Role::Member).await.unwrap();
    assert!(seen.iter().all(|c| c.id != secret));
    let row = s.channel(secret).await.unwrap().unwrap();
    assert_eq!(s.channel_audience(&row).await.unwrap(), vec![ana.id]);
}

#[tokio::test]
async fn direct_conversations_are_found_again() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let luis = s.create_user("luis", "Luis", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    s.add_member(ws, luis.id, Role::Member, 2).await.unwrap();
    let a = s.open_direct(ws, ana.id, luis.id, 3).await.unwrap();
    let b = s.open_direct(ws, luis.id, ana.id, 4).await.unwrap();
    assert_eq!(a, b);
    let view = s.channel_view(a, ana.id).await.unwrap().unwrap();
    assert_eq!(view.peer.unwrap().id, luis.id);
}

#[tokio::test]
async fn invitations_count_their_uses() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let luis = s.create_user("luis", "Luis", "h", 1).await.unwrap();
    let eva = s.create_user("eva", "Eva", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    s.create_invite(crate::workspaces::NewInvite {
        workspace: ws,
        code_hash: b"code",
        role: Role::Member,
        by: ana.id,
        max_uses: 1,
        expires_at: 1_000,
        now: 1,
    })
    .await
    .unwrap();
    assert!(
        s.redeem_invite(b"code", luis.id, 2)
            .await
            .unwrap()
            .new_member
    );
    assert!(
        !s.redeem_invite(b"code", luis.id, 2)
            .await
            .unwrap()
            .new_member
    );
    assert!(matches!(
        s.redeem_invite(b"code", eva.id, 2).await,
        Err(StoreError::InviteExhausted)
    ));
    assert!(matches!(
        s.redeem_invite(b"nope", eva.id, 2).await,
        Err(StoreError::NotFound)
    ));
    // Luis landed in #general.
    assert!(s.channels_for(ws, luis.id, Role::Member).await.unwrap()[0].joined);
}

#[tokio::test]
async fn unread_counts_and_reactions() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let luis = s.create_user("luis", "Luis", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    s.add_member(ws, luis.id, Role::Member, 2).await.unwrap();
    let ch = s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].id;
    let Inserted::Created(m) = s
        .insert_message(NewMessage {
            channel: ch,
            author: ana.id,
            thread_root: None,
            body: "@luis mira",
            nonce: "n",
            mentioned: &[luis.id],
            ciphertext: None,
            now: 3,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let luis_view = s.channels_for(ws, luis.id, Role::Member).await.unwrap();
    assert_eq!((luis_view[0].unread, luis_view[0].mentions), (1, 1));
    assert_eq!(
        s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].unread,
        0
    );
    s.mark_read(ch, luis.id, m.id).await.unwrap();
    assert_eq!(
        s.channels_for(ws, luis.id, Role::Member).await.unwrap()[0].unread,
        0
    );

    let r = s.toggle_reaction(m.id, luis.id, "👍", 4).await.unwrap();
    assert_eq!(r[0].count, 1);
    assert!(s
        .toggle_reaction(m.id, luis.id, "👍", 5)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn search_respects_what_a_user_can_read() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let luis = s.create_user("luis", "Luis", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    s.add_member(ws, luis.id, Role::Member, 2).await.unwrap();
    let secret = s
        .create_channel(ws, "secret", "", ChannelKind::Private, ana.id, 3)
        .await
        .unwrap();
    let general = s
        .channels_for(ws, ana.id, Role::Owner)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == "general")
        .unwrap()
        .id;
    for (ch, nonce, body) in [
        (general, "a", "plan 100% listo"),
        (secret, "b", "plan secreto"),
    ] {
        s.insert_message(NewMessage {
            channel: ch,
            author: ana.id,
            thread_root: None,
            body,
            nonce,
            mentioned: &[],
            ciphertext: None,
            now: 4,
        })
        .await
        .unwrap();
    }
    assert_eq!(
        s.search(ws, ana.id, false, "plan", 10).await.unwrap().len(),
        2
    );
    assert_eq!(
        s.search(ws, luis.id, false, "plan", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        s.search(ws, luis.id, false, "100%", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        s.search(ws, luis.id, false, "_", 10).await.unwrap().len(),
        0
    );
}

#[tokio::test]
async fn data_survives_reopening_the_file() {
    let dir = std::env::temp_dir().join(format!("agora-store-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}", dir.join("agora.db").display());
    {
        let s = Store::open(&url).await.unwrap();
        s.create_user("ana", "Ana", "h", 1).await.unwrap();
        s.close().await;
    }
    let s = Store::open(&url).await.unwrap();
    assert!(s.user_credentials("ana").await.unwrap().is_some());
    s.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_encrypted_log_moves_one_epoch_at_a_time() {
    let s = store().await;
    let ana = s.create_user("ana", "Ana", "h", 1).await.unwrap();
    let ws = s.create_workspace("Lab", ana.id, 1).await.unwrap();
    let general = s.channels_for(ws, ana.id, Role::Owner).await.unwrap()[0].id;

    // No group yet: nothing encrypted can be stored.
    let send = |epoch: u64, nonce: &'static str| NewMessage {
        channel: general,
        author: ana.id,
        thread_root: None,
        body: "",
        nonce,
        mentioned: &[],
        ciphertext: Some((b"sealed", epoch)),
        now: 2,
    };
    assert!(matches!(
        s.insert_message(send(0, "a")).await,
        Err(StoreError::StaleEpoch)
    ));
    s.create_mls_group(general, 0, b"info0", 2).await.unwrap();
    assert!(matches!(
        s.create_mls_group(general, 0, b"again", 2).await,
        Err(StoreError::Conflict)
    ));
    let Inserted::Created(m) = s.insert_message(send(0, "b")).await.unwrap() else {
        panic!("expected a new message");
    };
    assert!(m.encrypted && m.body.is_empty() && m.content_seq.is_some());

    // A commit for epoch 0 moves the group to 1; a second one for 0 lost the race.
    s.mls_commit(general, 0, b"c0", b"info1", None, ana.id, 3)
        .await
        .unwrap();
    assert!(matches!(
        s.mls_commit(general, 0, b"c0bis", b"x", None, ana.id, 3)
            .await,
        Err(StoreError::StaleEpoch)
    ));
    assert!(matches!(
        s.insert_message(send(0, "c")).await,
        Err(StoreError::StaleEpoch)
    ));
    assert_eq!(
        s.mls_group(general).await.unwrap().unwrap(),
        (1, b"info1".to_vec())
    );
    let (log, more) = s.mls_log(general, 0, 10).await.unwrap();
    assert!(!more);
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].message, Some(m.id));

    // Deleting a message erases its ciphertext but keeps the entry, so devices keep their place.
    s.delete_message(m.id, 4).await.unwrap();
    let (log, _) = s.mls_log(general, 0, 10).await.unwrap();
    assert_eq!(log.len(), 2);
    assert!(log[0].data.is_empty());
}
