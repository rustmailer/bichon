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

use bichon_core::database::insert_impl;
use bichon_core::database::manager::DB_MANAGER;
use bichon_core::oidc::flow::{begin_login, complete_login};
use bichon_core::oidc::handoff::OidcHandoffEntity;
use bichon_core::settings::cli::SETTINGS;
use bichon_core::token::AccessTokenModel;
use bichon_core::users::UserModel;
use poem::{
    handler,
    web::{Json, Query, Redirect},
    IntoResponse, Response, Result,
};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

const OIDC_PROVIDER_ID: &str = "oidc";

#[derive(Deserialize)]
pub struct OidcLoginParams {
    /// Optional path within the Bichon WebUI to return to after login.
    #[serde(default)]
    pub redirect_to: Option<String>,
}

/// Kick off the OIDC login: build the authorize URL, persist state+PKCE+nonce,
/// then 302 the browser to the IdP.
#[handler]
pub async fn oidc_login(Query(params): Query<OidcLoginParams>) -> Result<Response> {
    if !SETTINGS.bichon_oidc_enabled {
        return Ok(Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("OIDC single sign-on is not enabled on this server"));
    }

    match begin_login(sanitize_redirect(params.redirect_to)).await {
        Ok(url) => Ok(Redirect::temporary(url).into_response()),
        Err(e) => {
            error!("Failed to begin OIDC login: {}", e);
            // Do not echo internal error details back to the browser — they
            // may contain configuration hints useful to an attacker.
            Ok(Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to start OIDC login. Check server logs for details."))
        }
    }
}

#[derive(Deserialize)]
pub struct OidcCallbackParams {
    pub state: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Handle the redirect back from the IdP: exchange the code, verify the ID
/// token, resolve/auto-provision the Bichon user, mint a WebUI access token
/// and hand it back to the SPA.
#[handler]
pub async fn oidc_callback(Query(params): Query<OidcCallbackParams>) -> Result<Response> {
    if !SETTINGS.bichon_oidc_enabled {
        return Ok(Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("OIDC single sign-on is not enabled on this server"));
    }

    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        warn!("OIDC provider returned error '{}': {}", err, desc);
        // The full error text is user-controlled by the IdP — surface a
        // short generic message and hand the operator the details via logs.
        return Ok(redirect_login_error(
            "OIDC provider rejected the login. See server logs.",
        ));
    }

    let (state, code) = match (params.state, params.code) {
        (Some(s), Some(c)) => (s, c),
        _ => {
            return Ok(redirect_login_error(
                "OIDC callback missing required 'state' or 'code' parameter",
            ));
        }
    };

    let result = match complete_login(&state, &code).await {
        Ok(r) => r,
        Err(e) => {
            error!("OIDC token exchange or verification failed: {}", e);
            return Ok(redirect_login_error(
                "OIDC login failed. See server logs for details.",
            ));
        }
    };

    let claims = result.claims;

    let email = match claims.email.as_deref() {
        Some(e) if !e.is_empty() => e,
        _ => {
            return Ok(redirect_login_error(
                "OIDC provider did not return an email claim; cannot log in",
            ));
        }
    };

    let display_name = claims
        .preferred_username
        .as_deref()
        .or(claims.name.as_deref())
        .or(claims.given_name.as_deref());

    let user = match UserModel::find_or_provision_sso_user(
        OIDC_PROVIDER_ID,
        &claims.sub,
        email,
        display_name,
        SETTINGS.bichon_oidc_default_role_id,
    ) {
        Ok(u) => u,
        Err(e) => {
            error!("Failed to resolve OIDC user: {}", e);
            return Ok(redirect_login_error(
                "Failed to resolve Bichon user for OIDC subject. See server logs.",
            ));
        }
    };

    // Mint a fresh WebUI token for this session and persist it directly so we
    // do not clobber a concurrent password login for the same user.
    let token = AccessTokenModel::new_webui_token(user.id);
    let token_str = token.token.clone();
    if let Err(e) = insert_impl(DB_MANAGER.db(), token) {
        error!("Failed to persist OIDC-issued WebUI token: {}", e);
        return Ok(redirect_login_error(
            "Failed to issue session token. See server logs.",
        ));
    }

    // Do NOT ship the access token as a URL query parameter — it would leak
    // into browser history, Referer headers and server access logs. Instead
    // stash it in a single-use, short-lived server-side handoff entry and
    // hand the SPA only the handoff id.
    let redirect_to = result
        .redirect_after_login
        .as_deref()
        .filter(|s| s.starts_with('/'))
        .map(|s| s.to_string());
    let handoff = match OidcHandoffEntity::create(token_str, redirect_to) {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to create OIDC handoff entry: {}", e);
            return Ok(redirect_login_error(
                "Failed to hand off session. See server logs.",
            ));
        }
    };

    let base = SETTINGS.bichon_base_url.trim_end_matches('/');
    let target = format!(
        "{}/sso-callback?handoff={}",
        base,
        urlencoding::encode(&handoff.id),
    );

    Ok(Redirect::temporary(target).into_response())
}

#[derive(Deserialize)]
pub struct OidcHandoffRequest {
    pub handoff: String,
}

#[derive(Serialize)]
pub struct OidcHandoffResponse {
    pub access_token: String,
    pub redirect_to: String,
}

/// Consume a one-shot OIDC handoff id and return the freshly minted WebUI
/// access token in the JSON body. Deletes the handoff entry on success and on
/// expiry so it can never be replayed.
#[handler]
pub async fn oidc_handoff(Json(payload): Json<OidcHandoffRequest>) -> Result<Response> {
    if !SETTINGS.bichon_oidc_enabled {
        return Ok(Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body("OIDC single sign-on is not enabled on this server"));
    }

    if payload.handoff.is_empty() {
        return Ok(Response::builder()
            .status(http::StatusCode::BAD_REQUEST)
            .body("Missing handoff id"));
    }

    match OidcHandoffEntity::consume(&payload.handoff) {
        Ok(Some(entry)) => {
            let body = OidcHandoffResponse {
                access_token: entry.access_token,
                redirect_to: entry.redirect_after_login.unwrap_or_else(|| "/".to_string()),
            };
            match serde_json::to_string(&body) {
                Ok(json) => Ok(Response::builder()
                    .status(http::StatusCode::OK)
                    .content_type("application/json")
                    .header("Cache-Control", "no-store")
                    .header("Pragma", "no-cache")
                    .body(json)),
                Err(e) => {
                    error!("Failed to serialize handoff response: {}", e);
                    Ok(Response::builder()
                        .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                        .body("Internal error"))
                }
            }
        }
        Ok(None) => Ok(Response::builder()
            .status(http::StatusCode::UNAUTHORIZED)
            .body("SSO handoff not found, expired, or already used")),
        Err(e) => {
            error!("Failed to consume OIDC handoff: {}", e);
            Ok(Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Failed to consume handoff"))
        }
    }
}

fn sanitize_redirect(input: Option<String>) -> Option<String> {
    input.and_then(|s| {
        if s.starts_with('/') && !s.starts_with("//") {
            Some(s)
        } else {
            None
        }
    })
}

fn redirect_login_error(message: &str) -> Response {
    let base = SETTINGS.bichon_base_url.trim_end_matches('/');
    let target = format!(
        "{}/sign-in?sso_error={}",
        base,
        urlencoding::encode(message)
    );
    Redirect::temporary(target).into_response()
}
