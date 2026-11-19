//! Getting back into an account without email: the recovery key, a code from whoever runs the
//! server, and changing a password while signed in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_client::ClientError;
use agora_protocol::{ErrorCode, ResetPasswordRequest};
use agora_server::recovery::{self, Purpose};
use common::*;

const PASSWORD: &str = "ana-test-password";
const NEW: &str = "a brand new password";

fn code_of(e: ClientError) -> ErrorCode {
    match e {
        ClientError::Api { error, .. } => error.code,
        other => panic!("unexpected {other:?}"),
    }
}

fn reset(username: &str, code: &str) -> ResetPasswordRequest {
    ResetPasswordRequest {
        username: username.into(),
        code: code.into(),
        new_password: NEW.into(),
        totp: None,
        recovery_code: None,
    }
}

#[tokio::test]
async fn the_recovery_key_sets_a_new_password_once() {
    let server = Server::start("sqlite::memory:").await;
    let ana = Person::register(&server, "ana").await;
    assert!(!ana.recovery_status().await.unwrap().has_key);
    assert!(ana.new_recovery_key("wrong password").await.is_err());
    let key = ana.new_recovery_key(PASSWORD).await.unwrap().recovery_key;
    assert!(ana.recovery_status().await.unwrap().has_key);
    let anon = server.anonymous();

    // Wrong codes and unknown accounts look the same.
    for (user, code) in [("ana", "AAAA-BBBB"), ("nobody", key.as_str())] {
        assert_eq!(
            code_of(anon.reset_password(&reset(user, code)).await.unwrap_err()),
            ErrorCode::Invalid
        );
    }
    // A password that is too short is refused before anything is used up.
    let mut short = reset("ana", &key);
    short.new_password = "short".into();
    assert!(anon.reset_password(&short).await.is_err());

    // Typed the way a person would: lower case, spaces.
    let typed = key.to_lowercase().replace('-', " ");
    let (fresh, session, next_key) = anon.reset_password(&reset(" Ana ", &typed)).await.unwrap();
    assert_eq!(session.user.username, "ana");
    fresh.me().await.unwrap();
    assert_ne!(next_key, key);

    // Every older session is signed out, and the old password is gone.
    assert!(ana.me().await.is_err());
    assert!(anon.login("ana", PASSWORD).await.is_err());
    anon.login("ana", NEW).await.unwrap();
    // The key worked once; the replacement works instead.
    assert_eq!(
        code_of(anon.reset_password(&reset("ana", &key)).await.unwrap_err()),
        ErrorCode::Invalid
    );
    anon.reset_password(&reset("ana", &next_key)).await.unwrap();
}

#[tokio::test]
async fn a_code_from_the_operator_works_once_and_expires() {
    let server = Server::start("sqlite::memory:").await;
    let ana = Person::register(&server, "ana").await;
    let store = &server.state.store;
    let anon = server.anonymous();
    let user = store.user_id_by_username("ana").await.unwrap().unwrap();
    let issue = |expires: i64| {
        let code = recovery::new_code();
        let hash = recovery::hash(user, Purpose::Operator, &code);
        async move {
            store
                .set_reset_code(user, &hash, expires, false)
                .await
                .unwrap();
            code
        }
    };

    let late = issue(agora_server::auth::now_ms() - 1).await;
    assert_eq!(
        code_of(anon.reset_password(&reset("ana", &late)).await.unwrap_err()),
        ErrorCode::Invalid
    );
    let code = issue(agora_server::auth::now_ms() + 60_000).await;
    // The operator's code is not the recovery key, and the other way round.
    let key = ana.new_recovery_key(PASSWORD).await.unwrap().recovery_key;
    anon.reset_password(&reset("ana", &code)).await.unwrap();
    assert_eq!(
        code_of(anon.reset_password(&reset("ana", &code)).await.unwrap_err()),
        ErrorCode::Invalid
    );
    // Using the code replaced the recovery key as well.
    assert_eq!(
        code_of(anon.reset_password(&reset("ana", &key)).await.unwrap_err()),
        ErrorCode::Invalid
    );
}

#[tokio::test]
async fn two_step_accounts_need_the_second_factor_unless_the_operator_removes_it() {
    let server = Server::start("sqlite::memory:").await;
    let ana = Person::register(&server, "ana").await;
    let key = ana.new_recovery_key(PASSWORD).await.unwrap().recovery_key;
    ana.mfa_setup(PASSWORD).await.unwrap();
    let secret = server
        .state
        .mfa
        .pending(ana.user.id, agora_server::auth::now_ms())
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let codes = ana
        .mfa_enable(&code_at(secret.as_ref(), now - 30))
        .await
        .unwrap();
    let anon = server.anonymous();

    // The key alone is not enough.
    assert_eq!(
        code_of(anon.reset_password(&reset("ana", &key)).await.unwrap_err()),
        ErrorCode::MfaRequired
    );
    let mut wrong = reset("ana", &key);
    wrong.totp = Some("000000".into());
    assert_eq!(
        code_of(anon.reset_password(&wrong).await.unwrap_err()),
        ErrorCode::MfaRequired
    );
    // One of the account's own recovery codes does.
    let mut with_spare = reset("ana", &key);
    with_spare.recovery_code = Some(codes.recovery_codes[0].clone());
    let (_, _, key) = anon.reset_password(&with_spare).await.unwrap();
    assert!(
        anon.login("ana", NEW).await.is_err(),
        "still asks for a code"
    );

    // The operator can lift the second factor too.
    let user = server
        .state
        .store
        .user_id_by_username("ana")
        .await
        .unwrap()
        .unwrap();
    let code = recovery::new_code();
    server
        .state
        .store
        .set_reset_code(
            user,
            &recovery::hash(user, Purpose::Operator, &code),
            agora_server::auth::now_ms() + 60_000,
            true,
        )
        .await
        .unwrap();
    let _ = key;
    anon.reset_password(&reset("ana", &code)).await.unwrap();
    anon.login("ana", NEW).await.unwrap();
}

#[tokio::test]
async fn changing_the_password_needs_the_old_one_and_signs_out_the_others() {
    let server = Server::start("sqlite::memory:").await;
    let ana = Person::register(&server, "ana").await;
    let laptop = ana.another_device(&server).await;
    assert!(ana.change_password("wrong password", NEW).await.is_err());
    assert!(ana.change_password(PASSWORD, "short").await.is_err());
    ana.change_password(PASSWORD, NEW).await.unwrap();
    ana.me().await.unwrap();
    assert!(laptop.me().await.is_err());
    let anon = server.anonymous();
    assert!(anon.login("ana", PASSWORD).await.is_err());
    anon.login("ana", NEW).await.unwrap();
}

fn code_at(secret: &[u8], seconds: u64) -> String {
    totp_rs::Builder::new()
        .with_secret(secret.to_vec())
        .with_digits(6)
        .with_step_duration(30)
        .build()
        .unwrap()
        .generate(seconds)
        .to_string()
}
