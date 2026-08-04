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
use std::collections::BTreeSet;
use tracing::info;

use crate::{
    account::{
        entity::ImapConfig,
        payload::{AccountCreateRequest, AccountUpdateRequest, MinimalAccount},
        since::{DateSince, RelativeDate},
        state::DownloadState,
    },
    cache::imap::{mailbox::MailBox, task::SYNC_TASKS},
    common::paginated::DataPage,
    context::controller::DOWNLOAD_CONTROLLER,
    database::{
        count_impl, delete_impl, find_impl, insert_impl, list_all_impl, manager::DB_MANAGER,
        paginate_impl, update_impl, MemDbModel,
    },
    encrypt,
    error::{code::ErrorCode, BichonResult},
    id,
    oauth2::token::OAuth2AccessToken,
    raise_error,
    store::tantivy::envelope::ENVELOPE_MANAGER,
    users::{payload::UserUpdateRequest, role::DEFAULT_ACCOUNT_MANAGER_ROLE_ID, UserModel},
    utc_now,
};

pub type AccountModel = Account;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum AccountType {
    #[default]
    IMAP,
    NoSync,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum QuotaWindow {
    Hourly,
    #[default]
    Daily,
    Weekly,
    Monthly,
}

/// Include/exclude filter rule.
///
/// - `include` non-empty: only values matching these patterns pass.
/// - `exclude` non-empty: values matching these patterns are rejected.
/// - Both empty: all values pass.
/// - Both set: include checked first, then exclude.
///
/// Extension patterns use case-insensitive exact match; all others use regex.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct FilterRule {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl FilterRule {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    fn matches_exact(&self, value: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|e| e.eq_ignore_ascii_case(value)) {
            return false;
        }
        if !self.exclude.is_empty() && self.exclude.iter().any(|e| e.eq_ignore_ascii_case(value)) {
            return false;
        }
        true
    }

    fn matches_regex(&self, value: &str) -> bool {
        if !self.include.is_empty() && !matches_any_regex(&self.include, value) {
            return false;
        }
        if !self.exclude.is_empty() && matches_any_regex(&self.exclude, value) {
            return false;
        }
        true
    }

    fn validate_regex(&self, field: &str) -> Result<(), String> {
        validate_patterns(&self.include, &format!("{field}.include"))?;
        validate_patterns(&self.exclude, &format!("{field}.exclude"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExtractionRules {
    /// Type 0: Master switch.
    #[serde(default)]
    pub enabled: bool,
    /// Type 1: File extensions (exact match, e.g. `{"include": ["pdf","docx"]}`).
    #[serde(default)]
    pub extensions: FilterRule,
    /// Type 2: Folder patterns (regex, e.g. `{"include": ["^INBOX/Invoices"]}`).
    #[serde(default)]
    pub folders: FilterRule,
    /// Type 3: Attachment filename patterns (regex).
    #[serde(default)]
    pub attachment_names: FilterRule,
    /// Type 4: Sender patterns (regex).
    #[serde(default)]
    pub senders: FilterRule,
}

impl ExtractionRules {
    /// Returns `true` if the attachment should be extracted under these rules.
    pub fn should_extract(
        &self,
        ext: &str,
        folder: Option<&str>,
        attachment_name: Option<&str>,
        sender: Option<&str>,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.extensions.matches_exact(ext) {
            return false;
        }
        if !self.folders.is_empty() {
            if let Some(folder) = folder {
                if !self.folders.matches_regex(folder) {
                    return false;
                }
            }
        }
        if !self.attachment_names.is_empty() {
            if let Some(name) = attachment_name {
                if !self.attachment_names.matches_regex(name) {
                    return false;
                }
            }
        }
        if !self.senders.is_empty() {
            if let Some(sender) = sender {
                if !self.senders.matches_regex(sender) {
                    return false;
                }
            }
        }
        true
    }

    pub fn validate(&self) -> Result<(), String> {
        self.folders.validate_regex("folders")?;
        self.attachment_names.validate_regex("attachment_names")?;
        self.senders.validate_regex("senders")?;
        Ok(())
    }
}

/// Archive filtering rules — skip unwanted emails before storage.
///
/// Rule types:
///   0 — Master switch
///   1 — Sender filter (regex)
///   2 — Subject filter (regex)
///   3 — Skip emails larger than this (bytes)
///   4 — Skip emails with spam headers (X-Spam-Flag, X-Spam)
///
/// `None` = archive everything (backward compatible).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ArchiveRules {
    /// Type 0: Master switch. `false` = archive everything.
    #[serde(default)]
    pub enabled: bool,
    /// Type 1: Sender filter (regex, include/exclude).
    #[serde(default)]
    pub senders: FilterRule,
    /// Type 2: Subject filter (regex, include/exclude).
    #[serde(default)]
    pub subjects: FilterRule,
    /// Type 3: Skip emails larger than this (bytes). `None` = no size limit.
    #[serde(default)]
    pub skip_larger_than: Option<u64>,
    /// Type 4: Spam header names to check (e.g. `["X-Spam-Flag", "X-Spam"]`).
    /// When the value is `yes` or `true` (case-insensitive), the email is skipped.
    /// Empty = don't check. Common headers: `X-Spam-Flag` (SpamAssassin),
    /// `X-Spam` (rspamd), `X-MS-Exchange-Organization-SCL` (Exchange).
    #[serde(default)]
    pub spam_headers: Vec<String>,
}

impl ArchiveRules {
    /// Returns `true` if the email should be archived under these rules.
    pub fn should_archive(
        &self,
        sender: Option<&str>,
        subject: Option<&str>,
        size: u32,
        is_spam: bool,
    ) -> bool {
        if !self.enabled {
            return true;
        }
        if !self.senders.is_empty() {
            if let Some(sender) = sender {
                if !self.senders.matches_regex(sender) {
                    return false;
                }
            }
        }
        if !self.subjects.is_empty() {
            if let Some(subject) = subject {
                if !self.subjects.matches_regex(subject) {
                    return false;
                }
            }
        }
        if let Some(limit) = self.skip_larger_than {
            if size as u64 > limit {
                return false;
            }
        }
        if !self.spam_headers.is_empty() && is_spam {
            return false;
        }
        true
    }

    /// Validate all regex patterns are well-formed.
    pub fn validate(&self) -> Result<(), String> {
        self.senders.validate_regex("senders")?;
        self.subjects.validate_regex("subjects")?;
        Ok(())
    }
}

fn matches_any_regex(patterns: &[String], value: &str) -> bool {
    patterns.iter().any(|p| {
        regex::Regex::new(p)
            .map(|re| re.is_match(value))
            .unwrap_or(false)
    })
}

fn validate_patterns(patterns: &[String], field_name: &str) -> Result<(), String> {
    for p in patterns {
        regex::Regex::new(p)
            .map_err(|e| format!("{} pattern '{}' is invalid regex: {}", field_name, p, e))?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Account {
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    #[cfg_attr(
        feature = "web-api",
        oai(validator(custom = "crate::common::validator::EmailValidator"))
    )]
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
    #[serde(default)]
    pub max_email_size_bytes: Option<u64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub use_dangerous: bool,
    /// UIDONLY acquisition is opt-in because it changes remote message identity.
    #[serde(default)]
    pub uidonly_enabled: bool,
    pub pgp_key: Option<String>,
    pub imap_quota_bytes: Option<u64>,
    pub imap_quota_window: Option<QuotaWindow>,
    pub auto_download_new_mailboxes: Option<bool>,
    pub download_schedule: Option<String>,
    #[serde(default)]
    pub deleting: bool,
    /// Email-level filtering rules (Pro feature).
    /// `None` = archive everything (backward compatible).
    #[serde(default)]
    pub archive_rules: Option<ArchiveRules>,
    /// Attachment text extraction rules (Pro feature).
    /// `None` = extract everything (backward compatible).
    #[serde(default)]
    pub extraction_rules: Option<ExtractionRules>,
}

impl MemDbModel for Account {
    fn collection() -> &'static str {
        "accounts"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

impl Account {
    pub fn new(user_id: u64, request: AccountCreateRequest) -> BichonResult<Self> {
        Ok(Self {
            id: id!(64),
            email: request.email,
            login_name: request.login_name,
            account_name: request.account_name,
            imap: request.imap.map(|i| i.try_encrypt_password()).transpose()?,
            enabled: request.enabled,
            capabilities: None,
            date_since: request.date_since,
            download_folders: None,
            known_folders: None,
            account_type: request.account_type,
            download_interval_min: request.download_interval_min,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            use_dangerous: request.use_dangerous,
            uidonly_enabled: request.uidonly_enabled.unwrap_or_default(),
            pgp_key: request.pgp_key,
            created_by: user_id,
            download_batch_size: request.download_batch_size,
            max_email_size_bytes: request.max_email_size_bytes,
            date_before: request.date_before,
            auto_download_new_mailboxes: request.auto_download_new_mailboxes,
            imap_quota_bytes: request.imap_quota_bytes,
            imap_quota_window: request.imap_quota_window,
            download_schedule: request.download_schedule,
            deleting: false,
            archive_rules: request.archive_rules,
            extraction_rules: request.extraction_rules,
        })
    }

    pub fn check_account_exists(account_id: u64) -> BichonResult<AccountModel> {
        Self::get(account_id)
    }

    pub fn get(account_id: u64) -> BichonResult<AccountModel> {
        let result: AccountModel = Self::find(account_id)?.ok_or_else(|| {
            raise_error!(
                format!("Account with ID '{account_id}' not found"),
                ErrorCode::ResourceNotFound
            )
        })?;
        Ok(result)
    }

    pub fn find(account_id: u64) -> BichonResult<Option<AccountModel>> {
        let result = find_impl::<AccountModel>(DB_MANAGER.db(), &account_id.to_string())?;
        Ok(result)
    }

    pub async fn create_account(
        user_id: u64,
        request: AccountCreateRequest,
    ) -> BichonResult<AccountModel> {
        let entity = request.create_entity(user_id)?;
        let cloned = entity.clone();

        // Insert account into memdb
        insert_impl(DB_MANAGER.db(), entity)?;

        // Update user's account_access_map
        let user = UserModel::find(user_id)?.ok_or_else(|| {
            raise_error!(
                format!("User with id={} not found.", user_id),
                ErrorCode::ResourceNotFound
            )
        })?;

        let mut updated_map = user.account_access_map.clone();
        updated_map.insert(cloned.id, DEFAULT_ACCOUNT_MANAGER_ROLE_ID);

        UserModel::update(
            user_id,
            UserUpdateRequest {
                username: None,
                email: None,
                password: None,
                avatar_base64: None,
                global_roles: None,
                account_access_map: Some(updated_map),
                acl: None,
                description: None,
                theme: None,
                language: None,
            },
        )?;

        if matches!(cloned.account_type, AccountType::IMAP) {
            DOWNLOAD_CONTROLLER
                .trigger_schedule(cloned.id, cloned.email.clone())
                .await;
        }
        Ok(cloned)
    }

    pub fn update(
        account_id: u64,
        request: AccountUpdateRequest,
        validate: bool,
    ) -> BichonResult<()> {
        let account = AccountModel::get(account_id)?;
        if validate {
            request.validate_update_request(&account)?;
        }
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| Self::apply_update_fields(&current, request),
        )?;

        Ok(())
    }

    pub async fn delete(account_id: u64) -> BichonResult<()> {
        let account = Self::get(account_id)?;

        // Immediately stop scheduling to prevent new downloads
        if matches!(account.account_type, AccountType::IMAP) {
            SYNC_TASKS.stop(account.id).await?;
        }

        // Mark as deleting and disabled so frontend shows status and download tasks skip it
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.deleting = true;
                updated.enabled = false;
                Ok(updated)
            },
        )?;

        // Spawn background cleanup — heavy work (Tantivy, attachments) runs off the request path
        tokio::spawn(async move {
            if let Err(error) = Self::cleanup_account_resources_sequential(&account).await {
                tracing::error!(
                    "[CLEANUP_ACCOUNT_ERROR] Account {}: cleanup failed, reverting deleting flag: {:#?}",
                    account_id,
                    error
                );
                // Revert deleting flag so the user can retry (only if account record still exists)
                let _ = update_impl(
                    DB_MANAGER.db(),
                    &account_id.to_string(),
                    move |current: Account| {
                        let mut updated = current.clone();
                        updated.deleting = false;
                        updated.enabled = true;
                        Ok(updated)
                    },
                );
            }
        });

        Ok(())
    }

    fn delete_account(account: &AccountModel) -> BichonResult<()> {
        delete_impl::<AccountModel>(DB_MANAGER.db(), &account.id.to_string())
    }

    async fn cleanup_account_resources_sequential(account: &AccountModel) -> BichonResult<()> {
        // Sync task already stopped in delete() before spawning this background task
        if matches!(account.account_type, AccountType::IMAP) {
            DownloadState::delete(account.id)?;
        }
        OAuth2AccessToken::try_delete(account.id)?;
        UserModel::cleanup_account(account.id)?;
        MailBox::clean(account.id)?;
        ENVELOPE_MANAGER
            .delete_account_envelopes(account.id)
            .await?;
        Self::delete_account(account)?;
        info!("Sequential cleanup completed for account: {}", account.id);
        Ok(())
    }

    pub fn update_download_folders(
        account_id: u64,
        download_folders: Vec<String>,
    ) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.download_folders = Some(download_folders);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    pub fn update_known_folders(
        account_id: u64,
        known_folders: BTreeSet<String>,
    ) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.known_folders = Some(known_folders);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    pub fn update_capabilities(account_id: u64, capabilities: Vec<String>) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.capabilities = Some(capabilities);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    /// Retrieves a list of all `AccountEntity` instances.
    pub fn list_all() -> BichonResult<Vec<AccountModel>> {
        list_all_impl::<AccountModel>(DB_MANAGER.db())
    }

    pub fn find_by_email(email: &str) -> BichonResult<Option<AccountModel>> {
        let all: Vec<AccountModel> = list_all_impl::<AccountModel>(DB_MANAGER.db())?;
        let target_email = email.trim().to_lowercase();

        let first_match = all
            .into_iter()
            .find(|acc| acc.email.to_lowercase() == target_email);

        Ok(first_match)
    }

    pub fn minimal_list(only_nosync: bool) -> BichonResult<Vec<MinimalAccount>> {
        let result = list_all_impl::<AccountModel>(DB_MANAGER.db())?
            .into_iter()
            .filter(|account: &AccountModel| {
                !only_nosync || matches!(account.account_type, AccountType::NoSync)
            })
            .map(|account: AccountModel| MinimalAccount {
                id: account.id,
                email: account.email,
                name: account.account_name,
            })
            .collect::<Vec<MinimalAccount>>();
        Ok(result)
    }

    pub fn count() -> BichonResult<usize> {
        count_impl::<AccountModel>(DB_MANAGER.db())
    }

    pub fn paginate_list(
        page: Option<u64>,
        page_size: Option<u64>,
        desc: Option<bool>,
    ) -> BichonResult<DataPage<AccountModel>> {
        paginate_impl::<AccountModel>(DB_MANAGER.db(), page, page_size, desc).map(DataPage::from)
    }

    // This method applies the updates from the request to the old account entity
    fn apply_update_fields(
        old: &AccountModel,
        request: AccountUpdateRequest,
    ) -> BichonResult<AccountModel> {
        let mut new = old.clone();

        if let Some(date_since) = request.date_since {
            new.date_since = Some(date_since);
            new.date_before = None;
        }

        if let Some(date_before) = request.date_before {
            new.date_before = Some(date_before);
            new.date_since = None;
        }

        if let Some(clear_date_range) = request.clear_date_range {
            if clear_date_range {
                new.date_since = None;
                new.date_before = None;
            }
        }

        if let Some(account_name) = request.account_name {
            new.account_name = Some(account_name);
        }

        if matches!(old.account_type, AccountType::IMAP) {
            if let Some(imap) = &request.imap {
                if let Some(current_imap) = &mut new.imap {
                    current_imap.host = imap.host.clone();
                    current_imap.port = imap.port.clone();
                    current_imap.encryption = imap.encryption.clone();
                    current_imap.auth.auth_type = imap.auth.auth_type.clone();
                    if let Some(password) = &imap.auth.password {
                        let encrypted_password = encrypt!(password)?;
                        current_imap.auth.password = Some(encrypted_password);
                    }
                    current_imap.use_proxy = imap.use_proxy;
                }
            }

            if let Some(folder_names) = request.sync_folders {
                new.download_folders = Some(folder_names);
            }
            if let Some(sync_interval_min) = &request.download_interval_min {
                new.download_interval_min = Some(*sync_interval_min);
            }

            if let Some(download_batch_size) = &request.download_batch_size {
                new.download_batch_size = Some(*download_batch_size);
            }

            if let Some(max_email_size_bytes) = request.max_email_size_bytes {
                new.max_email_size_bytes = Some(max_email_size_bytes);
            }
        }

        if matches!(old.account_type, AccountType::NoSync) {
            if let Some(email) = &request.email {
                new.email = email.clone();
            }
        }

        if let Some(enabled) = request.enabled {
            new.enabled = enabled;
        }

        if let Some(use_dangerous) = request.use_dangerous {
            new.use_dangerous = use_dangerous;
        }

        if let Some(uidonly_enabled) = request.uidonly_enabled {
            new.uidonly_enabled = uidonly_enabled;
        }

        if let Some(pgp_key) = request.pgp_key {
            new.pgp_key = Some(pgp_key);
        }

        if let Some(imap_quota_bytes) = request.imap_quota_bytes {
            new.imap_quota_bytes = Some(imap_quota_bytes);
        }

        if let Some(imap_quota_window) = request.imap_quota_window {
            new.imap_quota_window = Some(imap_quota_window);
        }

        if let Some(auto_download_new_mailboxes) = request.auto_download_new_mailboxes {
            new.auto_download_new_mailboxes = Some(auto_download_new_mailboxes);
        }
        if let Some(download_schedule) = request.download_schedule {
            new.download_schedule = Some(download_schedule);
        }
        if request.clear_download_schedule == Some(true) {
            new.download_schedule = None;
        }
        if request.extraction_rules.is_some() {
            new.extraction_rules = request.extraction_rules;
        }
        if request.archive_rules.is_some() {
            new.archive_rules = request.archive_rules;
        }
        new.updated_at = utc_now!();
        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uidonly_setting_defaults_off_and_only_changes_explicitly() {
        let account = Account::new(1, AccountCreateRequest::default()).unwrap();
        assert!(!account.uidonly_enabled);
        let update = |old: &Account, value| {
            Account::apply_update_fields(
                old,
                AccountUpdateRequest {
                    uidonly_enabled: value,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let enabled = update(&account, Some(true));
        assert!(enabled.uidonly_enabled);
        assert!(update(&enabled, None).uidonly_enabled);
        assert!(!update(&enabled, Some(false)).uidonly_enabled);

        let mut legacy = serde_json::to_value(enabled).unwrap();
        legacy.as_object_mut().unwrap().remove("uidonly_enabled");
        assert!(!serde_json::from_value::<Account>(legacy)
            .unwrap()
            .uidonly_enabled);
    }

    // ── FilterRule ───────────────────────────────────────────────────

    #[test]
    fn filter_rule_include_only() {
        let r = FilterRule {
            include: vec![r"@ok\.com$".into()],
            ..Default::default()
        };
        assert!(r.matches_regex("bob@ok.com"));
        assert!(!r.matches_regex("spam@bad.com"));
    }

    #[test]
    fn filter_rule_exclude_only() {
        let r = FilterRule {
            exclude: vec![r"@spam\.com$".into()],
            ..Default::default()
        };
        assert!(r.matches_regex("bob@ok.com"));
        assert!(!r.matches_regex("bot@spam.com"));
    }

    #[test]
    fn filter_rule_include_then_exclude() {
        let r = FilterRule {
            include: vec![r"@company\.com$".into()],
            exclude: vec![r"noreply@company\.com$".into()],
            ..Default::default()
        };
        assert!(r.matches_regex("bob@company.com"));
        assert!(!r.matches_regex("noreply@company.com"));
        assert!(!r.matches_regex("spam@other.com"));
    }

    #[test]
    fn filter_rule_exact_match() {
        let r = FilterRule {
            include: vec!["pdf".into(), "docx".into()],
            exclude: vec!["xlsx".into()],
            ..Default::default()
        };
        assert!(r.matches_exact("pdf"));
        assert!(r.matches_exact("docx"));
        assert!(r.matches_exact("DOCX")); // case-insensitive
        assert!(!r.matches_exact("xlsx"));
        assert!(!r.matches_exact("txt"));
    }

    // ── ExtractionRules ─────────────────────────────────────────────

    #[test]
    fn extraction_rules_master_switch() {
        let rules = ExtractionRules {
            enabled: false,
            ..Default::default()
        };
        assert!(!rules.should_extract("pdf", None, None, None));
    }

    #[test]
    fn extraction_rules_extension_include() {
        let rules = ExtractionRules {
            enabled: true,
            extensions: FilterRule {
                include: vec!["pdf".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.should_extract("pdf", None, None, None));
        assert!(!rules.should_extract("docx", None, None, None));
    }

    #[test]
    fn extraction_rules_extension_exclude() {
        let rules = ExtractionRules {
            enabled: true,
            extensions: FilterRule {
                exclude: vec!["xlsx".into(), "pptx".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.should_extract("pdf", None, None, None));
        assert!(!rules.should_extract("xlsx", None, None, None));
    }

    #[test]
    fn extraction_rules_folder_regex() {
        let rules = ExtractionRules {
            enabled: true,
            folders: FilterRule {
                include: vec![r"^INBOX/Invoices".into(), r"Contracts$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.should_extract("pdf", Some("INBOX/Invoices"), None, None));
        assert!(rules.should_extract("pdf", Some("Finance/Contracts"), None, None));
        assert!(!rules.should_extract("pdf", Some("INBOX/Junk"), None, None));
    }

    #[test]
    fn extraction_rules_attachment_name_regex() {
        let rules = ExtractionRules {
            enabled: true,
            attachment_names: FilterRule {
                include: vec![r"^invoice-.*\.pdf$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.should_extract("pdf", None, Some("invoice-2024.pdf"), None));
        assert!(!rules.should_extract("pdf", None, Some("newsletter.pdf"), None));
    }

    #[test]
    fn extraction_rules_sender_regex() {
        let rules = ExtractionRules {
            enabled: true,
            senders: FilterRule {
                exclude: vec![r"@noreply\.com$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        // non-excluded sender passes
        assert!(rules.should_extract("pdf", None, None, Some("bob@ok.com")));
        // excluded sender blocked
        assert!(!rules.should_extract("pdf", None, None, Some("bot@noreply.com")));
    }

    #[test]
    fn extraction_rules_empty_filters_pass_everything() {
        let rules = ExtractionRules {
            enabled: true,
            ..Default::default()
        };
        assert!(rules.should_extract(
            "anything",
            Some("any/folder"),
            Some("any.pdf"),
            Some("any@x.com")
        ));
    }

    // ── ArchiveRules ────────────────────────────────────────────────

    #[test]
    fn archive_rules_disabled_archives_everything() {
        let rules = ArchiveRules {
            enabled: false,
            ..Default::default()
        };
        assert!(rules.should_archive(Some("spam@x.com"), Some("BUY NOW"), 999, false));
    }

    #[test]
    fn archive_rules_sender_exclude() {
        let rules = ArchiveRules {
            enabled: true,
            senders: FilterRule {
                exclude: vec![r"@spam\.com$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!rules.should_archive(Some("bot@spam.com"), None, 100, false));
        assert!(rules.should_archive(Some("friend@ok.com"), None, 100, false));
    }

    #[test]
    fn archive_rules_subject_exclude() {
        let rules = ArchiveRules {
            enabled: true,
            subjects: FilterRule {
                exclude: vec![r"(?i)unsubscribe|buy now|limited offer".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!rules.should_archive(None, Some("UNSUBSCRIBE NOW"), 100, false));
        assert!(!rules.should_archive(None, Some("Limited Offer!!"), 100, false));
        assert!(rules.should_archive(None, Some("Meeting tomorrow"), 100, false));
    }

    #[test]
    fn archive_rules_sender_include() {
        // Only archive emails from specific senders
        let rules = ArchiveRules {
            enabled: true,
            senders: FilterRule {
                include: vec![r"@partner\.com$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.should_archive(Some("bob@partner.com"), None, 100, false));
        assert!(!rules.should_archive(Some("spam@random.com"), None, 100, false));
    }

    #[test]
    fn archive_rules_skip_larger_than() {
        let rules = ArchiveRules {
            enabled: true,
            skip_larger_than: Some(50_000_000),
            ..Default::default()
        };
        assert!(rules.should_archive(None, None, 1_000_000, false));
        assert!(!rules.should_archive(None, None, 60_000_000, false));
    }

    #[test]
    fn archive_rules_skip_spam_headers() {
        let rules = ArchiveRules {
            enabled: true,
            spam_headers: vec!["X-Spam-Flag".into()],
            ..Default::default()
        };
        assert!(!rules.should_archive(None, None, 100, true));
        assert!(rules.should_archive(None, None, 100, false));
    }

    // ── Validation ──────────────────────────────────────────────────

    #[test]
    fn validate_extraction_rules_valid() {
        let rules = ExtractionRules {
            folders: FilterRule {
                include: vec![r"^INBOX/.*".into()],
                ..Default::default()
            },
            senders: FilterRule {
                exclude: vec![r"@spam\.com$".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_ok());
    }

    #[test]
    fn validate_extraction_rules_invalid_regex() {
        let rules = ExtractionRules {
            folders: FilterRule {
                include: vec!["***bad[".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_archive_rules_valid() {
        let rules = ArchiveRules {
            senders: FilterRule {
                exclude: vec![r"@spam\.com$".into()],
                ..Default::default()
            },
            subjects: FilterRule {
                include: vec![r"(?i)invoice".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_ok());
    }

    #[test]
    fn validate_archive_rules_invalid_regex() {
        let rules = ArchiveRules {
            senders: FilterRule {
                include: vec!["[unclosed".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }
}
