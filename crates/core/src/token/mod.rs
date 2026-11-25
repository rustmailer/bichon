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

use bichon_memdb::MemDb;

use crate::database::manager::DB_MANAGER;
use crate::database::{
    delete_impl, filter_impl, find_impl, insert_impl, list_all_impl, update_impl,
    with_transaction_exempt, MemDbModel,
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

pub mod migrate;
pub mod payload;
pub mod view;

/// How [`AccessTokenModel::token_material`] is encoded.
///
/// Records written before token hashing deserialize as `Plaintext`; their
/// `token_material` is the live bearer token. `startup` migration rewrites
/// them so that only a digest remains on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenScheme {
    #[default]
    Plaintext,
    Blake3,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum TokenType {
    WebUI,
    Api,
    /// Short-lived, one-time challenge issued for the MFA login step.
    MfaChallenge,
    /// Short-lived, one-time ticket issued when a password login must change
    /// the password before any session is granted. It only unlocks the
    /// change-password endpoint and is never a valid access token.
    PasswordChange,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccessTokenModel {
    /// The ID of the user who owns this token
    pub user_id: u64,
    /// The bearer token itself before migration, and its BLAKE3 digest after.
    ///
    /// The serialized field name stays `token` so existing memdb snapshots
    /// (and every older backup) keep deserializing; `token_scheme` says which
    /// of the two forms this value holds. Once migrated the plaintext is gone
    /// and the token cannot be displayed again.
    #[serde(rename = "token")]
    pub token_material: String,
    /// Whether [`Self::token_material`] is plaintext (legacy) or a digest.
    #[serde(default)]
    pub token_scheme: TokenScheme,
    /// First characters of the plaintext token, kept for identification in
    /// lists after the digest replaced the full value.
    #[serde(default)]
    pub token_prefix: String,
    /// Non-secret identifier used by revocation endpoints. Derived from the
    /// digest, so it is stable and does not reveal the token.
    #[serde(default)]
    pub token_id: String,
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
        self.token_material.clone()
    }
}

/// Domain-separated digest of a bearer token. The token space is 128 random
/// bits, so a single fast hash is enough; a slow KDF would only tax every
/// authenticated request.
pub(crate) fn digest_token(plaintext: &str) -> String {
    blake3::hash(format!("bichon-access-token:v1:{plaintext}").as_bytes())
        .to_hex()
        .to_string()
}

/// First characters of the plaintext, kept so the UI can tell tokens apart
/// after the full value is no longer stored.
pub(crate) fn token_prefix(plaintext: &str) -> String {
    plaintext.chars().take(12).collect()
}

/// Public, non-secret identifier derived from the digest. Stable across
/// restarts and across eager/lazy migration.
pub(crate) fn token_id_from_digest(digest: &str) -> String {
    digest.chars().take(24).collect()
}

/// How often "last used" is persisted. Every authenticated request refreshes
/// it in the model, but writing to memdb each time turns read-only polling
/// into disk I/O; a minute of drift is invisible in the UI and harmless for
/// the WebUI idle timeout.
const LAST_ACCESS_REFRESH_MS: i64 = 60_000;

pub(crate) fn should_refresh_last_access(last_access_at: i64, now: i64) -> bool {
    last_access_at <= 0 || now.saturating_sub(last_access_at) >= LAST_ACCESS_REFRESH_MS
}

impl AccessTokenModel {
    /// Build a record whose stored material is the digest, never the token.
    fn from_plaintext(
        plaintext: &str,
        user_id: u64,
        name: Option<String>,
        token_type: TokenType,
        expire_at: Option<i64>,
        metrics_only: bool,
    ) -> Self {
        let now = utc_now!();
        let digest = digest_token(plaintext);
        Self {
            token_id: token_id_from_digest(&digest),
            token_prefix: token_prefix(plaintext),
            token_material: digest,
            token_scheme: TokenScheme::Blake3,
            created_at: now,
            updated_at: now,
            last_access_at: Default::default(),
            name,
            user_id,
            token_type,
            expire_at,
            metrics_only,
        }
    }

    pub fn new_api_token(
        token: String,
        user_id: u64,
        name: Option<String>,
        expire_at: Option<i64>,
        metrics_only: bool,
    ) -> Self {
        Self::from_plaintext(&token, user_id, name, TokenType::Api, expire_at, metrics_only)
    }

    /// Returns the record and the plaintext token. The plaintext is only ever
    /// returned to the caller that created it.
    pub fn new_webui_token(user_id: u64) -> (AccessTokenModel, String) {
        let plaintext = generate_token!(128);
        let model = Self::from_plaintext(
            &plaintext,
            user_id,
            None,
            TokenType::WebUI,
            None,
            false,
        );
        (model, plaintext)
    }

    pub fn reset_webui_token(user_id: u64) -> BichonResult<String> {
        let old_token = Self::get_user_webui_token(user_id)?;
        let (new_token, plaintext) = Self::new_webui_token(user_id);

        match old_token {
            Some(old) => {
                // Tokens are gate-exempt (memdb-only writes); the exempt
                // transaction keeps login working during a backup window.
                with_transaction_exempt(DB_MANAGER.db(), move |txn| {
                    let txn = txn.delete(AccessTokenModel::collection(), old.key());
                    txn.insert(AccessTokenModel::collection(), new_token.key(), &new_token)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
                })?;
            }
            None => {
                insert_impl(DB_MANAGER.db(), new_token)?;
            }
        }

        Ok(plaintext)
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
                let _ = delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &stale_token.key());
            }
        }

        let plaintext = generate_token!(128);
        let token = AccessTokenModel::from_plaintext(
            &plaintext,
            user_id,
            None,
            TokenType::MfaChallenge,
            Some(expire_at),
            false,
        );
        insert_impl(DB_MANAGER.db(), token)?;
        Ok(plaintext)
    }

    /// Issue a short-lived, one-time ticket that only allows setting a new
    /// password. Used when a password login must change the password before
    /// any session is granted. Expires after 10 minutes and is consumed by the
    /// change-password endpoint.
    pub fn new_password_change_ticket(user_id: u64) -> BichonResult<String> {
        let now = utc_now!();
        let expire_at = now + 10 * 60 * 1000;

        // Garbage-collect this user's expired tickets to avoid unbounded growth.
        let stale = filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| {
            t.user_id == user_id && t.token_type == TokenType::PasswordChange
        })?;
        for stale_token in stale {
            if stale_token.expire_at.map_or(false, |e| e < now) {
                let _ = delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &stale_token.key());
            }
        }

        let plaintext = generate_token!(128);
        let token = AccessTokenModel::from_plaintext(
            &plaintext,
            user_id,
            Some("Password Change".into()),
            TokenType::PasswordChange,
            Some(expire_at),
            false,
        );
        insert_impl(DB_MANAGER.db(), token)?;
        Ok(plaintext)
    }

    /// Validate and consume a password-change ticket, returning the owning
    /// user id. The ticket is deleted whether or not it has expired so a
    /// replayed value cannot be retried.
    pub fn consume_password_change_ticket(ticket: &str) -> BichonResult<u64> {
        let invalid = || {
            raise_error!(
                "Invalid or expired password change ticket.".into(),
                ErrorCode::PermissionDenied
            )
        };

        let token = Self::find_by_secret(ticket)?.ok_or_else(invalid)?;
        if !matches!(token.token_type, TokenType::PasswordChange) {
            return Err(invalid());
        }
        // One-time: remove it before doing any other work.
        delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &token.key())?;

        if let Some(expire_at) = token.expire_at {
            if utc_now!() > expire_at {
                return Err(invalid());
            }
        }
        Ok(token.user_id)
    }

    pub fn get_user_webui_token(user_id: u64) -> BichonResult<Option<AccessTokenModel>> {
        let tokens =
            filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| t.user_id == user_id)?;
        Ok(tokens
            .into_iter()
            .find(|t| t.token_type == TokenType::WebUI))
    }

    /// Delete every WebUI session token owned by the user. Used when an
    /// administrator changes someone else's password (the account must sign
    /// in again) and at startup when the default admin still uses the
    /// well-known initial password. All rows are removed, not just the first:
    /// older installs can carry more than one WebUI token.
    pub fn revoke_user_webui_tokens(user_id: u64) -> BichonResult<()> {
        let tokens = filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| {
            t.user_id == user_id && t.token_type == TokenType::WebUI
        })?;
        for token in tokens {
            delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &token.key())?;
        }
        Ok(())
    }

    /// API tokens owned by `user_id`, in the masked shape the WebUI renders.
    /// Never returns token material — only the prefix and the public id.
    pub fn get_user_api_tokens(user_id: u64) -> BichonResult<Vec<AccessTokenResp>> {
        let user = UserModel::find(user_id)?;
        let tokens =
            filter_impl::<AccessTokenModel, _>(DB_MANAGER.db(), move |t| t.user_id == user_id)?;
        Ok(tokens
            .into_iter()
            .filter(|t| t.token_type == TokenType::Api)
            .map(|t| AccessTokenResp::from_model(&t, user.as_ref()))
            .collect())
    }

    /// Look a token up by the secret the client presented.
    ///
    /// Hashed records are found directly. A legacy record still keyed by its
    /// plaintext is migrated on the spot, so an upgrade does not invalidate
    /// credentials that were issued before it.
    pub(crate) fn find_by_secret_in(
        db: &MemDb,
        presented: &str,
    ) -> BichonResult<Option<AccessTokenModel>> {
        let digest = digest_token(presented);
        if let Some(model) = find_impl::<AccessTokenModel>(db, &digest)? {
            return Ok(Some(model));
        }

        // Legacy path: the collection key is the plaintext token. Rewrite the
        // record so this is the last time the plaintext is read.
        //
        // Only a plaintext record may match here. A hashed record's key is
        // its digest, and accepting a raw key hit would let anyone who saw
        // the database authenticate with the digest itself.
        if let Some(model) = find_impl::<AccessTokenModel>(db, presented)? {
            if model.token_scheme == TokenScheme::Plaintext
                && model.token_material == presented
            {
                let migrated = migrate::migrate_record_in(db, &model)?;
                return Ok(Some(migrated));
            }
        }

        Ok(None)
    }

    pub fn find_by_secret(presented: &str) -> BichonResult<Option<AccessTokenModel>> {
        Self::find_by_secret_in(DB_MANAGER.db(), presented)
    }

    /// Public id lookup used by the revocation endpoint.
    pub fn find_by_id(token_id: &str) -> BichonResult<Option<AccessTokenModel>> {
        if token_id.is_empty() {
            return Ok(None);
        }
        Ok(list_all_impl::<AccessTokenModel>(DB_MANAGER.db())?
            .into_iter()
            .find(|t| t.token_id == token_id))
    }

    /// Validate lifecycle (type and expiry) and refresh the last-access
    /// timestamp. Returns the stored token record.
    fn resolve_token(token: &str) -> BichonResult<AccessTokenModel> {
        let token_model = Self::find_by_secret(token)?.ok_or_else(|| {
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

        if matches!(token_model.token_type, TokenType::PasswordChange) {
            return Err(raise_error!(
                "Permission denied: password change tickets cannot be used as access tokens."
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

        // Refreshing `last_access_at` is a memdb write (and a WAL append).
        // Doing it on every request turns read-only polling into disk writes;
        // minute precision is plenty for both the WebUI idle timeout and the
        // "last used" column.
        let now = utc_now!();
        if should_refresh_last_access(token_model.last_access_at, now) {
            let key = token_model.key();
            update_impl(DB_MANAGER.db(), &key, move |current: AccessTokenModel| {
                let mut updated = current.clone();
                updated.last_access_at = now;
                Ok(updated)
            })?;
        }

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

    /// Delete by any identifier a caller may hold: the presented secret, the
    /// public token id, or the stored material (legacy plaintext or digest).
    /// Kept permissive so old callers and old WebUI builds keep working.
    pub fn delete(identifier: &str) -> BichonResult<()> {
        if let Some(model) = Self::find_by_secret(identifier)? {
            return delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &model.key());
        }
        if let Some(model) = Self::find_by_id(identifier)? {
            return delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &model.key());
        }
        // Already a stored key (hash or legacy plaintext).
        delete_impl::<AccessTokenModel>(DB_MANAGER.db(), identifier)
    }

    /// Look up by a secret or a public token id. The revocation endpoint uses
    /// the same function for both, so a stale client that still holds the old
    /// full-token URL keeps working after the upgrade.
    pub fn get_token(identifier: &str) -> BichonResult<AccessTokenModel> {
        if let Some(model) = Self::find_by_secret(identifier)? {
            return Ok(model);
        }
        if let Some(model) = Self::find_by_id(identifier)? {
            return Ok(model);
        }
        Err(raise_error!(
            format!("Access token '{}' not found", identifier),
            ErrorCode::ResourceNotFound
        ))
    }

    pub fn list_all_api_tokens() -> BichonResult<Vec<AccessTokenResp>> {
        let users = UserModel::list_all()?;
        let all = list_all_impl::<AccessTokenModel>(DB_MANAGER.db())?;

        let user_map: HashMap<u64, UserModel> = users.into_iter().map(|u| (u.id, u)).collect();

        let resp = all
            .into_iter()
            .filter(|t| t.token_type == TokenType::Api)
            .map(|token| AccessTokenResp::from_model(&token, user_map.get(&token.user_id)))
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
