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

use std::str::FromStr;

use crate::account::entity::{AuthType, ImapConfig};
use crate::account::migration::{
    AccountKind, AccountKindRole, AccountModel, AccountType, ArchiveRules, ExtractionRules,
    ProviderKind, QuotaWindow, RemoteCleanupConfig, ViewConfig, REMOTE_CLEANUP_FOLDER,
};
use crate::account::since::{DateSince, RelativeDate};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::{raise_error, validate_email};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccountCreateRequest {
    #[cfg_attr(
        feature = "web-api",
        oai(validator(custom = "crate::common::validator::EmailValidator"))
    )]
    pub email: String,
    pub login_name: Option<String>,
    pub account_name: Option<String>,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub account_type: AccountType,
    /// Tenant-level provider connection for `Graph`/`Gmail` accounts.
    ///
    /// `None` means account-level credentials: IMAP password/OAuth2, or a
    /// delegated single-mailbox provider token. IMAP, `NoSync` and `View`
    /// accounts must leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<u64>,
    #[cfg_attr(feature = "web-api", oai(validator(minimum(value = "1"))))]
    pub download_interval_min: Option<i64>,
    #[cfg_attr(
        feature = "web-api",
        oai(validator(minimum(value = "10"), maximum(value = "200")))
    )]
    pub download_batch_size: Option<u32>,
    pub max_email_size_bytes: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
    pub imap_quota_bytes: Option<u64>,
    pub imap_quota_window: Option<QuotaWindow>,
    pub auto_download_new_mailboxes: Option<bool>,
    pub download_schedule: Option<String>,
    /// Email archive filtering rules (Pro feature).
    /// `None` = archive everything (backward compatible).
    pub archive_rules: Option<ArchiveRules>,
    /// Attachment text extraction rules (Pro feature).
    /// `None` = extract everything (backward compatible).
    pub extraction_rules: Option<ExtractionRules>,
    /// Ordered header names whose first valid mailbox replaces the stored
    /// `from` address. `None` = disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_from_headers: Option<Vec<String>>,
    /// Account-level retention period in days (free/community feature).
    /// `None` or `0` = disabled (archive everything, never auto-purge).
    pub retention_days: Option<u64>,
    /// Minimum retention in days (Enterprise compliance). Messages younger
    /// than this window cannot be deleted by any path. `None` or `0` =
    /// disabled. Only `system:root` may set this.
    #[serde(default)]
    pub min_retention_days: Option<u64>,
    /// Source-mailbox cleanup (community feature). `None` = not configured.
    #[serde(default)]
    pub remote_cleanup: Option<RemoteCleanupConfig>,
    /// Special role marker. It is set internally when the journaling
    /// configuration creates its archive account; a client may not choose it.
    ///
    /// `None` — the default, and what every client that predates this field
    /// sends — is an ordinary account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AccountKind>,
    /// Sources and per-source filters for an `AccountType::View` account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewConfig>,
}

impl AccountCreateRequest {
    pub fn create_entity(mut self, user_id: u64) -> BichonResult<AccountModel> {
        if self.date_before.is_some() && self.date_since.is_some() {
            return Err(raise_error!(
                "date_before and date_since are mutually exclusive; specify only one time boundary"
                    .into(),
                ErrorCode::InvalidParameter
            ));
        }

        if self.imap_quota_bytes.is_some() ^ self.imap_quota_window.is_some() {
            return Err(raise_error!(
                "Quota bytes and quota window must be provided together or omitted together".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if let Some(date_since) = self.date_since.as_ref() {
            date_since.validate()?;
        }

        if let Some(date_before) = self.date_before.as_ref() {
            date_before.validate_date()?;
        }

        if let Some(cleanup) = &self.remote_cleanup {
            if !matches!(self.account_type, AccountType::IMAP) {
                return Err(raise_error!(
                    "Source-mailbox cleanup is only supported for IMAP accounts".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            cleanup.validate().map_err(|e| {
                raise_error!(format!("remote_cleanup: {e}"), ErrorCode::InvalidParameter)
            })?;
        }

        if self.connection_id.is_some()
            && !matches!(
                self.account_type,
                AccountType::Graph | AccountType::Gmail
            )
        {
            return Err(raise_error!(
                "connection_id is only valid for Graph or Gmail accounts".into(),
                ErrorCode::InvalidParameter
            ));
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
                if self.download_interval_min.is_none() && self.download_schedule.is_none() {
                    return Err(raise_error!(
                        "`sync_interval_min` or `download_schedule` is required for IMAP account type".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if let Some(ref schedule) = self.download_schedule {
                    validate_cron_expression(schedule)?;
                }
            }
            // Provider accounts never carry an IMAP server config. The
            // connection (or, for delegated accounts, the per-account OAuth2
            // token) supplies credentials; sync cadence is still per account.
            AccountType::Graph | AccountType::Gmail => {
                validate_email!(&self.email)?;
                if self.imap.is_some() {
                    return Err(raise_error!(
                        "IMAP configuration must not be set for Graph or Gmail accounts".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.download_interval_min.is_none() && self.download_schedule.is_none() {
                    return Err(raise_error!(
                        "`sync_interval_min` or `download_schedule` is required for provider accounts".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if let Some(ref schedule) = self.download_schedule {
                    validate_cron_expression(schedule)?;
                }
            }
            // A view stores nothing and must not carry sync configuration;
            // its validation happens below.
            AccountType::View | AccountType::NoSync => {}
        }

        // A build without the connector must not accept an account it can
        // never sync. The type and field still exist so Pro data stays
        // readable; creation is what is gated.
        if let Some(kind) = match self.account_type {
            AccountType::Graph => Some(ProviderKind::Graph),
            AccountType::Gmail => Some(ProviderKind::Gmail),
            _ => None,
        } {
            if !crate::ext::provider::is_available(kind) {
                return Err(crate::ext::provider::unavailable_error(kind));
            }
        }

        // The journal role is created by the journaling configuration, never
        // chosen by a client.
        if self.kind.is_some() {
            return Err(raise_error!(
                "This account role is reserved for the SMTP journaling configuration".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if matches!(self.account_type, AccountType::View) {
            // Pro/Enterprise only. The type exists in the data model
            // everywhere so rows stay readable, but a community installation
            // must not be able to create one — this is checked here, not only
            // by a hidden WebUI field.
            if !crate::ext::account_roles::view_role_enabled() {
                return Err(raise_error!(
                    "View accounts require the Pro or Enterprise edition".into(),
                    ErrorCode::Forbidden
                ));
            }
            let sources = self.view.as_ref().map(|v| v.sources.as_slice()).unwrap_or(&[]);
            validate_view_sources(sources)?;
        } else if self.view.is_some() {
            return Err(raise_error!(
                "Only a view account may carry view sources".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if let Some(ref rules) = self.extraction_rules {
            rules.validate().map_err(|e| {
                raise_error!(
                    format!("extraction_rules: {}", e),
                    ErrorCode::InvalidParameter
                )
            })?;
        }
        if let Some(ref rules) = self.archive_rules {
            rules.validate().map_err(|e| {
                raise_error!(format!("archive_rules: {}", e), ErrorCode::InvalidParameter)
            })?;
        }
        self.override_from_headers = normalize_override_from_headers(&self.override_from_headers)
            .map_err(|e| {
                raise_error!(format!("override_from_headers: {e}"), ErrorCode::InvalidParameter)
            })?;
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccountUpdateRequest {
    pub email: Option<String>,
    /// Represents the account activation status.
    ///
    /// If this value is `false`, all account-related resources will be unavailable
    /// and any attempts to access them should return an error indicating the account
    /// is inactive.
    pub enabled: Option<bool>,
    pub account_name: Option<String>,
    /// IMAP server configuration
    pub imap: Option<ImapConfig>,
    /// Tenant-level provider connection for a `Graph`/`Gmail` account.
    /// `None` = no change. Only valid on an existing provider account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<u64>,
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
    /// Incremental download interval (seconds)
    #[cfg_attr(feature = "web-api", oai(validator(minimum(value = "1"))))]
    pub download_interval_min: Option<i64>,
    #[cfg_attr(
        feature = "web-api",
        oai(validator(minimum(value = "10"), maximum(value = "200")))
    )]
    pub download_batch_size: Option<u32>,
    pub max_email_size_bytes: Option<u64>,
    pub use_dangerous: Option<bool>,

    pub pgp_key: Option<String>,
    pub imap_quota_bytes: Option<u64>,
    pub imap_quota_window: Option<QuotaWindow>,
    pub auto_download_new_mailboxes: Option<bool>,
    pub download_schedule: Option<String>,
    pub clear_download_schedule: Option<bool>,
    /// Clear archive filtering rules back to `None` (default behavior).
    /// Cannot be combined with `archive_rules`.
    pub clear_archive_rules: Option<bool>,
    /// Clear attachment extraction rules back to `None` (default behavior).
    /// Cannot be combined with `extraction_rules`.
    pub clear_extraction_rules: Option<bool>,
    /// Email archive filtering rules (Pro feature).
    /// `None` = no change. Use `Some(ArchiveRules { .. })` to set.
    pub archive_rules: Option<ArchiveRules>,
    /// Attachment text extraction rules (Pro feature).
    /// `None` = no change. Use `Some(ExtractionRules { .. })` to set.
    pub extraction_rules: Option<ExtractionRules>,
    /// Ordered header names whose first valid mailbox replaces the stored
    /// `from` address. `None` = no change; an empty list disables the rule.
    /// Cannot be combined with `clear_override_from_headers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_from_headers: Option<Vec<String>>,
    /// Clear the header override rule list.
    #[serde(default)]
    pub clear_override_from_headers: Option<bool>,
    /// Account-level retention period in days (free/community feature).
    /// `Some(0)` disables retention; `None` means no change. A background
    /// sweep purges envelopes older than the window (held accounts exempt).
    pub retention_days: Option<u64>,
    /// Clear retention back to `None` (disabled). Cannot be combined with
    /// `retention_days`.
    pub clear_retention_days: Option<bool>,
    /// Minimum retention in days (Enterprise compliance). `Some(0)` disables
    /// the deletion lock; `None` means no change. Only `system:root` may
    /// change it.
    #[serde(default)]
    pub min_retention_days: Option<u64>,
    /// Clear the minimum-retention lock. Cannot be combined with
    /// `min_retention_days`.
    #[serde(default)]
    pub clear_min_retention_days: Option<bool>,
    /// Source-mailbox cleanup (community feature). `None` = no change.
    #[serde(default)]
    pub remote_cleanup: Option<RemoteCleanupConfig>,
    /// Clear the source-mailbox cleanup configuration. Cannot be combined with
    /// `remote_cleanup`.
    #[serde(default)]
    pub clear_remote_cleanup: Option<bool>,
    /// Replace a `View` account's sources and filters. `None` = no change.
    /// Only meaningful on an `AccountType::View` account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewConfig>,
}

/// Dedicated retention-policy change request.
///
/// Retention fields are no longer accepted on the general account-update
/// endpoint: a weakening change may need a second signature, and a dedicated
/// endpoint lets the approval freeze exactly those two values instead of an
/// arbitrary account payload.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct RetentionPolicyRequest {
    /// New maximum-retention window. `None` = no change; `0` = disabled.
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Clear the maximum-retention window. Cannot be combined with
    /// `retention_days`.
    #[serde(default)]
    pub clear_retention_days: Option<bool>,
    /// New minimum-retention lock. `None` = no change; `0` = disabled.
    #[serde(default)]
    pub min_retention_days: Option<u64>,
    /// Clear the minimum-retention lock. Cannot be combined with
    /// `min_retention_days`.
    #[serde(default)]
    pub clear_min_retention_days: Option<bool>,
    /// Optional reason, shown to the approver under dual control.
    #[serde(default)]
    pub reason: Option<String>,
}

impl RetentionPolicyRequest {
    /// Resolve the requested values against the account's current policy.
    /// `Err` when a field and its `clear_*` flag are both set.
    pub fn resolve(
        &self,
        before_retention: Option<u64>,
        before_min: Option<u64>,
    ) -> BichonResult<(Option<u64>, Option<u64>)> {
        fn normalize(value: Option<u64>) -> Option<u64> {
            value.filter(|days| *days > 0)
        }
        if self.clear_retention_days == Some(true) && self.retention_days.is_some() {
            return Err(raise_error!(
                "clear_retention_days cannot be combined with retention_days".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.clear_min_retention_days == Some(true) && self.min_retention_days.is_some() {
            return Err(raise_error!(
                "clear_min_retention_days cannot be combined with min_retention_days".into(),
                ErrorCode::InvalidParameter
            ));
        }
        let retention = if self.clear_retention_days == Some(true) {
            None
        } else if self.retention_days.is_some() {
            normalize(self.retention_days)
        } else {
            normalize(before_retention)
        };
        let min = if self.clear_min_retention_days == Some(true) {
            None
        } else if self.min_retention_days.is_some() {
            normalize(self.min_retention_days)
        } else {
            normalize(before_min)
        };
        Ok((retention, min))
    }
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

        if self.imap_quota_bytes.is_some() ^ self.imap_quota_window.is_some() {
            return Err(raise_error!(
                "Quota bytes and quota window must be provided together or omitted together".into(),
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

        if self.clear_archive_rules == Some(true) && self.archive_rules.is_some() {
            return Err(raise_error!(
                "clear_archive_rules cannot be combined with archive_rules".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.clear_extraction_rules == Some(true) && self.extraction_rules.is_some() {
            return Err(raise_error!(
                "clear_extraction_rules cannot be combined with extraction_rules".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.clear_override_from_headers == Some(true)
            && self.override_from_headers.is_some()
        {
            return Err(raise_error!(
                "clear_override_from_headers cannot be combined with override_from_headers".into(),
                ErrorCode::InvalidParameter
            ));
        }
        normalize_override_from_headers(&self.override_from_headers).map_err(|e| {
            raise_error!(format!("override_from_headers: {e}"), ErrorCode::InvalidParameter)
        })?;
        if let Some(view) = &self.view {
            if !matches!(account.account_type, AccountType::View) {
                return Err(raise_error!(
                    "Only a view account may carry view sources".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            // Same edition/live-licence gate as creation: a lapsed licence
            // must not be able to widen an existing view's scope either.
            if !crate::ext::account_roles::view_role_enabled() {
                return Err(raise_error!(
                    "View accounts require the Pro or Enterprise edition".into(),
                    ErrorCode::Forbidden
                ));
            }
            validate_view_sources(&view.sources)?;
        }

        if self.clear_retention_days == Some(true) && self.retention_days.is_some() {
            return Err(raise_error!(
                "clear_retention_days cannot be combined with retention_days".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if self.clear_min_retention_days == Some(true) && self.min_retention_days.is_some() {
            return Err(raise_error!(
                "clear_min_retention_days cannot be combined with min_retention_days".into(),
                ErrorCode::InvalidParameter
            ));
        }

        if self.clear_remote_cleanup == Some(true) && self.remote_cleanup.is_some() {
            return Err(raise_error!(
                "clear_remote_cleanup cannot be combined with remote_cleanup".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if let Some(cleanup) = &self.remote_cleanup {
            if !matches!(account.account_type, AccountType::IMAP) {
                return Err(raise_error!(
                    "Source-mailbox cleanup is only supported for IMAP accounts".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            cleanup.validate().map_err(|e| {
                raise_error!(format!("remote_cleanup: {e}"), ErrorCode::InvalidParameter)
            })?;
        }

        if self.connection_id.is_some()
            && !matches!(
                account.account_type,
                AccountType::Graph | AccountType::Gmail
            )
        {
            return Err(raise_error!(
                "connection_id is only valid for Graph or Gmail accounts".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.imap.is_some()
            && matches!(
                account.account_type,
                AccountType::Graph | AccountType::Gmail
            )
        {
            return Err(raise_error!(
                "IMAP configuration must not be set for Graph or Gmail accounts".into(),
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
                if mailboxes
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(REMOTE_CLEANUP_FOLDER))
                {
                    return Err(raise_error!(
                        format!(
                            "'{REMOTE_CLEANUP_FOLDER}' is reserved for cleanup output and cannot be a sync folder"
                        ),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
            if self.clear_download_schedule == Some(true) && self.download_schedule.is_some() {
                return Err(raise_error!(
                    "clear_download_schedule cannot be combined with download_schedule".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            if let Some(ref schedule) = self.download_schedule {
                validate_cron_expression(schedule)?;
            }
        }
        if let Some(ref rules) = self.extraction_rules {
            rules.validate().map_err(|e| {
                raise_error!(
                    format!("extraction_rules: {}", e),
                    ErrorCode::InvalidParameter
                )
            })?;
        }
        if let Some(ref rules) = self.archive_rules {
            rules.validate().map_err(|e| {
                raise_error!(format!("archive_rules: {}", e), ErrorCode::InvalidParameter)
            })?;
        }
        Ok(())
    }
}

/// Validate and normalize an `override_from_headers` rule list.
///
/// `None` stays `None` (feature off). An empty list is normalized to `None`,
/// so "enabled but nothing configured" cannot become a distinct stored state.
/// Duplicates are removed case-insensitively while preserving precedence.
pub(crate) fn normalize_override_from_headers(
    headers: &Option<Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut normalized: Vec<String> = Vec::with_capacity(headers.len());
    for header in headers {
        let header = header.trim();
        if header.is_empty() {
            return Err("header name must not be empty".to_string());
        }
        // RFC 5322 field-name: printable US-ASCII except ':'; this also
        // rejects whitespace, CR/LF and anything that could be injected.
        if !header.bytes().all(|b| matches!(b, 33..=57 | 59..=126)) {
            return Err(format!("invalid header name '{header}'"));
        }
        if !normalized
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(header))
        {
            normalized.push(header.to_string());
        }
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn validate_cron_expression(expr: &str) -> BichonResult<()> {
    if expr.trim().is_empty() {
        return Err(raise_error!(
            "download_schedule must not be empty".into(),
            ErrorCode::InvalidParameter
        ));
    }
    cron::Schedule::from_str(expr).map_err(|e| {
        raise_error!(
            format!("Invalid cron expression '{}': {}", expr, e),
            ErrorCode::InvalidParameter
        )
    })?;
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]

pub struct MinimalAccount {
    pub id: u64,
    pub email: String,
    pub name: Option<String>,
    /// The IMAP authentication method of the account, or `None` for accounts that have
    /// no IMAP configuration at all (e.g. `NoSync`). `#[serde(default)]` keeps this
    /// readable from a server that predates the field.
    #[serde(default)]
    pub auth_type: Option<AuthType>,
    /// The storage type (`IMAP`, `NoSync`, `View`).
    ///
    /// On the minimal list so the UI can tell a view apart from a local
    /// account without loading the full account.
    pub account_type: AccountType,
    /// The only special role today: `journal` (the SMTP journaling archive).
    /// Absent for ordinary accounts and for views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AccountKindRole>,
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


/// Validate a view's sources: at least one, no duplicates, every source must
/// exist and must not itself be a view, and each filter must be an address.
///
/// Shared by creation and update so a view cannot be created valid and then
/// edited into an invalid state (or vice versa).
pub(crate) fn validate_view_sources(
    sources: &[crate::account::migration::ViewSource],
) -> BichonResult<()> {
    if sources.is_empty() {
        return Err(raise_error!(
            "A view account needs at least one source account".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for source in sources {
        if !seen.insert(source.account_id) {
            return Err(raise_error!(
                format!("View source account {} is listed twice", source.account_id),
                ErrorCode::InvalidParameter
            ));
        }
        let account = AccountModel::get(source.account_id).map_err(|_| {
            raise_error!(
                format!("View source account {} does not exist", source.account_id),
                ErrorCode::InvalidParameter
            )
        })?;
        if account.is_view_account() {
            return Err(raise_error!(
                format!("Account {} is itself a view; views cannot chain", source.account_id),
                ErrorCode::InvalidParameter
            ));
        }
        for address in source
            .any_participant
            .iter()
            .chain(source.any_recipient.iter())
        {
            validate_filter_address(address, source.account_id)?;
        }
    }
    Ok(())
}

/// A view filter address must be a bare e-mail address.
///
/// The filter is evaluated with **exact** equality — in the index query and in
/// the per-message read check — so anything that is not one address matches
/// nothing. A `contains('@')` test accepted `Zhang Wei <zhang@firma.de>` and
/// `a@b.de, c@d.de` (both reachable through the picker's free-text entry) and
/// the view then silently exposed an empty result set. Refusing it names the
/// problem while the operator is looking at the field.
fn validate_filter_address(address: &str, account_id: u64) -> BichonResult<()> {
    let address = address.trim();
    // The function, not the `validate_email!` macro imported at the top of
    // this module.
    crate::utils::validate_email(address).map_err(|_| {
        raise_error!(
            format!(
                "View filter '{}' on account {} is not an e-mail address",
                address, account_id
            ),
            ErrorCode::InvalidParameter
        )
    })
}

#[cfg(test)]
mod test {
    use super::{validate_cron_expression, validate_filter_address, MinimalAccount};
    use crate::account::entity::AuthType;

    /// A filter that is not a bare address can never match, so it is refused at
    /// the door rather than stored as a view that shows nothing.
    #[test]
    fn view_filter_addresses_must_be_bare_addresses() {
        for good in [
            "zhang@firma.de",
            "  JÜRGEN@firma.de  ",
            "a.b+tag@sub.example.co.uk",
        ] {
            assert!(
                validate_filter_address(good, 1).is_ok(),
                "{good} must be accepted"
            );
        }

        for bad in [
            // Display-name form and a list: both are what the free-text entry
            // produces, and both would match nothing.
            "Zhang Wei <zhang@firma.de>",
            "a@b.de, c@d.de",
            "zhang",
            "@firma.de",
            "zhang@",
            "",
        ] {
            assert!(
                validate_filter_address(bad, 1).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn valid_cron_expressions() {
        assert!(validate_cron_expression("0 0 0 * * *").is_ok()); // daily at midnight
        assert!(validate_cron_expression("0 */5 * * * *").is_ok()); // every 5 minutes
        assert!(validate_cron_expression("0 0 12 * * 1-5").is_ok()); // weekdays at noon
        assert!(validate_cron_expression("0 30 4 1 * *").is_ok()); // 1st of month at 04:30
        assert!(validate_cron_expression("0 0 * * * *").is_ok()); // every hour
    }

    #[test]
    fn invalid_cron_expression_too_few_fields() {
        assert!(validate_cron_expression("0 0 * *").is_err());
        assert!(validate_cron_expression("* * * * *").is_err()); // 5 fields, needs seconds
    }

    #[test]
    fn invalid_cron_expression_empty() {
        assert!(validate_cron_expression("").is_err());
        assert!(validate_cron_expression("   ").is_err());
    }

    #[test]
    fn invalid_cron_expression_garbage() {
        assert!(validate_cron_expression("not a cron").is_err());
    }

    /// A `bichon-cli` built from this revision may talk to a server that predates
    /// `auth_type`; the response must still parse.
    #[test]
    fn minimal_account_parses_without_auth_type() {
        let account: MinimalAccount =
            serde_json::from_str(r#"{"id":7,"email":"a@example.com","name":"a","account_type":"NoSync"}"#).unwrap();
        assert_eq!(account.id, 7);
        assert_eq!(account.email, "a@example.com");
        assert_eq!(account.name.as_deref(), Some("a"));
        assert_eq!(account.auth_type, None);
    }

    #[test]
    fn minimal_account_parses_auth_type() {
        let account: MinimalAccount = serde_json::from_str(
            r#"{"id":7,"email":"a@example.com","name":null,"auth_type":"OAuth2","account_type":"IMAP"}"#,
        )
        .unwrap();
        assert_eq!(account.auth_type, Some(AuthType::OAuth2));

        // NoSync accounts carry no IMAP config, so the field is explicitly null.
        let account: MinimalAccount =
            serde_json::from_str(r#"{"id":8,"email":"b@example.com","auth_type":null,"account_type":"NoSync"}"#).unwrap();
        assert_eq!(account.auth_type, None);
    }

    // ---- provider account model / validation --------------------------------

    use super::{AccountCreateRequest, AccountUpdateRequest, AccountType, ProviderKind};
    use crate::account::migration::{Account, ProviderKind as MigrationProviderKind};
    use crate::ext::provider::{is_available, register_connector, ProviderConnector};
    use std::sync::Arc;

    struct NoopConnector(ProviderKind);

    impl ProviderConnector for NoopConnector {
        fn kind(&self) -> ProviderKind {
            self.0
        }
        fn start_account_sync(&self, _account: &Account) -> crate::error::BichonResult<()> {
            Ok(())
        }
    }

    /// Provider creation is gated on a registered connector. Tests that need a
    /// provider account to pass that gate register a no-op implementation.
    fn ensure_provider_connectors() {
        for kind in [ProviderKind::Graph, ProviderKind::Gmail] {
            if !is_available(kind) {
                register_connector(Arc::new(NoopConnector(kind)));
            }
        }
    }

    fn graph_request() -> AccountCreateRequest {
        AccountCreateRequest {
            email: "user@example.com".into(),
            account_type: AccountType::Graph,
            connection_id: Some(7),
            download_interval_min: Some(15),
            ..Default::default()
        }
    }

    #[test]
    fn connection_id_is_only_for_provider_accounts() {
        for account_type in [AccountType::NoSync, AccountType::View] {
            let request = AccountCreateRequest {
                email: "user@example.com".into(),
                account_type,
                connection_id: Some(7),
                ..Default::default()
            };
            let err = request.create_entity(1).unwrap_err();
            assert!(format!("{err:?}").contains("connection_id is only valid"));
        }
    }

    #[test]
    fn provider_account_rejects_imap_config() {
        ensure_provider_connectors();
        let mut request = graph_request();
        request.imap = Some(crate::account::entity::ImapConfig::default());
        let err = request.create_entity(1).unwrap_err();
        assert!(format!("{err:?}").contains("IMAP configuration must not be set"));
    }

    #[test]
    fn provider_account_requires_sync_cadence() {
        ensure_provider_connectors();
        let mut request = graph_request();
        request.download_interval_min = None;
        let err = request.create_entity(1).unwrap_err();
        assert!(format!("{err:?}").contains("sync_interval_min"));
    }

    #[test]
    fn provider_account_accepts_tenant_connection_and_delegated_auth() {
        ensure_provider_connectors();

        let tenant = graph_request().create_entity(1).unwrap();
        assert_eq!(tenant.account_type, AccountType::Graph);
        assert_eq!(tenant.connection_id, Some(7));
        assert!(tenant.imap.is_none());
        assert!(tenant.is_provider_account());
        assert_eq!(
            tenant.provider_kind(),
            Some(MigrationProviderKind::Graph)
        );

        let mut delegated = graph_request();
        delegated.connection_id = None;
        let delegated = delegated.create_entity(1).unwrap();
        assert_eq!(delegated.connection_id, None);
        assert_eq!(delegated.account_type, AccountType::Graph);
    }

    #[test]
    fn update_rejects_connection_id_for_imap_and_allows_it_for_provider() {
        let imap = Account {
            id: 1,
            email: "user@example.com".into(),
            account_type: AccountType::IMAP,
            ..Default::default()
        };
        let request = AccountUpdateRequest {
            connection_id: Some(9),
            ..Default::default()
        };
        assert!(request.validate_update_request(&imap).is_err());

        let graph = Account {
            id: 2,
            email: "user@example.com".into(),
            account_type: AccountType::Graph,
            connection_id: Some(7),
            ..Default::default()
        };
        let request = AccountUpdateRequest {
            connection_id: Some(9),
            ..Default::default()
        };
        assert!(request.validate_update_request(&graph).is_ok());
    }

    #[test]
    fn account_type_and_connection_id_serialise_additively() {
        ensure_provider_connectors();
        let account = graph_request().create_entity(1).unwrap();
        let mut value = serde_json::to_value(&account).unwrap();
        assert_eq!(value["account_type"], "Graph");
        assert_eq!(value["connection_id"], 7);

        // Older rows have no `connection_id`; deserialising them must yield
        // `None` rather than fail.
        value.as_object_mut().unwrap().remove("connection_id");
        let restored: Account = serde_json::from_value(value).unwrap();
        assert_eq!(restored.connection_id, None);
        assert_eq!(restored.account_type, AccountType::Graph);
    }
}

#[cfg(test)]
mod override_from_headers_tests {
    use super::normalize_override_from_headers;

    #[test]
    fn none_and_empty_mean_disabled() {
        assert_eq!(normalize_override_from_headers(&None).unwrap(), None);
        assert_eq!(normalize_override_from_headers(&Some(vec![])).unwrap(), None);
        assert_eq!(
            normalize_override_from_headers(&Some(vec!["  ".into()])),
            Err("header name must not be empty".into())
        );
    }

    #[test]
    fn trims_and_deduplicates_case_insensitively() {
        let got = normalize_override_from_headers(&Some(vec![
            " X-AnonAddy-Original-Sender ".into(),
            "x-anonaddy-original-sender".into(),
            "X-SimpleLogin-Original-From".into(),
        ]))
        .unwrap();
        assert_eq!(
            got,
            Some(vec![
                "X-AnonAddy-Original-Sender".into(),
                "X-SimpleLogin-Original-From".into(),
            ])
        );
    }

    #[test]
    fn rejects_injection_and_non_field_names() {
        for bad in ["X-Test: value", "X-Test\r\nInjected", "X Test", "X-Test\u{0007}"] {
            assert!(normalize_override_from_headers(&Some(vec![bad.into()])).is_err(), "{bad:?}");
        }
    }
}
