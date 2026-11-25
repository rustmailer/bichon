//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project

use bichon_core::token::AccessTokenModel;
use bichon_core::users::UserModel;
use poem::test::TestClient;
use serde::{Deserialize, Serialize};

use super::{admin_token, build_api_route, setup};

const ADMIN_ROLE_ID: u64 = 100_000_000_000_000;

#[derive(Debug, Serialize)]
struct CreateUserPayload {
    username: String,
    email: String,
    password: String,
    global_roles: Vec<u64>,
    account_access_map: std::collections::BTreeMap<u64, u64>,
}

#[derive(Debug, Deserialize)]
struct UserView {
    id: u64,
}

/// The per-user ticket mechanics behind the forced first-login password change:
/// the ticket is never a usable session token, it is one-time, a validation
/// error does not consume it, and the new password works afterwards.
///
/// The built-in administrator's own default-password branch is not exercised
/// here because the shared suite replaces that password during setup (it would
/// race every other test). This test drives the same core functions with a
/// regular user's id.
#[tokio::test]
async fn password_change_ticket_is_one_time_and_never_a_session() {
    setup().await;
    let token = admin_token().await;
    let cli = TestClient::new(build_api_route())
        .default_header("X-Forwarded-For", "127.0.0.1");

    let username = "pwdchange1";
    let initial_password = "initial-pass-123";
    let resp = cli
        .post("/api/v1/users")
        .header("Authorization", &format!("Bearer {}", token))
        .body_json(&CreateUserPayload {
            username: username.into(),
            email: "pwdchange1@example.com".into(),
            password: initial_password.into(),
            global_roles: vec![ADMIN_ROLE_ID],
            account_access_map: Default::default(),
        })
        .send()
        .await;
    resp.assert_status_is_ok();
    let user: UserView = resp.json().await.value().deserialize();

    let ticket = AccessTokenModel::new_password_change_ticket(user.id).unwrap();

    // A password-change ticket must never authenticate a request.
    assert!(
        AccessTokenModel::resolve_user_from_token(&ticket).is_err(),
        "a password change ticket must not be usable as an access token"
    );

    // A rejected new password must not burn the one-time ticket.
    assert!(UserModel::complete_password_change(&ticket, "short").is_err());
    let result = UserModel::complete_password_change(&ticket, "new-password-123")
        .expect("the ticket must still be valid after a validation error");
    assert!(result.success);
    assert!(result.access_token.is_some());
    assert!(!result.must_change_password);

    // Replay: the ticket is gone.
    assert!(UserModel::complete_password_change(&ticket, "another-password-123").is_err());

    // The new password works and no longer asks for a change.
    let login = UserModel::authenticate_user(username.to_string(), "new-password-123".into())
        .expect("login with the new password");
    assert!(login.success);
    assert!(login.access_token.is_some());
    assert!(!login.must_change_password);
}

/// An administrator resetting another user's password signs that user out;
/// the user's existing WebUI token must stop working immediately.
#[tokio::test]
async fn admin_password_reset_revokes_the_target_session() {
    setup().await;
    let admin = admin_token().await;
    let cli = TestClient::new(build_api_route())
        .default_header("X-Forwarded-For", "127.0.0.1");

    let username = "pwdreset1";
    let resp = cli
        .post("/api/v1/users")
        .header("Authorization", &format!("Bearer {}", admin))
        .body_json(&CreateUserPayload {
            username: username.into(),
            email: "pwdreset1@example.com".into(),
            password: "initial-pass-123".into(),
            global_roles: vec![ADMIN_ROLE_ID],
            account_access_map: Default::default(),
        })
        .send()
        .await;
    resp.assert_status_is_ok();
    let user: UserView = resp.json().await.value().deserialize();

    let user_login = UserModel::authenticate_user(username.to_string(), "initial-pass-123".into())
        .expect("initial login");
    let user_token = user_login.access_token.expect("user token");

    let resp = cli
        .get("/api/v1/current-user")
        .header("Authorization", &format!("Bearer {}", user_token))
        .send()
        .await;
    resp.assert_status_is_ok();

    // Admin resets the target's password.
    let resp = cli
        .post(&format!("/api/v1/users/{}", user.id))
        .header("Authorization", &format!("Bearer {}", admin))
        .body_json(&serde_json::json!({ "password": "reset-pass-999" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    // The old session is gone; the new password authenticates.
    let resp = cli
        .get("/api/v1/current-user")
        .header("Authorization", &format!("Bearer {}", user_token))
        .send()
        .await;
    assert_eq!(
        resp.0.status().as_u16(),
        401,
        "the target's session must be revoked after an admin password reset"
    );
    let relogin = UserModel::authenticate_user(username.to_string(), "reset-pass-999".into())
        .expect("login with the reset password");
    assert!(relogin.success && relogin.access_token.is_some());
}
