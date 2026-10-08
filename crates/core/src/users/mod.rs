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

use crate::{
    database::{
        delete_impl, filter_impl, find_impl, list_all_impl, manager::DB_MANAGER, update_impl,
        with_transaction, MemDbModel,
    },
    decrypt,
    error::{code::ErrorCode, BichonResult},
    generate_token, id, raise_error,
    token::{AccessTokenModel, TokenType},
    users::{
        acl::AccessControl,
        payload::{UserCreateRequest, UserUpdateRequest},
        permissions::Permission,
        role::{UserRole, DEFAULT_ADMIN_ROLE_ID},
        view::UserView,
    },
    utc_now,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use subtle::ConstantTimeEq;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use tracing::warn;

pub mod acl;
pub mod manager;
pub mod minimal;
pub mod payload;
pub mod permissions;
pub mod role;
pub mod view;

pub type UserModel = BichonUserV2;

fn derive_username_base(email: &str, display_name: Option<&str>) -> String {
    if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if !sanitized.is_empty() {
            return sanitized;
        }
    }
    email
        .split('@')
        .next()
        .unwrap_or("user")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn allocate_unique_username(base: &str) -> BichonResult<String> {
    let base = if base.is_empty() { "user" } else { base };
    let base_owned = base.to_string();
    let candidate_taken = |candidate: &str| -> BichonResult<bool> {
        let c = candidate.to_string();
        let existing = filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.username == c)?;
        Ok(existing.into_iter().next().is_some())
    };
    if !candidate_taken(&base_owned)? {
        return Ok(base_owned);
    }
    for i in 2..1000 {
        let candidate = format!("{}{}", base_owned, i);
        if !candidate_taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(raise_error!(
        "Failed to allocate a unique username for auto-provisioned OIDC user".into(),
        ErrorCode::InternalError
    ))
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoginResult {
    pub success: bool,
    pub error_message: Option<String>,
    pub access_token: Option<String>,
    pub theme: Option<String>,
    pub language: Option<String>,
    /// When true, the password step succeeded and a TOTP code is required.
    #[serde(default)]
    pub mfa_required: bool,
    /// One-time challenge token for the MFA verification step.
    #[serde(default)]
    pub mfa_challenge: Option<String>,
}

pub const DEFAULT_ADMIN_USER_ID: u64 = 100000000000000;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BichonUserV2 {
    pub id: u64,
    pub username: String,
    pub email: String,

    pub password: Option<String>,

    /// Scoped Access: Defines per-account permissions.
    /// Example:
    /// { account_id: 1, role_id: role_manager_id } -> Manager on Account 1
    /// { account_id: 2, role_id: role_viewer_id }  -> Viewer on Account 2
    pub account_access_map: BTreeMap<u64, u64>,

    pub description: Option<String>,

    /// System Roles: Permissions that apply to the whole system
    /// (e.g., system settings, creating new users).
    pub global_roles: Vec<u64>,

    /// Expiry (unix **milliseconds**, matching `utc_now!()`) for a *global*
    /// role assignment, keyed by role id.
    ///
    /// A role id present here with a timestamp in the past is expired: the
    /// assignment in `global_roles` is retained (so the delegation can be
    /// listed and renewed) but confers nothing.
    ///
    /// The expiry lives on the *assignment*, not on `UserRole`, because a role
    /// is shared: putting `expires_at` on the role would expire the grant for
    /// every holder at once, and could not express "grant Manager to Zhang for
    /// 30 days" while Li, holding the same role, is unaffected.
    ///
    /// `#[serde(default)]` keeps rows written before this field existed
    /// loading as "no expiries" — i.e. every existing assignment is permanent,
    /// which is exactly the old behaviour.
    #[serde(default)]
    pub global_role_expiries: BTreeMap<u64, i64>,

    /// Expiry (unix **milliseconds**) for a *scoped* role assignment, keyed by
    /// account id — the same shape as `account_access_map`, which is the
    /// assignment it expires. See `global_role_expiries` for why this is not
    /// on the role.
    #[serde(default)]
    pub account_role_expiries: BTreeMap<u64, i64>,

    pub avatar: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional access control settings
    pub acl: Option<AccessControl>,

    pub theme: Option<String>,
    pub language: Option<String>,

    /// SSO identity: unique subject ID from the external IdP (e.g. OIDC `sub` claim).
    pub sso_id: Option<String>,
    /// SSO provider identifier: `"oidc"` or future `"saml"` / `"ldap"`.
    pub sso_provider: Option<String>,
    /// TOTP two-factor secret (AES-256-GCM encrypted at rest).
    #[serde(default)]
    pub totp_secret: Option<String>,
    /// Whether TOTP two-factor authentication is enabled for this user.
    #[serde(default)]
    pub totp_enabled: bool,
    /// One-time recovery codes (argon2id hashes), shown only once at enrollment.
    #[serde(default)]
    pub totp_recovery_codes: Vec<String>,
}

impl MemDbModel for BichonUserV2 {
    fn collection() -> &'static str {
        "users"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

impl BichonUserV2 {
    pub fn is_using_role(&self, role_id: u64) -> bool {
        if self.global_roles.contains(&role_id) {
            return true;
        }

        if self.account_access_map.values().any(|&id| id == role_id) {
            return true;
        }
        false
    }

    pub fn list_all() -> BichonResult<Vec<UserModel>> {
        Ok(list_all_impl::<UserModel>(DB_MANAGER.db())?)
    }

    /// Whether a global role assignment has passed its expiry.
    ///
    /// An assignment with no entry in `global_role_expiries` is permanent.
    /// The comparison is strict (`now >= expiry`), so a delegation set to
    /// expire at time T confers nothing from T onward.
    pub fn global_role_expired(&self, role_id: u64, now: i64) -> bool {
        self.global_role_expiries
            .get(&role_id)
            .is_some_and(|&expires_at| now >= expires_at)
    }

    /// Whether a scoped (per-account) role assignment has passed its expiry.
    pub fn account_role_expired(&self, account_id: u64, now: i64) -> bool {
        self.account_role_expiries
            .get(&account_id)
            .is_some_and(|&expires_at| now >= expires_at)
    }

    /// Effective global role ids: every assignment that has not expired.
    ///
    /// Use this rather than reading `global_roles` directly when the question
    /// is "what may this user do". `global_roles` is the *grant record* and
    /// deliberately keeps expired entries so a delegation stays visible and
    /// renewable; this is the *authority*.
    pub fn effective_global_roles(&self) -> Vec<u64> {
        let now = utc_now!();
        self.global_roles
            .iter()
            .copied()
            .filter(|&rid| !self.global_role_expired(rid, now))
            .collect()
    }

    fn get_all_permissions(&self) -> HashSet<String> {
        let mut all_perms = HashSet::new();

        // Effective roles only. This function backs `is_admin()`, which is the
        // ROOT bypass every permission check short-circuits through — a role
        // that still contributed here after expiring would keep granting full
        // administrative access past its end date, silently.
        for &role_id in &self.effective_global_roles() {
            if let Ok(Some(role)) = UserRole::find(role_id) {
                for perm in role.permissions {
                    all_perms.insert(perm);
                }
            }
        }

        all_perms
    }

    pub fn to_view(self, role_lookup: &BTreeMap<u64, UserRole>) -> UserView {
        let global_roles_names = self
            .global_roles
            .iter()
            .filter_map(|role_id| role_lookup.get(role_id))
            .map(|role| role.name.clone())
            .collect();

        let account_roles_summary = self
            .account_access_map
            .iter()
            .map(|(acc_id, role_id)| {
                let role_name = role_lookup
                    .get(role_id)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "Unknown Role".to_string());
                (*acc_id, role_name)
            })
            .collect();

        // Deadlines are computed once and used by three things below: which
        // roles still count, what the UI displays, and what is dropped from
        // the view's expiry maps.
        let now = utc_now!();
        let live_global: BTreeMap<u64, i64> = self
            .global_role_expiries
            .iter()
            .filter(|(&rid, &expires_at)| {
                now < expires_at && self.global_roles.contains(&rid)
            })
            .map(|(&rid, &expires_at)| (rid, expires_at))
            .collect();
        let live_scoped: BTreeMap<u64, i64> = self
            .account_role_expiries
            .iter()
            .filter(|(acc_id, &expires_at)| {
                now < expires_at && self.account_access_map.contains_key(acc_id)
            })
            .map(|(&acc_id, &expires_at)| (acc_id, expires_at))
            .collect();

        let global_permissions = {
            let mut perms = BTreeSet::new();

            // Effective roles only. Listing the permissions of a lapsed
            // delegation here would show an administrator that the user holds
            // access the enforcement path has already stopped granting — the
            // view would disagree with the system, in the permissive
            // direction, which is the worst way for them to disagree.
            for role_id in &self.effective_global_roles() {
                if let Some(role) = role_lookup.get(role_id) {
                    perms.extend(role.permissions.iter().cloned());
                }
            }

            perms.into_iter().collect()
        };

        let account_permissions = {
            let mut map: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();

            for (account_id, role_id) in &self.account_access_map {
                if self.account_role_expired(*account_id, now) {
                    continue;
                }
                if let Some(role) = role_lookup.get(role_id) {
                    let entry = map.entry(*account_id).or_default();
                    entry.extend(role.permissions.iter().cloned());
                }
            }

            map.into_iter()
                .map(|(acc_id, perms)| (acc_id, perms.into_iter().collect()))
                .collect()
        };
        UserView {
            id: self.id,
            username: self.username,
            email: self.email,
            password: self.password.map(|_| "************".to_string()),
            account_access_map: self.account_access_map,
            account_roles_summary,
            description: self.description,
            global_roles: self.global_roles,
            global_roles_names,
            global_role_expiries: live_global,
            account_role_expiries: live_scoped,
            avatar: self.avatar,
            created_at: self.created_at,
            updated_at: self.updated_at,
            acl: self.acl,
            account_permissions,
            global_permissions,
            theme: self.theme,
            language: self.language,
            sso_id: self.sso_id,
            sso_provider: self.sso_provider,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.get_all_permissions().contains(Permission::ROOT)
    }

    pub fn ensure_default_admin_exists() -> BichonResult<()> {
        let now = utc_now!();

        // 1. Try to get the existing admin user
        let admin = find_impl::<UserModel>(DB_MANAGER.db(), &DEFAULT_ADMIN_USER_ID.to_string())?;

        if admin.is_none() {
            // 2. Insert the BichonUser with the updated schema
            let user = UserModel {
                id: DEFAULT_ADMIN_USER_ID,
                username: "admin".into(),
                email: "placeholder@example.com".into(),
                password: Some(hash_login_password("admin@bichon")?),

                // Use global_roles as defined in our new schema
                global_roles: vec![DEFAULT_ADMIN_ROLE_ID],
                global_role_expiries: BTreeMap::new(),
                account_role_expiries: BTreeMap::new(),

                // Admin usually doesn't need specific scoped access
                account_access_map: BTreeMap::new(),

                avatar: None,
                created_at: now,
                updated_at: now,
                description: Some("System default administrator".into()),
                acl: None,
                theme: None,
                language: None,
                sso_id: None,
                sso_provider: None,
                totp_secret: None,
                totp_enabled: false,
                totp_recovery_codes: Vec::new(),
            };

            // 3. Generate and insert an initial access token for the first-time setup
            let access_token = AccessTokenModel {
                token: generate_token!(128),
                created_at: now,
                updated_at: now,
                last_access_at: Default::default(),
                name: Some("Initial Setup Token".into()),
                user_id: DEFAULT_ADMIN_USER_ID,
                token_type: TokenType::WebUI,
                expire_at: None, // Admin setup token usually persistent until changed
            };

            with_transaction(DB_MANAGER.db(), move |txn| {
                let txn = txn
                    .insert("users", DEFAULT_ADMIN_USER_ID.to_string(), &user)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .upsert("tokens", access_token.token.clone(), &access_token)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                Ok(txn)
            })?;
        }

        Ok(())
    }

    pub fn authenticate_user(username: String, password: String) -> BichonResult<LoginResult> {
        // Find by username
        let username_for_first = username.clone();
        let users = filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| {
            u.username == username_for_first
        })?;
        let user = match users.into_iter().next() {
            Some(u) => u,
            None => {
                // Fallback: find by email
                let users =
                    filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.email == username)?;
                match users.into_iter().next() {
                    Some(u) => u,
                    None => {
                        return Ok(LoginResult {
                            success: false,
                            error_message: Some("User or email not found.".to_string()),
                            access_token: None,
                            theme: None,
                            language: None,
                            mfa_required: false,
                            mfa_challenge: None,
                        });
                    }
                }
            }
        };

        match user.password.as_ref() {
            Some(stored_password) => {
                let is_argon2 = stored_password.starts_with("$argon2id$");
                let password_ok = if is_argon2 {
                    match PasswordHash::new(stored_password) {
                        Ok(parsed_hash) => Argon2::default()
                            .verify_password(password.as_bytes(), &parsed_hash)
                            .is_ok(),
                        Err(_) => false,
                    }
                } else {
                    let decrypted = decrypt!(stored_password)?;
                    bool::from(password.as_bytes().ct_eq(decrypted.as_bytes()))
                };

                if password_ok {
                    if !is_argon2 {
                        let id_string = user.id.to_string();
                        match hash_login_password(&password) {
                            Ok(hashed) => {
                                if let Err(e) = update_impl::<UserModel>(
                                    DB_MANAGER.db(),
                                    &id_string,
                                    move |current| {
                                        let mut updated = current.clone();
                                        updated.password = Some(hashed);
                                        updated.updated_at = utc_now!();
                                        Ok(updated)
                                    },
                                ) {
                                    warn!(
                                        "Login succeeded but failed to upgrade password storage for user '{}': {:#?}",
                                        user.username, e
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Login succeeded but failed to hash password for user '{}': {:#?}",
                                    user.username, e
                                );
                            }
                        }
                    }
                    if user.totp_enabled {
                        let challenge = AccessTokenModel::new_mfa_challenge(user.id)?;
                        return Ok(LoginResult {
                            success: true,
                            error_message: None,
                            access_token: None,
                            theme: user.theme.clone(),
                            language: user.language.clone(),
                            mfa_required: true,
                            mfa_challenge: Some(challenge),
                        });
                    }
                    let new_token = AccessTokenModel::reset_webui_token(user.id)?;
                    Ok(LoginResult {
                        success: true,
                        error_message: None,
                        access_token: Some(new_token),
                        theme: user.theme,
                        language: user.language,
                        mfa_required: false,
                        mfa_challenge: None,
                    })
                } else {
                    warn!(
                        "Login failed: Incorrect password for user '{}'.",
                        user.username
                    );
                    Ok(LoginResult {
                        success: false,
                        error_message: Some("Incorrect password.".to_string()),
                        access_token: None,
                        theme: None,
                        language: None,
                        mfa_required: false,
                        mfa_challenge: None,
                    })
                }
            }
            None => {
                warn!(
                    "Login failed: User '{}' has no password set.",
                    user.username
                );
                Ok(LoginResult {
                    success: false,
                    error_message: Some(
                        format!(
                            "User '{}' has no password set. Please try logging in with an alternative method (e.g., OAuth/SSO).",
                            user.username
                        )
                    ),
                    access_token: None,
                    theme: None,
                    language: None,
                    mfa_required: false,
                    mfa_challenge: None,
                })
            }
        }
    }

    pub fn find(user_id: u64) -> BichonResult<Option<UserModel>> {
        find_impl::<UserModel>(DB_MANAGER.db(), &user_id.to_string())
    }

    /// Resolve or auto-provision a Bichon user for an OIDC login.
    ///
    /// Resolution order:
    ///   1. Match on `(sso_provider, sso_id)` — same subject that logged in before.
    ///   2. Match on `email` — bind the existing user to this SSO identity.
    ///   3. Auto-provision a new user with the configured default global role.
    ///
    /// Returns the resolved/created user. The caller is expected to issue a
    /// WebUI access token for that user.
    pub fn find_or_provision_sso_user(
        provider: &str,
        subject: &str,
        email: &str,
        display_name: Option<&str>,
        default_role_id: u64,
    ) -> BichonResult<UserModel> {
        let subject_owned = subject.to_string();
        let provider_owned = provider.to_string();

        let matches_by_sso = filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| {
            u.sso_id.as_deref() == Some(&subject_owned)
                && u.sso_provider.as_deref() == Some(&provider_owned)
        })?;
        if let Some(user) = matches_by_sso.into_iter().next() {
            return Ok(user);
        }

        let email_owned = email.to_string();
        let matches_by_email =
            filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.email == email_owned)?;
        if let Some(mut user) = matches_by_email.into_iter().next() {
            let now = utc_now!();
            user.sso_id = Some(subject.to_string());
            user.sso_provider = Some(provider.to_string());
            user.updated_at = now;
            let user_clone = user.clone();
            update_impl(DB_MANAGER.db(), &user.id.to_string(), move |_current: UserModel| {
                Ok(user_clone.clone())
            })?;
            return Ok(user);
        }

        let now = utc_now!();
        let base = derive_username_base(email, display_name);
        let username = allocate_unique_username(&base)?;
        let new_user = UserModel {
            id: id!(96),
            username,
            email: email.to_string(),
            password: None,
            account_access_map: BTreeMap::new(),
            description: Some(format!("Auto-provisioned via OIDC ({})", provider)),
            global_roles: vec![default_role_id],
            avatar: None,
            created_at: now,
            updated_at: now,
            acl: None,
            theme: None,
            language: None,
            sso_id: Some(subject.to_string()),
            sso_provider: Some(provider.to_string()),
            totp_secret: None,
            totp_enabled: false,
            totp_recovery_codes: Vec::new(),
            // SSO-provisioned role assignments are permanent (no expiries).
            global_role_expiries: BTreeMap::new(),
            account_role_expiries: BTreeMap::new(),
        };

        let user_clone = new_user.clone();
        with_transaction(DB_MANAGER.db(), move |txn| {
            txn.insert("users", new_user.key(), &new_user)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
        })?;
        Ok(user_clone)
    }

    // ── TOTP two-factor authentication ─────────────────────────────

    /// Store a new (encrypted) TOTP secret. Enrollment is only completed by
    /// `enable_totp_with_recovery_codes` after the user proves the code works.
    pub fn set_totp_secret(&self, secret: &str) -> BichonResult<()> {
        let encrypted = crate::encrypt!(secret)?;
        let id = self.id.to_string();
        update_impl::<UserModel>(DB_MANAGER.db(), &id, move |current| {
            let mut updated = current.clone();
            updated.totp_secret = Some(encrypted);
            updated.totp_enabled = false;
            updated.updated_at = utc_now!();
            Ok(updated)
        })?;
        Ok(())
    }

    /// Decrypt this user's TOTP secret (used during verification).
    pub fn decrypted_totp_secret(&self) -> BichonResult<Option<String>> {
        match &self.totp_secret {
            Some(secret) => Ok(Some(decrypt!(secret)?)),
            None => Ok(None),
        }
    }

    /// Enable TOTP and generate a fresh set of one-time recovery codes.
    /// Returns the plaintext recovery codes (shown to the user exactly once).
    pub fn enable_totp_with_recovery_codes(&self) -> BichonResult<Vec<String>> {
        let codes = crate::utils::totp::generate_recovery_codes();
        let mut hashed = Vec::with_capacity(codes.len());
        for code in &codes {
            hashed.push(crate::utils::totp::hash_recovery_code(code)?);
        }
        let id = self.id.to_string();
        update_impl::<UserModel>(DB_MANAGER.db(), &id, move |current| {
            let mut updated = current.clone();
            updated.totp_enabled = true;
            updated.totp_recovery_codes = hashed;
            updated.updated_at = utc_now!();
            Ok(updated)
        })?;
        Ok(codes)
    }

    /// Disable TOTP and clear all MFA state.
    pub fn disable_totp(&self) -> BichonResult<()> {
        let id = self.id.to_string();
        update_impl::<UserModel>(DB_MANAGER.db(), &id, move |current| {
            let mut updated = current.clone();
            updated.totp_enabled = false;
            updated.totp_secret = None;
            updated.totp_recovery_codes = Vec::new();
            updated.updated_at = utc_now!();
            Ok(updated)
        })?;
        Ok(())
    }

    /// Verify a TOTP code against this user's secret (no state change).
    pub fn verify_totp_code(&self, code: &str, window: u8) -> BichonResult<bool> {
        let Some(secret) = self.decrypted_totp_secret()? else {
            return Ok(false);
        };
        Ok(crate::utils::totp::verify_code(&secret, code, window))
    }

    /// Verify a one-time recovery code; consumes it on success.
    pub fn verify_recovery_code(&self, code: &str) -> BichonResult<bool> {
        let normalized = crate::utils::totp::normalize_recovery_code(code);
        if normalized.is_empty() {
            return Ok(false);
        }
        let id = self.id.to_string();
        for stored in &self.totp_recovery_codes {
            if crate::utils::totp::verify_hashed_code(stored, &normalized)? {
                let consumed = stored.clone();
                let remaining: Vec<String> = self
                    .totp_recovery_codes
                    .iter()
                    .filter(|s| **s != consumed)
                    .cloned()
                    .collect();
                update_impl::<UserModel>(DB_MANAGER.db(), &id, move |current| {
                    let mut updated = current.clone();
                    updated.totp_recovery_codes = remaining;
                    updated.updated_at = utc_now!();
                    Ok(updated)
                })?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn check_username_conflict(username: &str) -> BichonResult<()> {
        let username_clone = username.to_string();
        let users =
            filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.username == username_clone)?;

        if users.into_iter().next().is_some() {
            return Err(raise_error!(
                format!("Username '{}' is already taken.", username).into(),
                ErrorCode::AlreadyExists
            ));
        }

        Ok(())
    }

    pub fn check_email_conflict(email: &str) -> BichonResult<()> {
        let email_clone = email.to_string();
        let users = filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.email == email_clone)?;

        if users.into_iter().next().is_some() {
            return Err(raise_error!(
                format!("Email '{}' is already registered.", email).into(),
                ErrorCode::AlreadyExists
            ));
        }

        Ok(())
    }

    pub fn create(request: UserCreateRequest) -> BichonResult<UserModel> {
        request.validate()?;
        Self::check_username_conflict(&request.username)?;
        Self::check_email_conflict(&request.email)?;

        let password_hash = Some(hash_login_password(&request.password)?);
        let now = utc_now!();

        let user = UserModel {
            id: id!(96),
            username: request.username,
            email: request.email,
            password: password_hash,
            global_roles: request.global_roles,
            avatar: request.avatar_base64,
            description: request.description,
            acl: request.acl,
            created_at: now,
            updated_at: now,
            account_access_map: request.account_access_map,
            // A freshly created user's assignments are permanent until an
            // administrator delegates with a deadline.
            global_role_expiries: BTreeMap::new(),
            account_role_expiries: BTreeMap::new(),
            theme: request.theme,
            language: request.language,
            sso_id: None,
            sso_provider: None,
            totp_secret: None,
            totp_enabled: false,
            totp_recovery_codes: Vec::new(),
        };

        let user_clone = user.clone();

        // 4. Atomic transaction for User and Initial Token
        let access_token = AccessTokenModel {
            token: generate_token!(128),
            created_at: now,
            updated_at: now,
            last_access_at: Default::default(),
            name: Some("Default WebUI Token".into()),
            user_id: user.id,
            token_type: TokenType::WebUI,
            expire_at: None,
        };

        with_transaction(DB_MANAGER.db(), move |txn| {
            let txn = txn
                .insert("users", user.key(), &user)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .insert("tokens", access_token.token.clone(), &access_token)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            Ok(txn)
        })?;

        Ok(user_clone)
    }

    //delete user，
    pub fn remove(id: u64) -> BichonResult<()> {
        if DEFAULT_ADMIN_USER_ID == id {
            return Err(raise_error!(
                format!("The default admin user (id={}) cannot be removed", id),
                ErrorCode::PermissionDenied
            ));
        }

        delete_impl::<UserModel>(DB_MANAGER.db(), &id.to_string())?;

        // Find and delete tokens belonging to this user
        let uid = id;

        let coll = DB_MANAGER.db().collection("tokens");
        let all_tokens: Vec<AccessTokenModel> = coll
            .list_all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let token_keys: Vec<String> = all_tokens
            .into_iter()
            .filter(|t| t.user_id == uid)
            .map(|t| t.token)
            .collect();

        if !token_keys.is_empty() {
            with_transaction(DB_MANAGER.db(), move |txn| {
                let mut txn = txn;
                for key in token_keys {
                    txn = txn.delete("tokens", key);
                }
                Ok(txn)
            })?;
        }

        // Remove edition-specific per-user data (e.g. Pro analytics views).
        crate::ext::user_cleanup::run_cleanups(id);

        Ok(())
    }

    pub fn update(id: u64, request: UserUpdateRequest) -> BichonResult<()> {
        let _ = &request.validate()?;
        let password_changed = request.password.is_some();
        let is_default_admin = id == DEFAULT_ADMIN_USER_ID;

        if is_default_admin {
            if let Some(roles) = request.global_roles.as_deref() {
                let is_valid = matches!(
                    roles,
                    [role] if *role == DEFAULT_ADMIN_ROLE_ID
                );

                if !is_valid {
                    return Err(raise_error!(
                        format!(
                            "The role assignments for default admin (id={}) are immutable to ensure system accessibility.",
                            id
                        ),
                        ErrorCode::Forbidden
                    ));
                }
            }
        }

        if let Some(username) = &request.username {
            let username_clone = username.clone();
            let users = filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| {
                u.username == username_clone
            })?;

            if let Some(u) = users.into_iter().next() {
                if u.id != id {
                    return Err(raise_error!(
                        format!("Username '{}' is already taken.", username).into(),
                        ErrorCode::AlreadyExists
                    ));
                }
            }
        }

        if let Some(email) = &request.email {
            let email_clone = email.clone();
            let users =
                filter_impl::<UserModel, _>(DB_MANAGER.db(), move |u| u.email == email_clone)?;

            if let Some(u) = users.into_iter().next() {
                if u.id != id {
                    return Err(raise_error!(
                        format!("Email '{}' is already registered.", email).into(),
                        ErrorCode::AlreadyExists
                    ));
                }
            }
        }

        update_impl::<UserModel>(DB_MANAGER.db(), &id.to_string(), move |current| {
            let mut updated = current.clone();
            if let Some(username) = request.username {
                updated.username = username;
            }
            if let Some(email) = request.email {
                updated.email = email;
            }
            if let Some(desc) = request.description {
                updated.description = Some(desc);
            }
            if let Some(password) = request.password {
                updated.password = Some(hash_login_password(&password)?);
            }

            if let Some(global_roles) = request.global_roles {
                updated.global_roles = global_roles;
                // Drop expiries for roles that are no longer assigned. Leaving
                // them would make a *later* permanent re-grant inherit the old
                // deadline: the admin re-adds the role, sees it in the list,
                // and the user still has nothing — with no expiry visible on
                // the assignment they just made.
                let assigned = updated.global_roles.clone();
                updated
                    .global_role_expiries
                    .retain(|role_id, _| assigned.contains(role_id));
            }

            if let Some(acl) = request.acl {
                updated.acl = Some(acl);
            }

            if let Some(account_access_map) = request.account_access_map {
                updated.account_access_map = account_access_map;
                // Same reasoning as the global case above, keyed by account.
                let scoped = updated.account_access_map.clone();
                updated
                    .account_role_expiries
                    .retain(|account_id, _| scoped.contains_key(account_id));
            }

            if let Some(avatar_base64) = request.avatar_base64 {
                updated.avatar = Some(avatar_base64);
            }

            if let Some(theme) = request.theme {
                updated.theme = Some(theme);
            }

            if let Some(language) = request.language {
                updated.language = Some(language);
            }

            updated.updated_at = utc_now!();

            Ok(updated)
        })?;

        if password_changed {
            AccessTokenModel::reset_webui_token(id)?;
        }

        Ok(())
    }

    fn list_authorized_users(account_id: u64) -> BichonResult<Vec<UserModel>> {
        let all = Self::list_all()?;
        let result: Vec<UserModel> = all
            .into_iter()
            .filter(|e| e.account_access_map.contains_key(&account_id))
            .collect();
        Ok(result)
    }

    pub fn cleanup_account(account_id: u64) -> BichonResult<()> {
        let users = Self::list_authorized_users(account_id)?;
        if users.is_empty() {
            return Ok(());
        }

        let now = utc_now!();
        for user in users {
            let key = user.id.to_string();
            update_impl::<UserModel>(DB_MANAGER.db(), &key, move |current| {
                let mut updated = current.clone();
                if updated.account_access_map.remove(&account_id).is_some() {
                    updated.updated_at = now;
                }
                Ok(updated)
            })?;
        }

        Ok(())
    }
}

fn hash_login_password(password: &str) -> BichonResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| {
            raise_error!(
                "Failed to hash password.".into(),
                ErrorCode::InternalError
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::after_n_days_timestamp;

    const NOW: i64 = 1_700_000_000_000;

    fn user(global_roles: Vec<u64>) -> UserModel {
        UserModel {
            global_roles,
            ..Default::default()
        }
    }

    /// The trap this field invites: `utc_now!()` is *milliseconds*, so an
    /// expiry written as `now + 30 * 86_400` (seconds, a common slip) lands in
    /// 1970 and every delegated role is born dead. Pinning the helper's unit
    /// against `utc_now!()` catches the mismatch at the source.
    #[test]
    fn expiry_helper_produces_milliseconds_like_utc_now() {
        let now = utc_now!();
        let in_a_day = after_n_days_timestamp!(now, 1);
        assert_eq!(
            in_a_day - now,
            86_400_000,
            "expiry must be milliseconds to compare against utc_now!()"
        );
    }

    #[test]
    fn an_assignment_without_an_expiry_is_permanent() {
        let u = user(vec![7]);
        assert!(!u.global_role_expired(7, NOW));
        assert!(!u.global_role_expired(7, i64::MAX));
    }

    #[test]
    fn an_assignment_expires_at_its_deadline_not_after() {
        let mut u = user(vec![7]);
        u.global_role_expiries.insert(7, NOW + 1_000);
        assert!(!u.global_role_expired(7, NOW));
        assert!(
            !u.global_role_expired(7, NOW + 999),
            "must still be live one millisecond before the deadline"
        );
        assert!(
            u.global_role_expired(7, NOW + 1_000),
            "the deadline itself is expired"
        );
        assert!(u.global_role_expired(7, NOW + 1_001));
    }

    /// A bare `Default::default()` user has no roles, so a missing map entry
    /// and an empty map must both mean "permanent" — otherwise every existing
    /// install loses its assignments on upgrade.
    #[test]
    fn an_empty_expiry_map_leaves_every_role_permanent() {
        let u = user(vec![1, 2, 3]);
        assert!(u.global_role_expiries.is_empty());
        assert_eq!(u.effective_global_roles(), vec![1, 2, 3]);
    }

    /// The grant record must survive expiry, so a lapsed delegation stays
    /// visible (and renewable) instead of vanishing from the admin's view.
    #[test]
    fn an_expired_assignment_is_dropped_from_authority_but_kept_as_a_record() {
        let mut u = user(vec![1, 2]);
        u.global_role_expiries.insert(2, NOW - 1);
        assert_eq!(
            u.effective_global_roles(),
            vec![1],
            "an expired role must not confer authority"
        );
        assert!(
            u.global_roles.contains(&2),
            "the grant itself is retained so it can be listed and renewed"
        );
        assert_eq!(u.global_role_expiries.get(&2), Some(&(NOW - 1)));
    }

    /// Re-granting a role that was previously delegated must be permanent
    /// again, not silently dead. `UserModel::update` retains only the expiries
    /// of roles still assigned, which is what makes the second grant work; if
    /// that retain is ever removed this test fails rather than shipping a
    /// feature that quietly grants nothing.
    #[test]
    fn reassigning_a_role_does_not_inherit_its_old_expiry() {
        let mut u = user(vec![1, 2]);
        u.global_role_expiries.insert(2, NOW - 1);
        assert_eq!(u.effective_global_roles(), vec![1]);

        // Simulate the retain `update()` performs when global_roles becomes
        // `[1]`, then a later re-grant of 2.
        let assigned = vec![1u64];
        u.global_role_expiries.retain(|rid, _| assigned.contains(rid));
        u.global_roles = vec![1, 2];

        assert_eq!(
            u.effective_global_roles(),
            vec![1, 2],
            "the re-granted role must be live, not carrying its previous deadline"
        );
    }

    /// Scoped assignments expire by *account*, not by role id — the map is
    /// keyed like `account_access_map`, and a role id accidentally used as a
    /// key here would silently never match.
    #[test]
    fn scoped_expiry_is_keyed_by_account() {
        let mut u = user(vec![]);
        u.account_access_map.insert(42, 5);
        u.account_role_expiries.insert(42, NOW - 1);
        assert!(u.account_role_expired(42, NOW));
        assert!(
            !u.account_role_expired(5, NOW),
            "the role id must not be what is looked up"
        );
    }
}
