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

use std::collections::BTreeMap;

use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::modules::users::acl::AccessControl;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct UserView {
    pub id: u64,
    pub username: String,
    pub email: String,

    pub password: Option<String>,

    /// Scoped Access: Defines per-account permissions.
    /// Example:
    /// { account_id: 1, role_id: role_manager_id } -> Manager on Account 1
    /// { account_id: 2, role_id: role_viewer_id }  -> Viewer on Account 2
    pub account_access_map: BTreeMap<u64, u64>,
    pub account_roles_summary: BTreeMap<u64, String>,
    pub account_permissions: BTreeMap<u64, Vec<String>>,
    pub description: Option<String>,
    /// Global Roles: Permissions that apply to the whole system
    /// (e.g., system settings, creating new users).
    pub global_roles: Vec<u64>,
    pub global_roles_names: Vec<String>,
    pub global_permissions: Vec<String>,
    pub avatar: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional access control settings
    pub acl: Option<AccessControl>,
    pub theme: Option<String>,
    pub language: Option<String>,
}
