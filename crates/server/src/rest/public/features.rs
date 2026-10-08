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

use bichon_core::settings::cli::SETTINGS;
use poem::{handler, web::Json, IntoResponse};
use serde::Serialize;

#[derive(Serialize)]
struct FeaturesResponse {
    features: Vec<String>,
    edition: &'static str,
    version: String,
    oidc_enabled: bool,
    oidc_auto_redirect: bool,
    sso_enabled: bool,
    ldap_enabled: bool,
    siem_enabled: bool,
}

#[handler]
pub async fn get_features() -> impl IntoResponse {
    let oidc_enabled = SETTINGS.bichon_oidc_enabled
        && SETTINGS.bichon_oidc_issuer_url.is_some()
        && SETTINGS.bichon_oidc_client_id.is_some()
        && SETTINGS.bichon_oidc_client_secret.is_some()
        && SETTINGS.bichon_oidc_redirect_uri.is_some();

    let mut features = Vec::new();
    if oidc_enabled {
        features.push("oidc".to_string());
    }

    Json(FeaturesResponse {
        features,
        edition: "community",
        version: env!("CARGO_PKG_VERSION").to_string(),
        oidc_enabled,
        oidc_auto_redirect: oidc_enabled && SETTINGS.bichon_oidc_auto_redirect,
        // Community edition: Pro-only flags default off. Kept in the
        // response so upstream frontend code reading sso/ldap/siem keeps
        // working after the merge.
        sso_enabled: oidc_enabled,
        ldap_enabled: false,
        siem_enabled: false,
    })
}
