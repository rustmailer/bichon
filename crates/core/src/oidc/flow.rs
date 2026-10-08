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

use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::oidc::discovery::{get_discovery, OidcDiscovery};
use crate::oidc::id_token::{peek_header, verify_and_parse, IdTokenClaims, VerifyParams};
use crate::oidc::jwks::find_key;
use crate::oidc::pending::OidcPendingEntity;
use crate::raise_error;
use crate::settings::cli::SETTINGS;
use base64::Engine as _;
use oauth2::{
    basic::{BasicErrorResponseType, BasicTokenType},
    AuthUrl, AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, ExtraTokenFields, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    StandardErrorResponse, StandardRevocableToken, StandardTokenIntrospectionResponse,
    StandardTokenResponse, TokenUrl,
};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

fn generate_nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcExtraFields {
    #[serde(default)]
    pub id_token: Option<String>,
}

impl ExtraTokenFields for OidcExtraFields {}

type OidcTokenResponse = StandardTokenResponse<OidcExtraFields, BasicTokenType>;
type OidcErrorResponse = StandardErrorResponse<BasicErrorResponseType>;

pub struct OidcConfig<'a> {
    pub issuer_url: &'a str,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub redirect_uri: &'a str,
}

/// Extract OIDC config from SETTINGS; returns an error if OIDC is not fully configured.
pub fn load_oidc_config() -> BichonResult<(String, String, String, String)> {
    if !SETTINGS.bichon_oidc_enabled {
        return Err(raise_error!(
            "OIDC single sign-on is not enabled on this server".into(),
            ErrorCode::PermissionDenied
        ));
    }
    let issuer = SETTINGS
        .bichon_oidc_issuer_url
        .clone()
        .ok_or_else(|| missing("bichon_oidc_issuer_url"))?;
    let client_id = SETTINGS
        .bichon_oidc_client_id
        .clone()
        .ok_or_else(|| missing("bichon_oidc_client_id"))?;
    let client_secret = SETTINGS
        .bichon_oidc_client_secret
        .clone()
        .ok_or_else(|| missing("bichon_oidc_client_secret"))?;
    let redirect_uri = SETTINGS
        .bichon_oidc_redirect_uri
        .clone()
        .ok_or_else(|| missing("bichon_oidc_redirect_uri"))?;
    Ok((issuer, client_id, client_secret, redirect_uri))
}

fn missing(field: &str) -> crate::error::BichonError {
    raise_error!(
        format!("OIDC is enabled but '{}' is not configured", field),
        ErrorCode::InvalidParameter
    )
}

type OidcClient = Client<
    OidcErrorResponse,
    OidcTokenResponse,
    StandardTokenIntrospectionResponse<OidcExtraFields, BasicTokenType>,
    StandardRevocableToken,
    StandardErrorResponse<oauth2::RevocationErrorResponseType>,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

fn build_client(cfg: &OidcConfig<'_>, discovery: &OidcDiscovery) -> BichonResult<OidcClient> {
    let auth_url = AuthUrl::new(discovery.authorization_endpoint.clone()).map_err(|e| {
        raise_error!(
            format!("Invalid authorization_endpoint in discovery: {}", e),
            ErrorCode::InvalidParameter
        )
    })?;
    let token_url = TokenUrl::new(discovery.token_endpoint.clone()).map_err(|e| {
        raise_error!(
            format!("Invalid token_endpoint in discovery: {}", e),
            ErrorCode::InvalidParameter
        )
    })?;
    let redirect = RedirectUrl::new(cfg.redirect_uri.to_string()).map_err(|e| {
        raise_error!(
            format!("Invalid OIDC redirect_uri configured: {}", e),
            ErrorCode::InvalidParameter
        )
    })?;

    let client: OidcClient = Client::new(ClientId::new(cfg.client_id.to_string()))
        .set_client_secret(ClientSecret::new(cfg.client_secret.to_string()))
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect);

    Ok(client)
}

fn build_http_client() -> BichonResult<reqwest::Client> {
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| {
            raise_error!(
                format!("Failed to build HTTP client for OIDC: {}", e),
                ErrorCode::InternalError
            )
        })
}

/// Build the authorization URL and persist the pending state.
/// Returns the URL the browser must be redirected to.
pub async fn begin_login(redirect_after_login: Option<String>) -> BichonResult<String> {
    let (issuer, client_id, client_secret, redirect_uri) = load_oidc_config()?;
    let cfg = OidcConfig {
        issuer_url: &issuer,
        client_id: &client_id,
        client_secret: &client_secret,
        redirect_uri: &redirect_uri,
    };

    let discovery = get_discovery(cfg.issuer_url).await?;
    let client = build_client(&cfg, &discovery)?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let nonce = generate_nonce();
    let request = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .add_extra_param("nonce", nonce.clone());

    let (authorize_url, csrf_state) = request.url();

    let pending = OidcPendingEntity::new(
        csrf_state.secret().to_string(),
        pkce_verifier.secret().to_string(),
        nonce,
        redirect_after_login,
    );
    pending.save()?;

    // The state value is a CSRF secret; do not log it.
    debug!(redirect_uri = cfg.redirect_uri, "OIDC login initiated");

    Ok(authorize_url.to_string())
}

/// Result of a successful OIDC callback: verified claims + the state entry
/// (so the caller can decide where to redirect the user next).
pub struct OidcCallbackResult {
    pub claims: IdTokenClaims,
    pub redirect_after_login: Option<String>,
}

/// Complete the OIDC login: exchange the code, verify the ID token, and return claims.
pub async fn complete_login(state: &str, code: &str) -> BichonResult<OidcCallbackResult> {
    let (issuer, client_id, client_secret, redirect_uri) = load_oidc_config()?;
    let cfg = OidcConfig {
        issuer_url: &issuer,
        client_id: &client_id,
        client_secret: &client_secret,
        redirect_uri: &redirect_uri,
    };

    let pending = OidcPendingEntity::get(state)?.ok_or_else(|| {
        raise_error!(
            "OIDC state parameter is invalid or expired".into(),
            ErrorCode::PermissionDenied
        )
    })?;

    let discovery = get_discovery(cfg.issuer_url).await?;
    let client = build_client(&cfg, &discovery)?;
    let http = build_http_client()?;

    // Deliberately format the token-exchange error with `Display`, not
    // `Debug`: some oauth2 crate errors embed the raw HTTP response body,
    // which may in edge cases echo back headers the request sent
    // (including the client secret when using client_secret_basic).
    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(pending.code_verifier.clone()))
        .request_async(&http)
        .await
        .map_err(|e| {
            raise_error!(
                format!("OIDC token exchange failed: {}", e),
                ErrorCode::HttpResponseError
            )
        })?;

    let id_token_str = token_response
        .extra_fields()
        .id_token
        .as_deref()
        .ok_or_else(|| {
            raise_error!(
                "OIDC token response did not contain an id_token".into(),
                ErrorCode::HttpResponseError
            )
        })?;

    // Authelia and most IdPs sign ID tokens asymmetrically (RS256/ES256);
    // HS256 is only accepted with the client secret.
    let header = peek_header(id_token_str)?;
    let key = if header.alg == "HS256" {
        None
    } else {
        Some(find_key(&discovery.jwks_uri, header.kid.as_deref(), &header.alg).await?)
    };
    let now_secs = chrono::Utc::now().timestamp();
    let claims = verify_and_parse(
        id_token_str,
        &VerifyParams {
            expected_issuer: &discovery.issuer,
            expected_audience: cfg.client_id,
            expected_nonce: &pending.nonce,
            client_secret: cfg.client_secret.as_bytes(),
            key: key.as_ref(),
            clock_skew_secs: 60,
            now_secs,
        },
    )?;

    // Best-effort cleanup; failure to delete the pending entry must not abort login.
    if let Err(e) = OidcPendingEntity::delete(state) {
        tracing::warn!(error = %e, "Failed to delete pending OIDC state");
    }

    Ok(OidcCallbackResult {
        claims,
        redirect_after_login: pending.redirect_after_login,
    })
}
