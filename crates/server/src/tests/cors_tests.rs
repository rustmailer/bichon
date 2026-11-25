//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project

use std::collections::HashSet;

use poem::http::header;
use poem::test::TestClient;
use poem::{get, handler, Endpoint, EndpointExt, Route};

use crate::rest::cors::{cors_middleware, SameOriginPassthrough};

#[handler]
async fn ok() -> &'static str {
    "ok"
}

fn app(origins: Option<&[&str]>) -> impl Endpoint {
    let origins: Option<HashSet<String>> =
        origins.map(|list| list.iter().map(|o| (*o).to_string()).collect());
    Route::new()
        .at("/", get(ok))
        .with(cors_middleware(origins, 86400))
        .with(SameOriginPassthrough::new("http://localhost:15630"))
}

const BACKEND: &str = "localhost:15630";

#[tokio::test]
async fn no_configured_origins_denies_cross_origin_and_allows_same_origin() {
    let cli = TestClient::new(app(None));

    // Cross-origin: rejected outright, no CORS headers.
    cli.get("/")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::HOST, BACKEND)
        .send()
        .await
        .assert_status(http::StatusCode::FORBIDDEN);

    // Same-origin: browser sends Origin on non-GET requests; it must pass and
    // needs no CORS headers.
    let resp = cli
        .get("/")
        .header(header::ORIGIN, "http://localhost:15630")
        .header(header::HOST, BACKEND)
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);

    // No Origin at all (plain same-origin GET, curl): untouched.
    cli.get("/").send().await.assert_status_is_ok();
}

#[tokio::test]
async fn configured_origin_is_allowed_with_credentials() {
    let cli = TestClient::new(app(Some(&["https://app.example"])));

    let resp = cli
        .get("/")
        .header(header::ORIGIN, "https://app.example")
        .header(header::HOST, BACKEND)
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "https://app.example");
    resp.assert_header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS, "true");
    resp.assert_header(header::VARY, "Origin");
}

#[tokio::test]
async fn unlisted_origin_is_denied_even_with_a_list() {
    let cli = TestClient::new(app(Some(&["https://app.example"])));
    cli.get("/")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::HOST, BACKEND)
        .send()
        .await
        .assert_status(http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn same_origin_is_not_reflected_and_needs_no_list_entry() {
    let cli = TestClient::new(app(Some(&["https://app.example"])));
    let resp = cli
        .get("/")
        .header(header::ORIGIN, "http://localhost:15630")
        .header(header::HOST, BACKEND)
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
}

#[tokio::test]
async fn forwarded_host_is_recognised_as_same_origin() {
    let cli = TestClient::new(app(None));
    let resp = cli
        .get("/")
        .header(header::ORIGIN, "https://archive.example.com")
        .header(header::HOST, "10.0.0.5:15630")
        .header("x-forwarded-host", "archive.example.com")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header_is_not_exist(header::ACCESS_CONTROL_ALLOW_ORIGIN);
}
