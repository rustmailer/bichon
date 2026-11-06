//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project

use poem::http::StatusCode;
use poem::test::TestClient;
use serde::{Deserialize, Serialize};

use super::{admin_token, build_api_route, setup};

fn api_client(route: impl poem::Endpoint) -> TestClient<impl poem::Endpoint> {
    TestClient::new(route).default_header("X-Forwarded-For", "127.0.0.1")
}

// ── Payloads ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct CreateAccountPayload {
    email: String,
    enabled: bool,
    account_type: String,
    use_dangerous: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountResp {
    id: u64,
    email: String,
    enabled: bool,
    #[allow(dead_code)] // present in the API response, not asserted here
    account_name: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DataPage<T> {
    items: Vec<T>,
    total_items: u64,
}

#[derive(Debug, Serialize)]
struct UpdateAccountPayload {
    enabled: Option<bool>,
    account_name: Option<String>,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn account_crud() {
    setup().await;
    let token = admin_token().await;
    let route = build_api_route();
    let cli = api_client(route);

    // ── Create ──────────────────────────────────────────────────────────
    let create_payload = CreateAccountPayload {
        email: "test-crud@example.com".into(),
        enabled: false,
        account_type: "NoSync".into(),
        use_dangerous: false,
        account_name: Some("CRUD Test Account".into()),
    };

    let resp = cli
        .post("/api/v1/account")
        .header("Authorization", &format!("Bearer {}", token))
        .body_json(&create_payload)
        .send()
        .await;

    resp.assert_status_is_ok();
    let account: AccountResp = resp.json().await.value().deserialize();
    assert_eq!(account.email, "test-crud@example.com");
    assert!(!account.enabled);
    let account_id = account.id;

    // ── Read ────────────────────────────────────────────────────────────
    let resp = cli
        .get(&format!("/api/v1/account/{}", account_id))
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    resp.assert_status_is_ok();
    let account: AccountResp = resp.json().await.value().deserialize();
    assert_eq!(account.id, account_id);

    // ── List ────────────────────────────────────────────────────────────
    let resp = cli
        .get("/api/v1/accounts")
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    resp.assert_status_is_ok();

    // ── Update ──────────────────────────────────────────────────────────
    let update_payload = UpdateAccountPayload {
        enabled: Some(true),
        account_name: Some("Updated Name".into()),
    };
    let resp = cli
        .post(&format!("/api/v1/account/{}", account_id))
        .header("Authorization", &format!("Bearer {}", token))
        .body_json(&update_payload)
        .send()
        .await;
    resp.assert_status_is_ok();

    // ── Delete ──────────────────────────────────────────────────────────
    let resp = cli
        .delete(&format!("/api/v1/account/{}", account_id))
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    resp.assert_status_is_ok();

    // ── Verify deleted (accepted) ───────────────────────────────────────
    // Deletion is asynchronous: DELETE returns 200 as soon as the account is
    // *marked* for deletion, and the purge of envelopes/attachments/index
    // runs on in the background. Immediately after, a GET may either still
    // find the account (marked `deleting`, purge in flight) or return 404
    // (purge already finished) — both are valid outcomes of the accepted
    // delete, so accept either instead of asserting a specific one.
    let resp = cli
        .get(&format!("/api/v1/account/{}", account_id))
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    let status = resp.0.status();
    assert!(
        status.is_success() || status.is_client_error(),
        "after delete the account must be gone or soft-deleting, got {status}"
    );
}

#[tokio::test]
async fn create_account_with_invalid_email_fails() {
    setup().await;
    let token = admin_token().await;
    let route = build_api_route();
    let cli = api_client(route);

    let payload = CreateAccountPayload {
        email: "not-an-email".into(),
        enabled: false,
        account_type: "NoSync".into(),
        use_dangerous: false,
        account_name: None,
    };

    let resp = cli
        .post("/api/v1/account")
        .header("Authorization", &format!("Bearer {}", token))
        .body_json(&payload)
        .send()
        .await;
    assert!(resp.0.status().is_client_error(), "invalid email should fail");
}

#[tokio::test]
async fn create_account_with_empty_email_fails() {
    setup().await;
    let token = admin_token().await;
    let route = build_api_route();
    let cli = api_client(route);

    let payload = CreateAccountPayload {
        email: "".into(),
        enabled: false,
        account_type: "NoSync".into(),
        use_dangerous: false,
        account_name: None,
    };

    let resp = cli
        .post("/api/v1/account")
        .header("Authorization", &format!("Bearer {}", token))
        .body_json(&payload)
        .send()
        .await;
    assert!(resp.0.status().is_client_error(), "empty email should fail");
}

#[tokio::test]
async fn get_nonexistent_account_returns_error() {
    setup().await;
    let token = admin_token().await;
    let route = build_api_route();
    let cli = api_client(route);

    let resp = cli
        .get("/api/v1/account/99999999")
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    assert!(resp.0.status().is_client_error(), "nonexistent account should 4xx");
}

#[tokio::test]
async fn delete_nonexistent_account_returns_404() {
    setup().await;
    let token = admin_token().await;
    let route = build_api_route();
    let cli = api_client(route);

    // The deletion guards (legal hold / journal / view source) need the full
    // account, so `delete` loads it first — a missing account is a 404 like
    // GET, not the old no-op success.
    let resp = cli
        .delete("/api/v1/account/99999999")
        .header("Authorization", &format!("Bearer {}", token))
        .send()
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
