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

use crate::token::{AccessTokenModel, TokenType};
use crate::users::UserModel;
use serde::{Deserialize, Serialize};

/// The masked shape of a token row.
///
/// The full token is returned exactly once, from the create endpoint. Lists
/// only expose a prefix for identification and a public id for revocation, so
/// the bearer secret never leaves the server again.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccessTokenResp {
    pub user_id: u64,
    /// Public, non-secret identifier used by the revoke endpoint.
    pub token_id: String,
    /// Leading characters of the token, for display only.
    pub token_prefix: String,
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

    pub user_name: String,
    pub user_email: String,

    /// Whether this token may only be used on the metrics endpoint.
    #[serde(default)]
    pub metrics_only: bool,
}

impl AccessTokenResp {
    pub(crate) fn from_model(token: &AccessTokenModel, user: Option<&UserModel>) -> Self {
        Self {
            user_id: token.user_id,
            token_id: token.token_id.clone(),
            token_prefix: token.token_prefix.clone(),
            name: token.name.clone(),
            token_type: token.token_type.clone(),
            created_at: token.created_at,
            updated_at: token.updated_at,
            expire_at: token.expire_at,
            last_access_at: token.last_access_at,
            user_name: user
                .map(|u| u.username.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            user_email: user
                .map(|u| u.email.clone())
                .unwrap_or_else(|| "N/A".to_string()),
            metrics_only: token.metrics_only,
        }
    }
}
