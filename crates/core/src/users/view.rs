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

use std::collections::BTreeMap;

//use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::users::acl::AccessControl;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
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

    /// Deadline (unix ms) for each *global* role assignment that has one,
    /// keyed by role id. A role absent from this map is permanent.
    ///
    /// Only non-expired assignments appear: the UI's question is "what is
    /// still delegated and when does it end", and an entry that has already
    /// lapsed is indistinguishable from a role the user was re-granted.
    ///
    /// `#[serde(default)]` + `skip_serializing_if` keep the wire shape
    /// unchanged for the overwhelmingly common case of no delegations — an
    /// older client parsing this response never sees the key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub global_role_expiries: BTreeMap<u64, i64>,

    /// Deadline (unix ms) for each *scoped* role assignment that has one,
    /// keyed by account id. See `global_role_expiries`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub account_role_expiries: BTreeMap<u64, i64>,
    pub avatar: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional access control settings
    pub acl: Option<AccessControl>,
    pub theme: Option<String>,
    pub language: Option<String>,
    pub nav_layout: Option<String>,

    /// SSO identity: unique subject ID from the external IdP (e.g. OIDC `sub` claim).
    pub sso_id: Option<String>,
    /// SSO provider identifier: `"oidc"` or future `"saml"` / `"ldap"`.
    pub sso_provider: Option<String>,
}
