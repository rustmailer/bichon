//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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
    decrypt, encrypt, generate_token, id,
    modules::{
        database::{
            async_find_impl, batch_delete_impl, delete_impl, list_all_impl, manager::DB_MANAGER,
            secondary_find_impl, update_impl, with_transaction,
        },
        error::{code::ErrorCode, BichonResult},
        token::{AccessTokenModel, AccessTokenModelKey, TokenType},
        users::{
            acl::AccessControl,
            payload::{UserCreateRequest, UserUpdateRequest},
            permissions::Permission,
            role::{UserRole, DEFAULT_ADMIN_ROLE_ID},
            view::UserView,
        },
    },
    raise_error, utc_now,
};
use itertools::Itertools;
use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tracing::warn;

pub mod acl;
pub mod manager;
pub mod minimal;
pub mod payload;
pub mod permissions;
pub mod role;
pub mod view;

pub type UserModel = BichonUserV2;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct LoginResult {
    pub success: bool,
    pub error_message: Option<String>,
    pub access_token: Option<String>,
    pub theme: Option<String>,
    pub language: Option<String>,
}

pub const DEFAULT_ADMIN_USER_ID: u64 = 100000000000000;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
#[native_model(id = 10, version = 1)]
#[native_db]
pub struct BichonUser {
    #[primary_key]
    pub id: u64,
    #[secondary_key(unique)]
    pub username: String,
    #[secondary_key(unique)]
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
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
#[native_model(id = 10, version = 2, from = BichonUser)]
#[native_db]
pub struct BichonUserV2 {
    #[primary_key]
    pub id: u64,
    #[secondary_key(unique)]
    pub username: String,
    #[secondary_key(unique)]
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
}

impl BichonUserV2 {
    pub async fn list_all() -> BichonResult<Vec<UserModel>> {
        Ok(list_all_impl::<UserModel>(DB_MANAGER.meta_db()).await?)
    }

    async fn get_all_permissions(&self) -> HashSet<String> {
        let mut all_perms = HashSet::new();

        for &role_id in &self.global_roles {
            if let Ok(Some(role)) = UserRole::find(role_id).await {
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
        }
    }

    pub async fn is_admin(&self) -> bool {
        self.get_all_permissions().await.contains(Permission::ROOT)
    }

    pub async fn ensure_default_admin_exists() -> BichonResult<()> {
        with_transaction(DB_MANAGER.meta_db(), move |rw| {
            let now = utc_now!();

            // 1. Try to get the existing admin user
            let admin = rw
                .get()
                .primary::<UserModel>(DEFAULT_ADMIN_USER_ID)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            if admin.is_none() {
                // 2. Insert the BichonUser with the updated schema
                rw.insert(UserModel {
                    id: DEFAULT_ADMIN_USER_ID,
                    username: "admin".into(),
                    email: "placeholder@example.com".into(),
                    password: Some(encrypt!("admin@bichon")?),

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
                })
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

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

                rw.upsert(access_token)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            }

            Ok(())
        })
        .await?;

        Ok(())
    }

    pub async fn authenticate_user(
        username: String,
        password: String,
    ) -> BichonResult<LoginResult> {
        let user_option = secondary_find_impl::<UserModel>(
            DB_MANAGER.meta_db(),
            BichonUserV2Key::username,
            username.clone(),
        )
        .await?;

        let user = match user_option {
            Some(u) => u,
            None => {
                match secondary_find_impl::<UserModel>(
                    DB_MANAGER.meta_db(),
                    BichonUserV2Key::email,
                    username,
                )
                .await?
                {
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
            Some(encrypted_password) => {
                let decrypted = decrypt!(encrypted_password)?;
                if password == decrypted {
                    let new_token = AccessTokenModel::reset_webui_token(user.id).await?;
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

    pub async fn find(user_id: u64) -> BichonResult<Option<UserModel>> {
        async_find_impl(DB_MANAGER.meta_db(), user_id).await
    }

    pub async fn check_username_conflict(username: &str) -> BichonResult<()> {
        // Check username duplicate
        if secondary_find_impl::<UserModel>(
            DB_MANAGER.meta_db(),
            BichonUserV2Key::username,
            username.to_string(),
        )
        .await?
        .is_some()
        {
            return Err(raise_error!(
                format!("Username '{}' is already taken.", username).into(),
                ErrorCode::AlreadyExists
            ));
        }

        Ok(())
    }

    pub async fn check_email_conflict(email: &str) -> BichonResult<()> {
        // Check email duplicate
        if secondary_find_impl::<UserModel>(
            DB_MANAGER.meta_db(),
            BichonUserV2Key::email,
            email.to_string(),
        )
        .await?
        .is_some()
        {
            return Err(raise_error!(
                format!("Email '{}' is already registered.", email).into(),
                ErrorCode::AlreadyExists
            ));
        }

        Ok(())
    }

    pub async fn create(request: UserCreateRequest) -> BichonResult<UserModel> {
        request.validate().await?;
        Self::check_username_conflict(&request.username).await?;
        Self::check_email_conflict(&request.email).await?;

        let password_hash = Some(encrypt!(&request.password)?);
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
        };

        let user_clone = user.clone();

        // 4. Atomic transaction for User and Initial Token
        with_transaction(DB_MANAGER.meta_db(), move |rw| {
            let user_id = user.id;

            // Insert User
            rw.insert(user)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            // Create initial WebUI access token
            let access_token = AccessTokenModel {
                token: generate_token!(128),
                created_at: now,
                updated_at: now,
                last_access_at: Default::default(),
                name: Some("Default WebUI Token".into()),
                user_id,
                token_type: TokenType::WebUI,
                expire_at: None,
            };

            rw.insert(access_token)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            Ok(())
        })
        .await?;

        Ok(user_clone)
    }

    //delete user，
    pub async fn remove(id: u64) -> BichonResult<()> {
        if DEFAULT_ADMIN_USER_ID == id {
            return Err(raise_error!(
                format!("The default admin user (id={}) cannot be removed", id),
                ErrorCode::PermissionDenied
            ));
        }

        delete_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get()
                .primary::<BichonUser>(id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!("The User with id={id} that you want to delete was not found."),
                        ErrorCode::ResourceNotFound
                    )
                })
        })
        .await?;

        batch_delete_impl(DB_MANAGER.meta_db(), move |rw| {
            let tokens: Vec<AccessTokenModel> = rw
                .scan()
                .secondary::<AccessTokenModel>(AccessTokenModelKey::user_id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .start_with(id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            Ok(tokens)
        })
        .await?;
        Ok(())
    }

    pub async fn update(id: u64, request: UserUpdateRequest) -> BichonResult<()> {
        let _ = &request.validate().await?;
        let password_changed = request.password.is_some();

        if DEFAULT_ADMIN_USER_ID == id && request.global_roles.is_some() {
            return Err(raise_error!(
                format!("The role assignments for default admin (id={}) are immutable to ensure system accessibility.", id),
                ErrorCode::Forbidden
            ));
        }

        if let Some(username) = &request.username {
            let user_option = secondary_find_impl::<UserModel>(
                DB_MANAGER.meta_db(),
                BichonUserV2Key::username,
                username.to_string(),
            )
            .await?;

            if let Some(u) = user_option {
                if u.id != id {
                    return Err(raise_error!(
                        format!("Username '{}' is already taken.", username).into(),
                        ErrorCode::AlreadyExists
                    ));
                }
            }
        }

        if let Some(email) = &request.email {
            let user_option = secondary_find_impl::<UserModel>(
                DB_MANAGER.meta_db(),
                BichonUserV2Key::email,
                email.to_string(),
            )
            .await?;

            if let Some(u) = user_option {
                if u.id != id {
                    return Err(raise_error!(
                        format!("Email '{}' is already registered.", email).into(),
                        ErrorCode::AlreadyExists
                    ));
                }
            }
        }

        update_impl(
            DB_MANAGER.meta_db(),
            move |rw| {
                rw.get()
                    .primary::<UserModel>(id)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            format!("User with id={} not found", id),
                            ErrorCode::ResourceNotFound
                        )
                    })
            },
            move |current| {
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
                    updated.password = Some(encrypt!(&password)?);
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
            },
        )
        .await?;

        if password_changed {
            AccessTokenModel::reset_webui_token(id).await?;
        }

        Ok(())
    }

    async fn list_authorized_users(account_id: u64) -> BichonResult<Vec<UserModel>> {
        let all = Self::list_all().await?;
        let result: Vec<UserModel> = all
            .into_iter()
            .filter(|e| e.account_access_map.contains_key(&account_id))
            .collect();
        Ok(result)
    }

    pub async fn cleanup_account(account_id: u64) -> BichonResult<()> {
        let users = Self::list_authorized_users(account_id).await?;
        if users.is_empty() {
            return Ok(());
        }

        with_transaction(DB_MANAGER.meta_db(), move |rw| {
            let now = utc_now!();
            for user in users {
                let current = rw
                    .get()
                    .primary::<UserModel>(user.id)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            format!("User {} not found", user.id),
                            ErrorCode::ResourceNotFound
                        )
                    })?;

                let mut updated = current.clone();

                if updated.account_access_map.remove(&account_id).is_some() {
                    updated.updated_at = now;
                    rw.update(current, updated)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                }
            }
            Ok(())
        })
        .await?;

        Ok(())
    }
}

impl From<BichonUserV2> for BichonUser {
    fn from(value: BichonUserV2) -> Self {
        BichonUser {
            id: value.id,
            username: value.username,
            email: value.email,
            password: value.password,
            account_access_map: value.account_access_map,
            description: value.description,
            global_roles: value.global_roles,
            avatar: value.avatar,
            created_at: value.created_at,
            updated_at: value.updated_at,
            acl: value.acl,
        }
    }
}

impl From<BichonUser> for BichonUserV2 {
    fn from(value: BichonUser) -> Self {
        BichonUserV2 {
            id: value.id,
            username: value.username,
            email: value.email,
            password: value.password,
            account_access_map: value.account_access_map,
            description: value.description,
            global_roles: value.global_roles,
            avatar: value.avatar,
            created_at: value.created_at,
            updated_at: value.updated_at,
            acl: value.acl,
            theme: None,
            language: None,
        }
    }
}
