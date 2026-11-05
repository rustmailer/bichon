//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use crate::common::auth::ApiGuard;
use crate::common::error::ErrorCapture;
use crate::common::log::Tracing;
use crate::common::recorder::Recorder;
use crate::common::tls::rustls_config;
use crate::common::timeout::{Timeout, TIMEOUT_HEADER};
use crate::error::handler::error_handler;
use crate::rest::public::features::get_features;
use crate::rest::public::login::login;
use crate::rest::public::mfa::mfa_verify;
use crate::rest::public::status::get_status;
use bichon_core::common::signal::SIGNAL_MANAGER;
use bichon_core::error::code::ErrorCode;
use bichon_core::error::BichonResult;
use bichon_core::raise_error;
use bichon_core::settings::cli::SETTINGS;

use api::create_openapi_service;
use http::Method;
use poem::listener::{Listener, TcpListener};
use poem::middleware::{CatchPanic, Compression, Cors};
use poem::{get, post, Endpoint, EndpointExt, Route, Server};
use public::oauth2::oauth2_callback;
use std::collections::HashSet;
use std::time::Duration;

#[cfg(feature = "embed-web")]
use {
    assets::FrontEndAssets,
    http::HeaderValue,
    poem::{handler, endpoint::EmbeddedFilesEndpoint, IntoResponse},
    poem::middleware::SetHeader,
};

pub mod api;
pub mod assets;
pub mod health;
pub mod maintenance;
pub mod metrics;
pub mod public;

pub type ApiResult<T, E = ApiErrorResponse> = std::result::Result<T, E>;

use super::error::ApiErrorResponse;

/// Build the community route tree. Pro/Enterprise servers can call this
/// and then add their own routes before passing the tree to the server.
pub fn build_routes() -> impl Endpoint {
    let api_service = create_openapi_service()
        .summary("A lightweight, high-performance Rust email archiver with WebUI");

    let swagger = api_service.swagger_ui();
    let redoc = api_service.redoc();
    let scalar = api_service.scalar();
    let spec_json = api_service.spec_endpoint();
    let spec_yaml = api_service.spec_endpoint_yaml();
    let openapi_explorer = api_service.openapi_explorer();

    let open_api_route = Route::new()
        .nest_no_strip("/api/v1", api_service)
        .with(ApiGuard)
        .with(ErrorCapture)
        .with(Timeout)
        .with(Tracing)
        .with(Recorder);

    let cors_origins: Option<HashSet<String>> = SETTINGS.bichon_cors_origins.clone();
    let cors_origins: Vec<String> = cors_origins.unwrap_or_default().into_iter().collect();

    let cors = Cors::new()
        .allow_origins_fn(move |origin| {
            tracing::debug!("CORS: Incoming Origin = {:?}", origin);
            tracing::debug!("CORS: Configured origins = {:?}", cors_origins);
            if cors_origins.is_empty() {
                tracing::debug!("CORS: No origins configured, allowing all");
                return true;
            }
            cors_origins.iter().any(|o| o == origin)
        })
        .allow_credentials(true)
        .allow_methods(&[
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::HEAD,
            Method::PATCH,
        ])
        .allow_headers(vec!["Content-Type", "Authorization", TIMEOUT_HEADER])
        .expose_headers(vec!["Accept"])
        .max_age(SETTINGS.bichon_cors_max_age);

    let app_logic = Route::new()
        .nest("/api-docs/swagger", swagger)
        .nest("/api-docs/redoc", redoc)
        .nest("/api-docs/explorer", openapi_explorer)
        .nest("/api-docs/scalar", scalar)
        .nest("/api-docs/spec.json", spec_json)
        .nest("/api-docs/spec.yaml", spec_yaml)
        .nest("/oauth2/callback", get(oauth2_callback))
        // `.at` (not `.nest`) so the inner endpoint sees the full request path:
        // `ApiGuard` uses it to decide whether a metrics-only token may be
        // used, and `nest` would strip the prefix before the guard runs.
        .at(
            "/api/v1/metrics",
            get(crate::rest::metrics::get_metrics).with(ApiGuard),
        )
        .nest("/api/v1/features", get(get_features))
        .nest("/api/status", get(get_status))
        .nest("/api/login", post(login))
        .nest("/api/auth/mfa/verify", post(mfa_verify))
        .at(
            "/api/v1/exports/download/:ticket",
            get(crate::rest::api::export::download_export_ticket_handler),
        )
        .nest_no_strip("/api/v1", open_api_route);

    let app_logic = add_web_assets(app_logic);

    Route::new()
        // Liveness only, mounted outside `bichon_base_url` so a sub-path
        // deployment keeps a stable probe path. Readiness stays at
        // `/api/status`, which is intentionally 503 in maintenance mode.
        .at("/healthz", get(health::healthz))
        .nest(&SETTINGS.bichon_base_url, app_logic)
        .with(cors)
        .with_if(SETTINGS.bichon_http_compression_enabled, Compression::new())
        .with(CatchPanic::new())
}

#[cfg(feature = "embed-web")]
fn add_web_assets(route: Route) -> impl Endpoint {
    let cache_static = || {
        SetHeader::new().overriding(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("max-age=86400"),
        )
    };

    route
        .nest_no_strip(
            "/assets",
            EmbeddedFilesEndpoint::<FrontEndAssets>::new().with(cache_static()),
        )
        .at("/*", serve_index_with_base)
}

#[cfg(not(feature = "embed-web"))]
fn add_web_assets(route: Route) -> Route {
    route
}

#[cfg(feature = "embed-web")]
#[handler]
async fn serve_index_with_base() -> impl IntoResponse {
    let mut html =
        String::from_utf8_lossy(&FrontEndAssets::get("index.html").unwrap().data).to_string();

    let raw_base = &SETTINGS.bichon_base_url;
    let base_href = if raw_base.ends_with('/') {
        raw_base.clone()
    } else {
        format!("{}/", raw_base)
    };

    let inject_content = format!(
        r#"<base href="{}"><script>window.__BICHON_BASE__ = '{}';</script>"#,
        base_href, raw_base
    );

    html = html.replace("<head>", &format!("<head>{}", inject_content));
    poem::Response::builder()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

pub async fn start_http_server() -> BichonResult<()> {
    let listener = TcpListener::bind((
        SETTINGS.bichon_bind_ip.clone().unwrap_or("0.0.0.0".into()),
        SETTINGS.bichon_http_port as u16,
    ));

    let listener = if SETTINGS.bichon_enable_rest_https {
        listener.rustls(rustls_config()?).boxed()
    } else {
        listener.boxed()
    };

    let route = build_routes();

    let mut rx = SIGNAL_MANAGER.subscribe();
    let shutdown_fut = async move {
        let _ = rx.recv().await;
    };
    let server = Server::new(listener)
        .name("Bichon Service")
        .idle_timeout(Duration::from_secs(60))
        .run_with_graceful_shutdown(
            route.catch_all_error(error_handler),
            shutdown_fut,
            Some(Duration::from_secs(5)),
        );
    println!(
        "Bichon Service is now running on port {}.",
        SETTINGS.bichon_http_port
    );
    server
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}

#[cfg(test)]
mod liveness_route_tests {
    use super::*;
    use poem::{handler, http::StatusCode, Request};

    #[handler]
    async fn unavailable() -> StatusCode {
        StatusCode::SERVICE_UNAVAILABLE
    }

    /// `/healthz` must win over the root-nested application (which registers
    /// its own `/*` catch-all for the SPA) and must not inherit readiness.
    /// This mirrors the shape of `build_routes`.
    #[test]
    fn healthz_is_reachable_next_to_a_root_nested_app_with_catch_all() {
        let app = Route::new()
            .at("/healthz", get(health::healthz))
            .nest(
                "/",
                Route::new()
                    .at("/api/status", get(unavailable))
                    .at("/*", get(unavailable)),
            );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let status = |path: &'static str| {
            let req = Request::builder()
                .uri(poem::http::Uri::from_static(path))
                .finish();
            rt.block_on(app.call(req)).unwrap().status()
        };

        assert_eq!(status("/healthz"), StatusCode::OK);
        assert_eq!(status("/api/status"), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(status("/some/spa/route"), StatusCode::SERVICE_UNAVAILABLE);
    }
}
