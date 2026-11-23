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

use std::collections::HashMap;

use crate::database::manager::DB_MANAGER;
use crate::database::{
    MemDbModel, delete_impl, filter_impl, find_impl, insert_impl, list_all_impl, update_impl,
    with_transaction_exempt
};
use crate::error::code::ErrorCode;
use crate::raise_error;
use crate::common::auth::ClientContext;
use crate::token::view::AccessTokenResp;
use crate::users::permissions::Permission;
use crate::users::UserModel;
use crate::{
    error::BichonResult, generate_token, token::payload::AccessTokenCreateRequest, utc_now,
};
//use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};

pub mod payload;
pub mod view;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum TokenType {
    WebUI,
    Api,
    /// Short-lived, one-time challenge issued for the MFA login step.
    MfaChallenge,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccessTokenModel {
    /// The ID of the user who owns this token
    pub user_id: u64,
    /// The unique token string used for authentication
    pub token: String,
    /// An optional name of the token.
    pub name: Option<String>,
    /// Token type: WebUI or API
    pub token_type: TokenType,
    /// The timestamp (in milliseconds since epoch) when the token was created.
    pub created_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token was last updated.
    pub updated_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token expires.
    /// None means the token does not expire (this applies only to API tokens).
    pub expire_at: Option<i64>,
    /// The timestamp (in milliseconds since epoch) when the token was last used.
    pub last_access_at: i64,
    /// When true, this API token may only be used on the metrics endpoint
    /// (`GET /api/v1/metrics`). It is otherwise a normal API token: it has
    /// the same expiry, revocation and last-access semantics, and the user
    /// behind it must still hold the `system:metrics` permission. Older
    /// records deserialize as `false` via `#[serde(default)]`.
    #[serde(default)]
    pub metrics_only: bool,
}

impl MemDbModel for AccessTokenModel {
    /// Tokens are ephemeral credentials that only ever touch memdb: they may
    /// be written during a backup capture window (see
    /// [`MemDbModel::BACKUP_GATE_EXEMPT`]) so login and logout keep working
    /// while a backup runs.
    const BACKUP_GATE_EXEMPT: bool = true;

    fn collection() -> &'static str {
        "tokens"
    }
    fn key(&self) -> String {
        self.token.clone()
    }
}

impl AccessTokenModel {
    pub fn new_api_token(
        token: String,
        user_id: u64,
        name: Option<String>,
        expire_at: Option<i64>,
        metrics_only: bool,
    ) -> Self {
        Self {
            token,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            last_access_at: Default::default(),
            name,
            user_id,
            token_type: TokenType::Api,
            expire_at,
            metrics_only,
        }
    }

    pub fn new_webui_token(user_id: u64) -> AccessTokenModel {
        let now = utc_now!();
        AccessTokenModel {
            token: generate_token!(128),
            created_at: now,
            updated_at: now,
            last_access_at: Default::default(),
            name: None,
            user_id,
            token_type: TokenType::WebUI,
            expire_at: None,
            metrics_only: false,
        }
    }

    pub fn reset_webui_token(user_id: u64) -> BichonResult<String> {
        let old_token = Self::get_user_webui_token(user_id)?;
        let new_token = Self::new_webui_token(user_id);
        let new_token_str = new_token.token.clone();

        match old_token {
            Some(old) => {
                // Tokens are gate-exempt (memdb-only writes); the exempt
                // transaction keeps login working during a backup window.
                with_transaction_exempt(DB_MANAGER.db(), move |txn| {
                    let txn = txn.delete(AccessTokenModel::collection(), old.token.clone());
                    txn.insert(AccessTokenModel::collection(), new_token.key(), &new_token)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
                })?;
            }
            None => {
                insert_impl(DB_MANAGER.db(), new_token)?;
            }
        }

        Ok(new_token_str)
    }

    /// Issue a short-lived, one-time challenge token for the MFA login step.
    /// Expires after 5 minutes and is consumed by the verify endpoint.
    pub fn new_mfa_challenge(user_id: u64) -> BichonResult<String> {
        let now = utc_now!();
        let expire_at = now + 5 * 60 * 1000;

        // Garbage-collect this user's expired challenges to avoid unbounded growth.
        let stale = filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| {
            t.user_id == user_id && t.token_type == TokenType::MfaChallenge
        })?;
        for stale_token in stale {
            if stale_token.expire_at.map_or(false, |e| e < now) {
                let _ = delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &stale_token.token);
            }
        }

        let token = AccessTokenModel {
            token: generate_token!(128),
            created_at: now,
            updated_at: now,
            last_access_at: 0,
            name: None,
            user_id,
            token_type: TokenType::MfaChallenge,
            expire_at: Some(expire_at),
            metrics_only: false,
        };
        let token_str = token.token.clone();
        insert_impl(DB_MANAGER.db(), token)?;
        Ok(token_str)
    }

    pub fn get_user_webui_token(user_id: u64) -> BichonResult<Option<AccessTokenModel>> {
        let tokens =
            filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| t.user_id == user_id)?;
        Ok(tokens
            .into_iter()
            .find(|t| t.token_type == TokenType::WebUI))
    }

    pub fn get_user_api_tokens(user_id: u64) -> BichonResult<Vec<AccessTokenModel>> {
        let tokens =
            filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| t.user_id == user_id)?;
        Ok(tokens
            .into_iter()
            .filter(|t| t.token_type == TokenType::Api)
            .collect())
    }

    /// Validate a token's lifecycle (type and expiry) and refresh its
    /// last-access timestamp. Returns the stored token record.
    fn resolve_token(token: &str) -> BichonResult<AccessTokenModel> {
        let token_str = token.to_string();
        let token_model = find_impl::<AccessTokenModel>(DB_MANAGER.db(), &token_str)?
            .ok_or_else(|| {
                raise_error!(
                    "Invalid access token provided. Please check your credentials.".into(),
                    ErrorCode::PermissionDenied
                )
            })?;

        if matches!(token_model.token_type, TokenType::MfaChallenge) {
            return Err(raise_error!(
                "Permission denied: MFA challenge tokens cannot be used as access tokens."
                    .into(),
                ErrorCode::PermissionDenied
            ));
        }

        if matches!(token_model.token_type, TokenType::WebUI) {
            // Use last_access_at if set, otherwise fall back to created_at
            let last_active = if token_model.last_access_at > 0 {
                token_model.last_access_at
            } else {
                token_model.created_at
            };
            let idle = utc_now!() - last_active;
            let max_life =
                crate::settings::runtime::webui_token_expiration_hours() as i64 * 60 * 60 * 1000;

            if idle > max_life {
                return Err(raise_error!(
                    "Permission denied: the WebUI token has expired.".into(),
                    ErrorCode::PermissionDenied
                ));
            }
        }

        if matches!(token_model.token_type, TokenType::Api) {
            if let Some(expire_at) = token_model.expire_at {
                if utc_now!() > expire_at {
                    return Err(raise_error!(
                        "Your API token has expired and is no longer valid.".into(),
                        ErrorCode::PermissionDenied
                    ));
                }
            }
        }

        // Update last_access_at on every successful use for both token types
        update_impl(DB_MANAGER.db(), &token_str, |current: AccessTokenModel| {
            let mut updated = current.clone();
            updated.last_access_at = utc_now!();
            Ok(updated)
        })?;

        Ok(token_model)
    }

    fn user_for_token(token_model: &AccessTokenModel) -> BichonResult<UserModel> {
        UserModel::find(token_model.user_id)?
            .ok_or_else(|| raise_error!("The user associated with this access token does not exist or may have been deleted.".into(), ErrorCode::ResourceNotFound))
    }

    /// Resolve a user from a general-purpose access token.
    ///
    /// Metrics-only tokens are rejected here: every caller of this function
    /// (SMTP authentication, login helpers, the REST guard on non-metrics
    /// routes) must not accept a credential that is scoped to the metrics
    /// endpoint. The metrics endpoint uses
    /// [`Self::resolve_user_from_token_for_metrics`] instead.
    pub fn resolve_user_from_token(token: &str) -> BichonResult<UserModel> {
        let token_model = Self::resolve_token(token)?;
        if token_model.metrics_only {
            return Err(raise_error!(
                "Permission denied: this token is restricted to the metrics endpoint."
                    .into(),
                ErrorCode::Forbidden
            ));
        }
        Self::user_for_token(&token_model)
    }

    /// Resolve a user from a token presented on the metrics endpoint.
    ///
    /// Accepts both normal tokens and metrics-only tokens. The caller must
    /// still check the user's `system:metrics` permission — the token scope
    /// only decides *where* it may be used, not *what* it may read.
    pub fn resolve_user_from_token_for_metrics(token: &str) -> BichonResult<UserModel> {
        let token_model = Self::resolve_token(token)?;
        Self::user_for_token(&token_model)
    }

    pub fn create_api_token(
        user_id: u64,
        request: AccessTokenCreateRequest,
    ) -> BichonResult<String> {
        // Validate request parameters first
        request.validate()?;

        // A metrics-only token is useless — and misleading — for a user who
        // cannot read metrics. Resolve the owner's effective permissions now
        // so the failure is reported where the token is created. The metrics
        // endpoint re-checks at use time as well.
        if request.metrics_only {
            let user = UserModel::find(user_id)?.ok_or_else(|| {
                raise_error!(
                    format!("Cannot create a token for user {}: user not found", user_id),
                    ErrorCode::ResourceNotFound
                )
            })?;
            if !ClientContext::check_has_permission(&user, None, Permission::SYSTEM_METRICS) {
                return Err(raise_error!(
                    "Cannot create a metrics-only token for a user without the \
                     system:metrics permission."
                        .into(),
                    ErrorCode::Forbidden
                ));
            }
        }

        let expire_at = request
            .expire_in
            .map(|hours| utc_now!() + (hours as i64) * 60 * 60 * 1000);
        let token = generate_token!(128);
        let access_token = AccessTokenModel::new_api_token(
            token.clone(),
            user_id,
            request.name,
            expire_at,
            request.metrics_only,
        );
        insert_impl(DB_MANAGER.db(), access_token)?;
        Ok(token)
    }

    pub fn delete(token: &str) -> BichonResult<()> {
        delete_impl::<AccessTokenModel>(DB_MANAGER.db(), token)
    }

    pub fn get_token(token: &str) -> BichonResult<AccessTokenModel> {
        find_impl::<AccessTokenModel>(DB_MANAGER.db(), token)?.ok_or_else(|| {
            raise_error!(
                format!("Access token '{}' not found", token),
                ErrorCode::ResourceNotFound
            )
        })
    }

    pub fn list_all_api_tokens() -> BichonResult<Vec<AccessTokenResp>> {
        let users = UserModel::list_all()?;
        let all = list_all_impl::<AccessTokenModel>(DB_MANAGER.db())?;

        let user_map: HashMap<u64, UserModel> = users.into_iter().map(|u| (u.id, u)).collect();

        let resp = all
            .into_iter()
            .filter(|t| t.token_type == TokenType::Api)
            .map(|token| {
                let user = user_map.get(&token.user_id);
                AccessTokenResp {
                    user_name: user
                        .map(|u| u.username.clone())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    user_email: user
                        .map(|u| u.email.clone())
                        .unwrap_or_else(|| "N/A".to_string()),
                    user_id: token.user_id,
                    name: token.name,
                    token: token.token,
                    token_type: token.token_type,
                    created_at: token.created_at,
                    updated_at: token.updated_at,
                    expire_at: token.expire_at,
                    last_access_at: token.last_access_at,
                    metrics_only: token.metrics_only,
                }
            })
            .collect();

        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::AccessTokenModel;

    /// Tokens written before the metrics-only scope existed carry no
    /// `metrics_only` field and must still deserialize — as unrestricted
    /// tokens, preserving their previous behavior.
    #[test]
    fn legacy_token_records_default_to_not_metrics_only() {
        let json = r#"{
            "user_id": 1,
            "token": "legacy-token",
            "name": null,
            "token_type": "Api",
            "created_at": 0,
            "updated_at": 0,
            "expire_at": null,
            "last_access_at": 0
        }"#;
        let model: AccessTokenModel = serde_json::from_str(json).unwrap();
        assert!(!model.metrics_only);
    }
}
