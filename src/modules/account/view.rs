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

use std::collections::{BTreeSet, HashMap};

use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::modules::{
    account::{
        entity::ImapConfig,
        migration::{AccountModel, AccountType},
        since::{DateSince, RelativeDate},
    },
    users::UserModel,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AccountResp {
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub sync_batch_size: Option<u32>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub created_user_name: String,
    pub created_user_email: String,
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
}

impl AccountResp {
    pub fn from_model(account: AccountModel, user_map: &HashMap<u64, UserModel>) -> AccountResp {
        let user = user_map.get(&account.created_by);
        AccountResp {
            id: account.id,
            imap: account.imap,
            enabled: account.enabled,
            email: account.email,
            name: account.name,
            capabilities: account.capabilities,
            date_since: account.date_since,
            date_before: account.date_before,
            folder_limit: account.folder_limit,
            sync_folders: account.sync_folders,
            account_type: account.account_type,
            sync_interval_min: account.sync_interval_min,
            sync_batch_size: account.sync_batch_size,
            known_folders: account.known_folders,
            created_at: account.created_at,
            updated_at: account.updated_at,
            created_by: account.created_by,
            created_user_name: user
                .map(|u| u.username.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            created_user_email: user
                .map(|u| u.email.clone())
                .unwrap_or_else(|| "N/A".to_string()),
            use_proxy: account.use_proxy,
            use_dangerous: account.use_dangerous,
            pgp_key: account.pgp_key,
        }
    }
}
