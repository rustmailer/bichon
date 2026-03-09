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

use crate::modules::account::entity::ImapConfig;
use crate::modules::account::migration::{AccountModel, AccountType};
use crate::modules::account::since::{DateSince, RelativeDate};
use crate::modules::error::code::ErrorCode;
use crate::modules::error::BichonResult;
use crate::{raise_error, validate_email};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AccountCreateRequest {
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: String,
    pub name: Option<String>,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub account_type: AccountType,
    #[oai(validator(minimum(value = "100")))]
    pub folder_limit: Option<u32>,
    #[oai(validator(minimum(value = "10")))]
    pub sync_interval_min: Option<i64>,
    #[oai(validator(minimum(value = "10"), maximum(value = "200")))]
    pub sync_batch_size: Option<u32>,
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
    pub imap_daily_quota_bytes: Option<u32>,
    pub auto_sync_new_mailboxes: Option<bool>,
}

impl AccountCreateRequest {
    pub fn create_entity(self, user_id: u64) -> BichonResult<AccountModel> {
        if self.date_before.is_some() && self.date_since.is_some() {
            return Err(raise_error!(
                "date_before and date_since are mutually exclusive; specify only one time boundary"
                    .into(),
                ErrorCode::InvalidParameter
            ));
        }

        if let Some(date_since) = self.date_since.as_ref() {
            date_since.validate()?;
        }

        if let Some(date_before) = self.date_before.as_ref() {
            date_before.validate_date()?;
        }

        match self.account_type {
            AccountType::IMAP => {
                match &self.imap {
                    Some(imap) => Self::validate_request(imap, &self.email)?,
                    None => {
                        return Err(raise_error!(
                            "IMAP configuration is required for IMAP account type".into(),
                            ErrorCode::InvalidParameter
                        ))
                    }
                }
                if self.sync_interval_min.is_none() {
                    return Err(raise_error!(
                        "`sync_interval_min` is required for IMAP account type".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
            AccountType::NoSync => {}
        }
        Ok(AccountModel::new(user_id, self)?)
    }

    fn validate_request(imap: &ImapConfig, email: &str) -> BichonResult<()> {
        imap.auth
            .validate()
            .map_err(|e| raise_error!(e.to_owned(), ErrorCode::InvalidParameter))?;
        validate_email!(email)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AccountUpdateRequest {
    pub email: Option<String>,
    /// Represents the account activation status.
    ///
    /// If this value is `false`, all account-related resources will be unavailable
    /// and any attempts to access them should return an error indicating the account
    /// is inactive.
    pub enabled: Option<bool>,
    /// Display name for the account (optional)
    pub name: Option<String>,
    /// IMAP server configuration
    pub imap: Option<ImapConfig>,
    /// Controls initial synchronization time range
    ///
    /// When dealing with large mailboxes, this restricts scanning to:
    /// - Messages after specified starting point
    /// - Or within sliding window
    ///
    /// ### Use Cases
    /// - Event-driven systems (only sync recent actionable emails)
    /// - First-time sync optimization for large accounts
    /// - Reducing server load during resyncs
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub clear_date_range: Option<bool>,
    /// Max emails to sync for this folder.  
    /// If not set, sync all emails.  
    /// otherwise sync up to `n` most recent emails (min 10).
    #[oai(validator(minimum(value = "100")))]
    pub folder_limit: Option<u32>,
    pub clear_folder_limit: Option<bool>,
    /// Configuration for selective folder (mailbox/label) synchronization
    ///
    /// - For IMAP/SMTP accounts:
    ///   Stores the mailbox names, since IMAP mailboxes do not have stable IDs.
    ///   Synchronization is keyed by the folder name.
    ///
    /// - For Gmail API accounts:
    ///   A Gmail label is treated as a mailbox (model mapping).
    ///   Since label names can be easily changed, the stable `labelId` is recorded here
    ///   instead of the label name.
    ///
    /// Defaults to standard folders (`INBOX`, `Sent`) if empty.
    /// Modified folders will be automatically synced on the next update.
    pub sync_folders: Option<Vec<String>>,
    /// Incremental sync interval (seconds)
    #[oai(validator(minimum(value = "10")))]
    pub sync_interval_min: Option<i64>,
    #[oai(validator(minimum(value = "10"), maximum(value = "200")))]
    pub sync_batch_size: Option<u32>,
    /// Optional proxy ID for establishing the connection to external APIs (e.g., Gmail, Outlook).
    /// - If `None` or not provided, the client will connect directly to the API server.
    /// - If `Some(proxy_id)`, the client will use the pre-configured proxy with the given ID for API requests.
    pub use_proxy: Option<u64>,

    pub use_dangerous: Option<bool>,

    pub pgp_key: Option<String>,
    pub imap_daily_quota_bytes: Option<u32>,
    pub auto_sync_new_mailboxes: Option<bool>,
}

impl AccountUpdateRequest {
    pub fn validate_update_request(&self, account: &AccountModel) -> BichonResult<()> {
        if self.date_before.is_some() && self.date_since.is_some() {
            return Err(raise_error!(
                "date_before and date_since are mutually exclusive; specify only one time boundary"
                    .into(),
                ErrorCode::InvalidParameter
            ));
        }

        if self.clear_folder_limit == Some(true) && self.folder_limit.is_some() {
            return Err(raise_error!(
                "clear_folder_limit cannot be combined with folder_limit".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if self.clear_date_range == Some(true)
            && (self.date_since.is_some() || self.date_before.is_some())
        {
            return Err(raise_error!(
                "clear_date_range cannot be combined with date_since or date_before".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if let Some(date_since) = self.date_since.as_ref() {
            date_since.validate()?;
        }

        if let Some(date_before) = self.date_before.as_ref() {
            date_before.validate_date()?;
        }

        if matches!(account.account_type, AccountType::IMAP) {
            if let Some(mailboxes) = self.sync_folders.as_ref() {
                if mailboxes.is_empty() {
                    return Err(raise_error!(
                    "Invalid configuration: 'sync_folders' cannot be empty. \
                     If you are modifying the subscription list, please provide at least one mailbox to subscribe to.".into(), ErrorCode::InvalidParameter
                ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]

pub struct MinimalAccount {
    pub id: u64,
    pub email: String,
}

pub fn filter_accessible_accounts<'a>(
    all_accounts: &'a [MinimalAccount],
    allowed: &Vec<u64>,
) -> Vec<MinimalAccount> {
    all_accounts
        .iter()
        .filter(|acct| allowed.contains(&acct.id))
        .cloned()
        .collect()
}
