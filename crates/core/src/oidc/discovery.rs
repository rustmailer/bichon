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
use crate::raise_error;
use serde::Deserialize;
use std::sync::RwLock;
use std::time::{Duration, Instant};

const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(3600);
const DISCOVERY_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Deserialize)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: Option<String>,
    pub jwks_uri: String,
    #[serde(default)]
    pub id_token_signing_alg_values_supported: Vec<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

struct CachedDiscovery {
    doc: OidcDiscovery,
    fetched_at: Instant,
    issuer_url: String,
}

static DISCOVERY_CACHE: RwLock<Option<CachedDiscovery>> = RwLock::new(None);

fn build_discovery_url(issuer_url: &str) -> String {
    let base = issuer_url.trim_end_matches('/');
    format!("{}/.well-known/openid-configuration", base)
}

pub async fn get_discovery(issuer_url: &str) -> BichonResult<OidcDiscovery> {
    if let Ok(guard) = DISCOVERY_CACHE.read() {
        if let Some(cached) = guard.as_ref() {
            if cached.issuer_url == issuer_url && cached.fetched_at.elapsed() < DISCOVERY_CACHE_TTL
            {
                return Ok(cached.doc.clone());
            }
        }
    }

    let url = build_discovery_url(issuer_url);
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_HTTP_TIMEOUT)
        .build()
        .map_err(|e| {
            raise_error!(
                format!("Failed to build HTTP client for OIDC discovery: {}", e),
                ErrorCode::InternalError
            )
        })?;

    let resp = client.get(&url).send().await.map_err(|e| {
        raise_error!(
            format!("OIDC discovery request to {} failed: {}", url, e),
            ErrorCode::HttpResponseError
        )
    })?;

    if !resp.status().is_success() {
        return Err(raise_error!(
            format!(
                "OIDC discovery returned non-success status {} for {}",
                resp.status(),
                url
            ),
            ErrorCode::HttpResponseError
        ));
    }

    let doc: OidcDiscovery = resp.json().await.map_err(|e| {
        raise_error!(
            format!("Failed to parse OIDC discovery document: {}", e),
            ErrorCode::HttpResponseError
        )
    })?;

    if let Ok(mut guard) = DISCOVERY_CACHE.write() {
        *guard = Some(CachedDiscovery {
            doc: doc.clone(),
            fetched_at: Instant::now(),
            issuer_url: issuer_url.to_string(),
        });
    }

    Ok(doc)
}
