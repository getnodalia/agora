//! Two-step verification end to end: setting it up, signing in with a code or a recovery code,
//! codes used once, and turning it off.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use agora_client::ClientError;
use agora_protocol::{ErrorCode, LoginRequest};
use common::*;

const PASSWORD: &str = "ana-test-password";

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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn code_of(e: ClientError) -> ErrorCode {
    match e {
        ClientError::Api { error, .. } => error.code,
        other => panic!("unexpected {other:?}"),
    }
}

fn login(code: Option<String>, recovery: Option<String>) -> LoginRequest {
    LoginRequest {
        username: "ana".into(),
        password: PASSWORD.into(),
        totp: code,
        recovery_code: recovery,
    }
}

#[tokio::test]
async fn sign_in_needs_a_second_factor_once_it_is_enabled() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, user) = register(&server, "ana").await;
    let (other, _) = server.anonymous().login("ana", PASSWORD).await.unwrap();
    assert!(!ana.mfa_status().await.unwrap().enabled);

    assert!(ana.mfa_setup("wrong password").await.is_err());
    let setup = ana.mfa_setup(PASSWORD).await.unwrap();
    assert!(setup.uri.starts_with("otpauth://totp/"));
    let secret = server
        .state
        .mfa
        .pending(user.id, agora_server::auth::now_ms())
        .unwrap();
    assert_eq!(
        code_of(ana.mfa_enable("000000").await.unwrap_err()),
        ErrorCode::Invalid
    );
    // A code from the previous step, so the next one is still free to sign in with.
    let codes = ana
        .mfa_enable(&code_at(secret.as_ref(), now_secs() - 30))
        .await
        .unwrap()
        .recovery_codes;
    assert_eq!(codes.len(), 10);
    let status = ana.mfa_status().await.unwrap();
    assert!(status.enabled);
    assert_eq!(status.recovery_codes_left, 10);
    // Enabling signed every other session out.
    assert!(other.me().await.is_err());
    assert!(ana.me().await.is_ok());

    let anon = server.anonymous();
    assert_eq!(
        code_of(anon.login("ana", PASSWORD).await.unwrap_err()),
        ErrorCode::MfaRequired
    );
    assert_eq!(
        code_of(
            anon.login_with(&login(Some("123456".into()), None))
                .await
                .unwrap_err()
        ),
        ErrorCode::MfaRequired
    );
    let current = code_at(secret.as_ref(), now_secs());
    anon.login_with(&login(Some(current.clone()), None))
        .await
        .unwrap();
    // The same code does not sign in twice.
    assert!(anon.login_with(&login(Some(current), None)).await.is_err());

    // A recovery code works once.
    let recovery = codes[0].clone();
    anon.login_with(&login(None, Some(recovery.clone())))
        .await
        .unwrap();
    assert!(anon.login_with(&login(None, Some(recovery))).await.is_err());
    assert_eq!(ana.mfa_status().await.unwrap().recovery_codes_left, 9);

    // Turning it off needs the password and a fresh code; then a password is enough again.
    assert!(ana.mfa_disable(PASSWORD, "000000").await.is_err());
    // The next step's code is accepted within the allowed clock skew.
    let (_, step) = server
        .state
        .store
        .totp_factor(user.id)
        .await
        .unwrap()
        .unwrap();
    let next = code_at(secret.as_ref(), (step as u64 + 1) * 30);
    ana.mfa_disable(PASSWORD, &next).await.unwrap();
    assert!(!ana.mfa_status().await.unwrap().enabled);
    anon.login("ana", PASSWORD).await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn setup_requires_the_password_and_a_fresh_pending_secret() {
    let server = Server::start("sqlite::memory:").await;
    let (ana, _) = register(&server, "ana").await;
    assert_eq!(
        code_of(ana.mfa_enable("123456").await.unwrap_err()),
        ErrorCode::Invalid
    );
    assert_eq!(
        code_of(ana.mfa_new_recovery_codes("123456").await.unwrap_err()),
        ErrorCode::Invalid
    );
    server.stop().await;
}
