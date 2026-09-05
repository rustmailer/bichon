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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoginResult {
    pub success: bool,
    pub error_message: Option<String>,
    pub access_token: Option<String>,
    pub theme: Option<String>,
    pub language: Option<String>,
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

    fn get_all_permissions(&self) -> HashSet<String> {
        let mut all_perms = HashSet::new();

        for &role_id in &self.global_roles {
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

        let global_permissions = {
            let mut perms = BTreeSet::new();

            for role_id in &self.global_roles {
                if let Some(role) = role_lookup.get(role_id) {
                    perms.extend(role.permissions.iter().cloned());
                }
            }

            perms.into_iter().collect()
        };

        let account_permissions = {
            let mut map: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();

            for (account_id, role_id) in &self.account_access_map {
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
                    let new_token = AccessTokenModel::reset_webui_token(user.id)?;
                    Ok(LoginResult {
                        success: true,
                        error_message: None,
                        access_token: Some(new_token),
                        theme: user.theme,
                        language: user.language,
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
                })
            }
        }
    }

    pub fn find(user_id: u64) -> BichonResult<Option<UserModel>> {
        find_impl::<UserModel>(DB_MANAGER.db(), &user_id.to_string())
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
            theme: request.theme,
            language: request.language,
            sso_id: None,
            sso_provider: None,
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
            }

            if let Some(acl) = request.acl {
                updated.acl = Some(acl);
            }

            if let Some(account_access_map) = request.account_access_map {
                updated.account_access_map = account_access_map;
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
