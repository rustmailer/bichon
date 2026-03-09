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

use crate::modules::users::permissions::Permission;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::{
    bichon_version,
    modules::{
        account::migration::AccountModel,
        common::auth::ClientContext,
        error::{code::ErrorCode, BichonResult},
        indexer::manager::ENVELOPE_INDEX_MANAGER,
        settings::dir::DATA_DIR_MANAGER,
        utils::get_total_size,
    },
    raise_error,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct DashboardStats {
    pub account_count: usize,                  // Number of accounts
    pub email_count: u64,                      // Total number of emails
    pub total_size_bytes: u64,                 // Total size of all emails (in bytes)
    pub storage_usage_bytes: u64,              // Actual storage used (in bytes)
    pub index_usage_bytes: u64,                // Index storage size (in bytes)
    pub recent_activity: Vec<TimeBucket>,      // Email activity over recent days
    pub top_senders: Vec<Group>,               // Top 10 senders
    pub top_accounts: Vec<Group>,              // Top 10 accounts
    pub with_attachment_count: u64,            // Emails with attachments
    pub without_attachment_count: u64,         // Emails without attachments
    pub top_largest_emails: Vec<LargestEmail>, // Top 10 largest emails
    pub system_version: String, // The semantic version string of the currently running backend service
    pub commit_hash: String,    // Git commit hash used to build this system version
}

impl DashboardStats {
    pub async fn get(context: ClientContext) -> BichonResult<Self> {
        let has_all_accounts = context
            .has_permission(None, Permission::ACCOUNT_MANAGE_ALL)
            .await;

        let authorized_ids: Option<HashSet<u64>> = if has_all_accounts {
            None
        } else {
            Some(context.user.account_access_map.keys().cloned().collect())
        };

        let mut stat = ENVELOPE_INDEX_MANAGER
            .get_dashboard_stats(authorized_ids.clone())
            .await?;

        stat.top_largest_emails = ENVELOPE_INDEX_MANAGER
            .top_10_largest_emails(authorized_ids.clone())
            .await?;

        stat.account_count = if has_all_accounts {
            AccountModel::count().await?
        } else {
            authorized_ids.as_ref().map(|ids| ids.len()).unwrap_or(0)
        };

        stat.email_count = ENVELOPE_INDEX_MANAGER.total_emails(authorized_ids).await?;

        if has_all_accounts {
            stat.storage_usage_bytes = get_total_size(&DATA_DIR_MANAGER.eml_dir)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            stat.index_usage_bytes = get_total_size(&DATA_DIR_MANAGER.envelope_dir)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        } else {
            stat.storage_usage_bytes = 0;
            stat.index_usage_bytes = 0;
        }

        stat.system_version = bichon_version!().to_string();
        stat.commit_hash = env!("GIT_HASH").to_string();

        Ok(stat)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct TimeBucket {
    pub timestamp_ms: i64, // Timestamp in milliseconds
    pub count: u64,        // Number of emails in this time bucket
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct Group {
    pub key: String,
    pub count: u64, // Number of emails from this sender
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct LargestEmail {
    pub subject: String, // Email subject
    pub size_bytes: u64, // Email size in bytes
    pub id: u64,
}
