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
    modules::{
        account::migration::AccountModel,
        error::{code::ErrorCode, BichonResult},
        users::{
            acl::AccessControl,
            permissions::{Permission, VALID_PERMISSION_SET},
            role::{RoleType, UserRole},
        },
        utils::decode_avatar_bytes,
    },
    raise_error,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn allowed_themes() -> HashSet<&'static str> {
    ["light", "dark"].into_iter().collect()
}

fn allowed_languages() -> HashSet<&'static str> {
    [
        "ar", "da", "de", "en", "es", "fi", "fr", "it", "jp", "ko", "nl", "no", "pl", "pt", "ru",
        "sv", "zh", "zh-tw",
    ]
    .into_iter()
    .collect()
}

fn validate_option_in_set(
    value: &Option<String>,
    allowed: &std::collections::HashSet<&'static str>,
    field_name: &str,
) -> BichonResult<()> {
    if let Some(v) = value {
        if !allowed.contains(v.as_str()) {
            return Err(raise_error!(
                format!("invalid {} value: '{}'", field_name, v),
                ErrorCode::InvalidParameter
            ));
        }
    }
    Ok(())
}

fn validate_theme(theme: &Option<String>) -> BichonResult<()> {
    validate_option_in_set(theme, &allowed_themes(), "theme")
}

fn validate_language(language: &Option<String>) -> BichonResult<()> {
    validate_option_in_set(language, &allowed_languages(), "language")
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct RoleCreateRequest {
    pub name: String,
    pub role_type: RoleType,
    pub description: Option<String>,
    pub permissions: BTreeSet<String>,
}

impl RoleCreateRequest {
    pub async fn validate(&self) -> BichonResult<()> {
        let trimmed_name = self.name.trim();
        if trimmed_name.is_empty() {
            return Err(raise_error!(
                "Role name cannot be empty or consist only of whitespace.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        let name_lower = trimmed_name.to_lowercase();
        if name_lower == "admin" || name_lower == "manager" || name_lower == "viewer" {
            return Err(raise_error!(
                format!(
                    "The name '{}' is reserved for system builtin roles.",
                    trimmed_name
                ),
                ErrorCode::InvalidParameter
            ));
        }

        if self.permissions.is_empty() {
            return Err(raise_error!(
                "Role must be assigned at least one permission.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        for permission in &self.permissions {
            if !VALID_PERMISSION_SET.contains(permission.as_str()) {
                return Err(raise_error!(
                    format!(
                        "Invalid permission '{}' specified in the request.",
                        permission
                    ),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        Permission::validate_role_permissions(&self.role_type, &self.permissions)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct RoleUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub permissions: Option<BTreeSet<String>>,
}

impl RoleUpdateRequest {
    pub async fn validate(&self) -> BichonResult<()> {
        // 1. Ensure at least one field is provided for the update
        if self.name.is_none() && self.description.is_none() && self.permissions.is_none() {
            return Err(raise_error!(
                "Update request must contain at least one field to modify (name, description, or permissions).".into(),
                ErrorCode::InvalidParameter
            ));
        }

        // 2. Validate Name if present
        if let Some(name) = &self.name {
            let trimmed_name = name.trim();
            if trimmed_name.is_empty() {
                return Err(raise_error!(
                    "Role name cannot be set to an empty string or consist only of whitespace."
                        .into(),
                    ErrorCode::InvalidParameter
                ));
            }

            // Prevent renaming to reserved system names
            let name_lower = trimmed_name.to_lowercase();
            if name_lower == "admin" || name_lower == "manager" || name_lower == "viewer" {
                return Err(raise_error!(
                    format!(
                        "The name '{}' is reserved for system builtin roles.",
                        trimmed_name
                    ),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        // 3. Validate Permissions if present
        if let Some(permissions) = &self.permissions {
            // Ensure the role doesn't end up with zero permissions
            if permissions.is_empty() {
                return Err(raise_error!(
                    "Permissions list cannot be empty. A role must have at least one permission."
                        .into(),
                    ErrorCode::InvalidParameter
                ));
            }

            // Check for invalid permission strings using a functional approach
            if let Some(invalid_permission) = permissions
                .iter()
                .find(|p| !VALID_PERMISSION_SET.contains(p.as_str()))
            {
                return Err(raise_error!(
                    format!(
                        "Invalid permission '{}' specified in the update request.",
                        invalid_permission
                    ),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct UserCreateRequest {
    pub username: String,

    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: String,

    pub password: String,

    /// Global Roles: System-wide permissions (e.g., Admin, User Manager).
    pub global_roles: Vec<u64>,

    /// Scoped Access: List of accounts paired with specific roles.
    /// This allows different permissions per account.
    pub account_access_map: BTreeMap<u64, u64>,

    pub acl: Option<AccessControl>,
    pub avatar_base64: Option<String>,
    pub description: Option<String>,
    pub theme: Option<String>,
    pub language: Option<String>,
}

impl UserCreateRequest {
    pub async fn validate(&self) -> BichonResult<()> {
        let username_len = self.username.len();

        // 1. Username constraints
        if username_len < 5 {
            return Err(raise_error!(
                "Username must be at least 5 characters long.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if username_len > 32 {
            return Err(raise_error!(
                "Username cannot exceed 32 characters.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        // 2. Password constraints
        let password_len = self.password.len();
        if password_len < 8 {
            return Err(raise_error!(
                "Password must be at least 8 characters long.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if password_len > 256 {
            return Err(raise_error!(
                "Password cannot exceed 256 characters.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        // 3. Global Roles validation
        if self.global_roles.is_empty() {
            return Err(raise_error!(
                "Global roles list cannot be empty. At least one role must be selected.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        validate_theme(&self.theme)?;
        validate_language(&self.language)?;

        let all_roles = UserRole::list_all().await?;
        let role_type_map: HashMap<u64, RoleType> =
            all_roles.into_iter().map(|r| (r.id, r.role_type)).collect();

        for rid in &self.global_roles {
            match role_type_map.get(rid) {
                Some(RoleType::Global) => {}
                Some(_) => {
                    return Err(raise_error!(
                        format!("Role {} is not a System role", rid),
                        ErrorCode::InvalidParameter
                    ))
                }
                None => {
                    return Err(raise_error!(
                        format!("System Role {} not found", rid),
                        ErrorCode::InvalidParameter
                    ))
                }
            }
        }

        for (aid, rid) in &self.account_access_map {
            if AccountModel::find(*aid).await?.is_none() {
                return Err(raise_error!(
                    format!("Account {} not found", aid),
                    ErrorCode::InvalidParameter
                ));
            }
            match role_type_map.get(rid) {
                Some(RoleType::Account) => {}
                Some(_) => {
                    return Err(raise_error!(
                        format!(
                            "Role {} assigned to account {} must be an Account role",
                            rid, aid
                        ),
                        ErrorCode::InvalidParameter
                    ))
                }
                None => {
                    return Err(raise_error!(
                        format!("Role {} for account {} not found", rid, aid),
                        ErrorCode::InvalidParameter
                    ))
                }
            }
        }

        if let Some(acl) = &self.acl {
            acl.validate()?;
        }

        if let Some(desc) = &self.description {
            if desc.len() > 256 {
                return Err(raise_error!(
                    "Description cannot exceed 256 characters.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        if let Some(avatar_base64) = &self.avatar_base64 {
            decode_avatar_bytes(&avatar_base64)?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct UserUpdateRequest {
    pub username: Option<String>,
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: Option<String>,
    pub password: Option<String>,
    pub avatar_base64: Option<String>,
    pub global_roles: Option<Vec<u64>>,
    /// Scoped Access
    pub account_access_map: Option<BTreeMap<u64, u64>>,
    pub acl: Option<AccessControl>,
    pub description: Option<String>,
    pub theme: Option<String>,
    pub language: Option<String>,
}

impl UserUpdateRequest {
    pub async fn validate(&self) -> BichonResult<()> {
        if let Some(username) = &self.username {
            let len = username.len();
            if len < 5 || len > 32 {
                return Err(raise_error!(
                    "Username must be 5-32 characters.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        if let Some(password) = &self.password {
            let len = password.len();
            if len < 8 || len > 256 {
                return Err(raise_error!(
                    "Password must be 8-256 characters.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        validate_theme(&self.theme)?;
        validate_language(&self.language)?;

        let all_roles = UserRole::list_all().await?;
        let role_type_map: HashMap<u64, RoleType> =
            all_roles.into_iter().map(|r| (r.id, r.role_type)).collect();

        if let Some(roles) = &self.global_roles {
            if roles.is_empty() {
                return Err(raise_error!(
                    "Roles list cannot be empty.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            for role_id in roles {
                match role_type_map.get(role_id) {
                    Some(RoleType::Global) => {}
                    Some(_) => {
                        return Err(raise_error!(
                            format!("Role {} is not a System role", role_id),
                            ErrorCode::InvalidParameter
                        ))
                    }
                    None => {
                        return Err(raise_error!(
                            format!("System Role {} not found", role_id),
                            ErrorCode::InvalidParameter
                        ))
                    }
                }
            }
        }

        if let Some(account_access_map) = &self.account_access_map {
            for (aid, rid) in account_access_map {
                if AccountModel::find(*aid).await?.is_none() {
                    return Err(raise_error!(
                        format!("Account {} not found", aid),
                        ErrorCode::InvalidParameter
                    ));
                }
                match role_type_map.get(rid) {
                    Some(RoleType::Account) => {}
                    Some(_) => {
                        return Err(raise_error!(
                            format!(
                                "Role {} assigned to account {} must be an Account role",
                                rid, aid
                            ),
                            ErrorCode::InvalidParameter
                        ))
                    }
                    None => {
                        return Err(raise_error!(
                            format!("Role {} for account {} not found", rid, aid),
                            ErrorCode::InvalidParameter
                        ))
                    }
                }
            }
        }

        if let Some(desc) = &self.description {
            if desc.len() > 256 {
                return Err(raise_error!(
                    "Description too long.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }

        if let Some(acl) = &self.acl {
            acl.validate()?;
        }

        if let Some(avatar) = &self.avatar_base64 {
            decode_avatar_bytes(avatar)?;
        }

        Ok(())
    }
}
