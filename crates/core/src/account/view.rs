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

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

use crate::{
    account::{
        entity::ImapConfig,
        migration::{AccountModel, AccountType, ArchiveRules, ExtractionRules, QuotaWindow},
        since::{DateSince, RelativeDate},
    },
    users::UserModel,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccountResp {
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub email: String,
    pub account_name: Option<String>,
    pub login_name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub download_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub download_interval_min: Option<i64>,
    pub download_batch_size: Option<u32>,
    pub max_email_size_bytes: Option<u64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub created_user_name: String,
    pub created_user_email: String,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
    pub imap_quota_bytes: Option<u64>,
    pub imap_quota_window: Option<QuotaWindow>,
    pub auto_download_new_mailboxes: Option<bool>,
    pub download_schedule: Option<String>,
    pub archive_rules: Option<ArchiveRules>,
    /// Attachment text extraction rules (Pro feature); `None` = extract everything.
    pub extraction_rules: Option<ExtractionRules>,
    /// Header names whose first valid mailbox replaces the stored sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_from_headers: Option<Vec<String>>,
    /// Account-level retention window in days (free/community). `None`/`0` = keep everything.
    pub retention_days: Option<u64>,
    /// Source-mailbox cleanup configuration (community); `None` = not configured.
    #[serde(default)]
    pub remote_cleanup: Option<crate::account::migration::RemoteCleanupConfig>,
    /// Legal hold flag (Enterprise); while set the retention sweep and bulk deletion skip this account.
    #[serde(default)]
    pub legal_hold: bool,
    /// Reason recorded when the hold was placed.
    pub hold_reason: Option<String>,
    /// User id that placed the hold.
    pub hold_placed_by: Option<u64>,
    /// Epoch millis when the hold was placed.
    pub hold_placed_at: Option<i64>,
    pub deleting: bool,
    /// Optional special role (journal); `None` = ordinary account or view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::account::migration::AccountKind>,
    /// Sources and filters of a `View` account; `None` for every other type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<crate::account::migration::ViewConfig>,
}

impl AccountResp {
    pub fn from_model(account: AccountModel, user_map: &HashMap<u64, UserModel>) -> AccountResp {
        let user = user_map.get(&account.created_by);
        AccountResp {
            id: account.id,
            imap: account.imap,
            enabled: account.enabled,
            email: account.email,
            account_name: account.account_name,
            login_name: account.login_name,
            capabilities: account.capabilities,
            date_since: account.date_since,
            date_before: account.date_before,
            download_folders: account.download_folders,
            account_type: account.account_type,
            download_interval_min: account.download_interval_min,
            download_batch_size: account.download_batch_size,
            max_email_size_bytes: account.max_email_size_bytes,
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
            use_dangerous: account.use_dangerous,
            pgp_key: account.pgp_key,
            imap_quota_bytes: account.imap_quota_bytes,
            imap_quota_window: account.imap_quota_window,
            auto_download_new_mailboxes: account.auto_download_new_mailboxes,
            download_schedule: account.download_schedule,
            archive_rules: account.archive_rules,
            extraction_rules: account.extraction_rules,
            override_from_headers: account.override_from_headers,
            retention_days: account.retention_days,
            remote_cleanup: account.remote_cleanup,
            legal_hold: account.legal_hold,
            hold_reason: account.hold_reason,
            hold_placed_by: account.hold_placed_by,
            hold_placed_at: account.hold_placed_at,
            deleting: account.deleting,
            kind: account.kind,
            view: account.view,
        }
    }
}
