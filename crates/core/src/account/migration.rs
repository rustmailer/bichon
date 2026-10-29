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
use std::str::FromStr;
use tracing::info;

use crate::{
    account::{
        entity::ImapConfig,
        payload::{AccountCreateRequest, AccountUpdateRequest, MinimalAccount},
        since::{DateSince, RelativeDate},
        state::DownloadState,
    },
    archive::imap::{mailbox::MailBox, task::SYNC_TASKS},
    common::{auth::ClientContext, paginated::DataPage},
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
    store::tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER},
    users::{payload::UserUpdateRequest, role::DEFAULT_ACCOUNT_MANAGER_ROLE_ID, UserModel},
    utc_now,
};

pub type AccountModel = Account;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum AccountType {
    #[default]
    IMAP,
    /// A local account that stores its own mail (import target, journal
    /// archive, or a hand-made local mailbox).
    NoSync,
    /// A read-only, filter-scoped view over one or more other accounts.
    ///
    /// A *type*, not a flag: a view stores nothing, is never synced, receives
    /// no SMTP, and every "is this a storage account?" check must answer "no"
    /// without remembering a special case. Its sources and filters live in
    /// [`Account::view`].
    View,
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
        validate_non_empty(&self.extensions.include, "extensions.include")?;
        validate_non_empty(&self.extensions.exclude, "extensions.exclude")?;
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

    /// Validate all regex patterns are well-formed and non-empty.
    pub fn validate(&self) -> Result<(), String> {
        self.senders.validate_regex("senders")?;
        self.subjects.validate_regex("subjects")?;
        validate_non_empty(&self.spam_headers, "spam_headers")?;
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
        if p.trim().is_empty() {
            return Err(format!("{field_name} pattern must not be empty"));
        }
        regex::Regex::new(p)
            .map_err(|e| format!("{} pattern '{}' is invalid regex: {}", field_name, p, e))?;
    }
    Ok(())
}

fn validate_non_empty(patterns: &[String], field_name: &str) -> Result<(), String> {
    for p in patterns {
        if p.trim().is_empty() {
            return Err(format!("{field_name} item must not be empty"));
        }
    }
    Ok(())
}

/// Fixed destination folder for source-mailbox cleanup. Bichon creates and
/// owns this folder; it is never used as a download source.
pub const REMOTE_CLEANUP_FOLDER: &str = "bichon_auto_created_trash";

/// Minimum grace period between archiving a message and moving it out of the
/// source mailbox: none. Zero means "eligible on the next run" and is a valid
/// setting, because the move is reversible — the message lands in
/// [`REMOTE_CLEANUP_FOLDER`] on the same server and the local archive keeps its
/// copy — and a message whose local copy fails verification is skipped rather
/// than moved. Emptying that folder is the irreversible step, and it is gated
/// separately on a recent successful backup. Holding mail in the source
/// mailbox while the archive fills is still the default (30 days), not a floor.
pub const MIN_REMOTE_CLEANUP_GRACE_DAYS: u64 = 0;

/// Hard ceiling on one cleanup run. The scheduled task is deliberately slow;
/// callers may lower this, never exceed it.
pub const MAX_REMOTE_CLEANUP_PER_RUN: u64 = 500;

fn default_cleanup_grace_days() -> u64 {
    30
}

fn default_cleanup_max_per_run() -> u64 {
    50
}

fn default_true() -> bool {
    true
}

/// Configuration for moving archived mail out of the source IMAP mailbox into
/// [`REMOTE_CLEANUP_FOLDER`].
///
/// Moving is reversible (the message still exists on the server) and may run on
/// a schedule. Permanently emptying that folder is a separate, explicit API
/// operation and is never scheduled.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct RemoteCleanupConfig {
    /// Master switch for the scheduled move task. Default off.
    #[serde(default)]
    pub enabled: bool,
    /// Cron expression for the scheduled move task.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Source folders. `None` = the account's download folders, minus
    /// special-use folders and the cleanup destination.
    #[serde(default)]
    pub folders: Option<Vec<String>>,
    /// Only move messages archived at least this many days ago.
    /// Measured from the envelope's `ingest_at`.
    #[serde(default = "default_cleanup_grace_days")]
    pub eligible_after_archive_days: u64,
    /// Optional additional message-age floor (based on the message date).
    /// `None` = no extra floor; accounts using `date_before` already bound it.
    #[serde(default)]
    pub min_message_age_days: Option<u64>,
    /// Maximum messages moved in one batch.
    #[serde(default = "default_cleanup_max_per_run")]
    pub max_messages_per_run: u64,
    /// Leave flagged/starred messages in the source mailbox.
    #[serde(default = "default_true")]
    pub skip_flagged: bool,
    // There is deliberately no pause flag: `enabled` and the schedule are the
    // only ways to stop runs.
}

impl Default for RemoteCleanupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: None,
            folders: None,
            eligible_after_archive_days: default_cleanup_grace_days(),
            min_message_age_days: None,
            max_messages_per_run: default_cleanup_max_per_run(),
            skip_flagged: true,
        }
    }
}

impl RemoteCleanupConfig {
    pub fn validate(&self) -> Result<(), String> {
        // No lower bound on `eligible_after_archive_days`: 0 is a valid choice
        // ("eligible on the next run") because the move is reversible. See
        // MIN_REMOTE_CLEANUP_GRACE_DAYS.
        if self.max_messages_per_run == 0 || self.max_messages_per_run > MAX_REMOTE_CLEANUP_PER_RUN {
            return Err(format!(
                "max_messages_per_run must be between 1 and {MAX_REMOTE_CLEANUP_PER_RUN}"
            ));
        }
        if let Some(days) = self.min_message_age_days {
            if days == 0 {
                return Err("min_message_age_days must be greater than 0 when set".into());
            }
        }
        if let Some(schedule) = &self.schedule {
            if schedule.trim().is_empty() {
                return Err("cleanup schedule must not be empty".into());
            }
            cron::Schedule::from_str(schedule)
                .map_err(|e| format!("Invalid cleanup schedule '{schedule}': {e}"))?;
        }
        if let Some(folders) = &self.folders {
            if folders.is_empty() {
                return Err("cleanup folders must not be empty when set".into());
            }
            for folder in folders {
                if folder.trim().is_empty() {
                    return Err("cleanup folder name must not be empty".into());
                }
                if folder.eq_ignore_ascii_case(REMOTE_CLEANUP_FOLDER) {
                    return Err(format!(
                        "'{REMOTE_CLEANUP_FOLDER}' cannot be a cleanup source folder"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// What `kind.role` says the account is. `AccountType` stays the storage type
/// (IMAP / NoSync); this is the *special role* on top of it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
#[serde(rename_all = "snake_case")]
pub enum AccountKindRole {
    /// The account SMTP journal deliveries are filed into. Reserved for the
    /// journaling configuration — it cannot be chosen when creating an
    /// account by hand.
    ///
    /// `view` is deliberately **not** a role: it is an [`AccountType`] of its
    /// own, because a view stores nothing and every storage-related check must
    /// exclude it by type rather than by remembering a flag.
    #[default]
    #[cfg_attr(feature = "web-api", oai(rename = "journal"))]
    Journal,
}

/// A view's restriction on one source account.
///
/// Every condition set here is **ANDed** with whatever the viewer searches
/// for: a view can only ever narrow, never widen. An empty filter exposes the
/// whole source read-only — a deliberate, administrator-chosen delegation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ViewSource {
    pub account_id: u64,
    /// The message must have **at least one** of these addresses in From, To,
    /// Cc or Bcc (OR within the field).
    ///
    /// A person with several addresses, or a view over a small team, needs more
    /// than one. The serde adapter also accepts a single string, so a row
    /// written against the first version of this field (one address) still
    /// parses.
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    #[cfg_attr(feature = "web-api", oai(default))]
    pub any_participant: Vec<String>,
    /// The message must have **at least one** of these addresses in To, Cc or
    /// Bcc (OR within the field).
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    #[cfg_attr(feature = "web-api", oai(default))]
    pub any_recipient: Vec<String>,
}

impl ViewSource {
    /// A source without any condition: the whole account is delegated.
    ///
    /// The distinction matters wherever a caller can only work with *account
    /// ids* — the attachment index, the aggregate endpoints, the dashboard —
    /// because those cannot evaluate a participant filter and must therefore
    /// leave a filtered source out entirely rather than expose all of it.
    pub fn is_unfiltered(&self) -> bool {
        self.any_participant.is_empty() && self.any_recipient.is_empty()
    }
}

/// Accept both `"a@b.de"` and `["a@b.de", "c@d.de"]` (and `null`).
fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    Ok(
        match Option::<OneOrMany>::deserialize(deserializer)? {
            None => Vec::new(),
            Some(OneOrMany::One(address)) => vec![address],
            Some(OneOrMany::Many(addresses)) => addresses,
        },
    )
}

/// Optional special role of an account.
///
/// Kept separate from [`AccountType`] on purpose: a journal archive and a view
/// are both `NoSync` accounts, and overloading the type would have made every
/// existing `account_type` check wrong. `None` — the default, and what every
/// row written before this field existed deserializes to — is an ordinary
/// account.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccountKind {
    pub role: AccountKindRole,
    /// Journal: the address the tenant's journaling rule delivers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_address: Option<String>,
}

/// Configuration of an [`AccountType::View`] account.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ViewConfig {
    /// The accounts the view reads from, each with its own filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ViewSource>,
}

/// What an account update did to a view's scope, for the audit trail.
///
/// A bare `account.updated` line cannot distinguish renaming a view from
/// dropping its filters — and the second is a widening of someone's visibility
/// that nobody could reconstruct afterwards. Both sides are recorded, because
/// "from what" is half the answer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ViewConfigChange {
    pub before: Option<ViewConfig>,
    pub after: Option<ViewConfig>,
}

impl ViewConfigChange {
    /// `None` when the scope is unchanged, so a rename or an unrelated field
    /// leaves no view record behind.
    pub fn between(before: Option<&ViewConfig>, after: Option<&ViewConfig>) -> Option<Self> {
        if before == after {
            return None;
        }
        Some(Self {
            before: before.cloned(),
            after: after.cloned(),
        })
    }
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
    /// Ordered header names checked at ingest. The first one that parses to a
    /// valid mailbox replaces the stored `from` address; the original address
    /// is kept in `wire_from`. `None` = disabled, and it is also what every
    /// account written before this field existed deserializes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_from_headers: Option<Vec<String>>,
    /// Account-level retention period in days (free/community feature).
    /// `None` or `0` = disabled (archive everything, never auto-purge).
    /// A background sweep purges envelopes whose effective date
    /// (`min(date, internal_date)`) is older than this window. Accounts on
    /// legal hold are always exempt.
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Source-mailbox cleanup (community feature). `None` = not configured.
    /// Moving archived mail to [`REMOTE_CLEANUP_FOLDER`] may be scheduled;
    /// permanently emptying that folder never is.
    #[serde(default)]
    pub remote_cleanup: Option<RemoteCleanupConfig>,
    /// Legal hold flag (Enterprise feature; community edition has no setter
    /// and this stays at its default of `false` forever). While set, the
    /// account's data is frozen: the retention sweep skips the account and
    /// bulk deletion paths refuse to purge it.
    #[serde(default)]
    pub legal_hold: bool,
    /// Reason recorded when the hold was placed (audit-friendly, required).
    #[serde(default)]
    pub hold_reason: Option<String>,
    /// User id that placed the hold.
    #[serde(default)]
    pub hold_placed_by: Option<u64>,
    /// Epoch millis when the hold was placed.
    #[serde(default)]
    pub hold_placed_at: Option<i64>,
    /// User id that released the hold (last release).
    #[serde(default)]
    pub hold_released_by: Option<u64>,
    /// Epoch millis when the hold was released.
    #[serde(default)]
    pub hold_released_at: Option<i64>,
    /// Optional special role — journal archive or read-only view.
    ///
    /// `None` is an ordinary account, and it is also what every row written
    /// before this field existed deserializes to, so the field is additive and
    /// backward compatible. `AccountType` is deliberately untouched: a journal
    /// account and a view are both `NoSync` accounts, and overloading the type
    /// would have made every existing `account_type` check wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AccountKind>,
    /// Sources and per-source filters of an [`AccountType::View`] account.
    /// `None` for every other account type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewConfig>,
    /// Archive-side IMAP UID counter: the lowest value that has NOT been handed
    /// out yet for this account. Block-reserved, monotonic, never regresses.
    /// 0 = never initialized.
    ///
    /// Lives on the account row (not on `MailBox`) because every account write
    /// is a read-modify-write (`update_impl`), while mailbox rows are rewritten
    /// wholesale from remote listings (`download/flow.rs`), which would silently
    /// reset a counter stored there — and a reset counter reissues UIDs.
    #[serde(default)]
    pub imap_uid_next: u64,
    /// UIDVALIDITY generation. Bumped when this account's UID space effectively
    /// restarts (a restore to an earlier backup point, or a counter found
    /// behind the index), which invalidates every client cache for the account.
    /// See `archive::imap::stable_uid::uid_validity`.
    #[serde(default)]
    pub imap_uid_generation: u32,
}

impl MemDbModel for Account {
    fn collection() -> &'static str {
        "accounts"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

/// The account name is a display-only label: an empty or whitespace-only value
/// means "no name". It is stored as `None`, never as an empty string, so
/// consumers can rely on `account_name != ""` meaning a real name.
fn normalize_optional_name(name: Option<String>) -> Option<String> {
    match name {
        Some(n) if n.trim().is_empty() => None,
        other => other,
    }
}

impl Account {
    /// The account SMTP journal deliveries are filed into.
    pub fn is_journal_account(&self) -> bool {
        matches!(
            self.kind.as_ref().map(|k| &k.role),
            Some(AccountKindRole::Journal)
        )
    }

    /// A read-only, filter-scoped view over other accounts.
    pub fn is_view_account(&self) -> bool {
        matches!(self.account_type, AccountType::View)
    }

    /// Whether the account stores mail of its own (as opposed to a view).
    pub fn stores_mail(&self) -> bool {
        !self.is_view_account()
    }

    pub fn new(user_id: u64, request: AccountCreateRequest) -> BichonResult<Self> {
        Ok(Self {
            id: id!(64),
            email: request.email,
            login_name: request.login_name,
            account_name: normalize_optional_name(request.account_name),
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
            override_from_headers: request.override_from_headers,
            retention_days: request.retention_days,
            remote_cleanup: request.remote_cleanup,
            legal_hold: false,
            hold_reason: None,
            hold_placed_by: None,
            hold_placed_at: None,
            hold_released_by: None,
            hold_released_at: None,
            kind: request.kind,
            view: request.view,
            imap_uid_next: 0,
            imap_uid_generation: 0,
        })
    }

    pub fn check_account_exists(account_id: u64) -> BichonResult<AccountModel> {
        Self::get(account_id)
    }

    /// Whether the account currently sits under a legal hold. While held, the
    /// retention sweep must skip it and bulk deletion paths must refuse.
    pub fn is_on_hold(&self) -> bool {
        self.legal_hold
    }

    /// Effective retention period in days. `0` / `None` means disabled.
    pub fn retention_days_effective(&self) -> u64 {
        self.retention_days.unwrap_or(0)
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

    /// Create an account on `context`'s user's behalf.
    ///
    /// Takes the `ClientContext`, not a bare user id, because the view rule is
    /// a permission check — creating a view delegates read access to the
    /// accounts it names, so the caller must hold `account:manage` on every
    /// source. Enforcing it *here* rather than in the REST handler means a
    /// second caller (a CLI, an approval executor, an MCP tool) cannot create a
    /// view without it.
    pub async fn create_account(
        user_id: u64,
        request: AccountCreateRequest,
        context: &ClientContext,
    ) -> BichonResult<AccountModel> {
        if matches!(request.account_type, AccountType::View) {
            let sources = request
                .view
                .as_ref()
                .map(|view| view.sources.as_slice())
                .unwrap_or(&[]);
            crate::account::access::require_view_source_management(context, sources)?;
        }
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

    /// Update an account on `context`'s user's behalf.
    ///
    /// Validation is unconditional and the view rule is enforced here, not by
    /// the caller: editing a view can widen what it exposes, so every source in
    /// the submitted configuration must be one the caller manages (or be kept
    /// exactly as it was). See
    /// [`crate::account::access::require_view_source_management_for_update`].
    ///
    /// This returns `()`, so a caller that needs to *audit* a scope change has
    /// to read the old view first — as the REST handler does before emitting
    /// `Event::AccountUpdated` with its `view_change`. The permission rule is
    /// enforced here; the record is not, because the record needs an actor
    /// name this function does not receive.
    pub fn update(
        account_id: u64,
        request: AccountUpdateRequest,
        context: &ClientContext,
    ) -> BichonResult<()> {
        let account = AccountModel::get(account_id)?;
        request.validate_update_request(&account)?;
        if let Some(view) = &request.view {
            let old_sources = account
                .view
                .as_ref()
                .map(|old| old.sources.as_slice())
                .unwrap_or(&[]);
            crate::account::access::require_view_source_management_for_update(
                context,
                old_sources,
                &view.sources,
            )?;
        }
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| Self::apply_update_fields(&current, request),
        )?;

        Ok(())
    }

    /// Mark an account for deletion; the purge runs in the background.
    ///
    /// The refusal checks live in [`Self::delete_and_wait`], which this spawns —
    /// but they are repeated here first, because the spawned task's error can
    /// only be logged: without this the HTTP caller would get a success for a
    /// deletion that never happened.
    pub async fn delete(account_id: u64) -> BichonResult<()> {
        Self::ensure_deletable(&Self::get(account_id)?)?;
        // Spawn background cleanup — heavy work (Tantivy, attachments) runs off the request path
        tokio::spawn(async move {
            if let Err(error) = Self::delete_and_wait(account_id).await {
                tracing::error!(
                    "[CLEANUP_ACCOUNT_ERROR] Account {}: cleanup failed, reverting deleting flag: {:#?}",
                    account_id,
                    error
                );
            }
        });
        Ok(())
    }

    /// The conditions under which an account may not be deleted at all.
    ///
    /// Pure and synchronous so both delete paths (`delete` for HTTP,
    /// `delete_and_wait` for the dual-control executor) refuse for exactly the
    /// same reasons, and so the rules are testable without a database.
    pub fn ensure_deletable(account: &AccountModel) -> BichonResult<()> {
        // Legal hold: an account under a hold is frozen — deleting the account
        // would cascade-purge every message and attachment it holds, defeating
        // the purpose of the hold. Refuse before any state is mutated. In the
        // community edition `legal_hold` is always false, so this guard is a
        // no-op there.
        if account.is_on_hold() {
            return Err(raise_error!(
                format!(
                    "Account {} ({}) is under a legal hold; deleting the account is disabled while the hold is active",
                    account.id, account.email
                ),
                ErrorCode::Forbidden
            ));
        }

        // Journal archive: this account is the one complete copy of everything
        // the customer's journaling rule produced. There is no way to rebuild
        // it — the rule only carries mail from the moment it was configured,
        // and the source mailboxes may long be gone. Refuse unconditionally;
        // unlike a view, it is not an access wrapper but the archive itself.
        if account.is_journal_account() {
            return Err(raise_error!(
                format!(
                    "Account {} ({}) is a journal archive and cannot be deleted; \
                     point journaling at another address instead and keep this record",
                    account.id, account.email
                ),
                ErrorCode::Forbidden
            ));
        }

        // A view source: deleting it would leave the view pointing at an id
        // that no longer exists — its results silently shrink, and the row
        // cannot be repaired from the UI (there is no account left to select).
        // The operator removes it from the view first, then deletes.
        let views = crate::account::access::views_referencing(&Self::list_all()?, account.id);
        if !views.is_empty() {
            let listed: Vec<String> = views.iter().map(|id| id.to_string()).collect();
            return Err(raise_error!(
                format!(
                    "Account {} ({}) is a source of view account(s) {}; remove it from those views first",
                    account.id,
                    account.email,
                    listed.join(", ")
                ),
                ErrorCode::Forbidden
            ));
        }

        Ok(())
    }

    /// The same pre-checks and prelude as [`delete`], but the heavy cleanup is
    /// awaited instead of spawned.
    ///
    /// `delete` returns as soon as the account is *marked* for deletion; the
    /// purge of envelopes, attachments and the Tantivy index runs on for
    /// minutes or hours afterwards, with no completion signal and no record.
    /// That is the right shape for an HTTP handler with a deadline, and the
    /// wrong shape for a caller that needs to know when the work actually
    /// finished — a dual-control executor recording "the deletion executed"
    /// while the account is still fully present would be recording a falsehood.
    ///
    /// `delete` is implemented in terms of this function so there is one
    /// implementation of the cleanup and its failure handling, not two.
    ///
    /// On failure the account is revived (`deleting = false`, `enabled = true`)
    /// so an operator can retry, and the error is returned to the caller rather
    /// than only logged.
    pub async fn delete_and_wait(account_id: u64) -> BichonResult<()> {
        let account = Self::get(account_id)?;
        Self::ensure_deletable(&account)?;

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

        if let Err(error) = Self::cleanup_account_resources_sequential(&account).await {
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
            return Err(error);
        }

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
        ATTACHMENT_MANAGER
            .delete_account_attachments(account.id)
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

    /// Place a legal hold on an account (Enterprise feature; the community
    /// edition has no caller for this, so the flag stays at its default of
    /// `false` there forever). While held, the retention sweep skips the
    /// account and bulk deletion refuses to purge it.
    pub fn place_legal_hold(
        account_id: u64,
        by: u64,
        reason: Option<String>,
    ) -> BichonResult<AccountModel> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.legal_hold = true;
                updated.hold_reason = reason;
                updated.hold_placed_by = Some(by);
                updated.hold_placed_at = Some(utc_now!());
                // Re-arming a hold clears any previous release stamp so the
                // model always shows the current hold cycle.
                updated.hold_released_by = None;
                updated.hold_released_at = None;
                Ok(updated)
            },
        )
    }

    /// Release a legal hold (Enterprise feature; the community edition has no
    /// caller for this). The account is immediately eligible for the retention
    /// sweep again on its next run.
    pub fn release_legal_hold(
        account_id: u64,
        by: u64,
        reason: Option<String>,
    ) -> BichonResult<AccountModel> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.legal_hold = false;
                updated.hold_reason = reason;
                updated.hold_released_by = Some(by);
                updated.hold_released_at = Some(utc_now!());
                // The model tracks one hold cycle at a time: releasing clears
                // the placement stamps, mirroring how re-arming clears the
                // release stamps, so a released account reads clean. The
                // permanent placement record lives in the audit log via the
                // LegalHoldPlaced event.
                updated.hold_placed_by = None;
                updated.hold_placed_at = None;
                Ok(updated)
            },
        )
    }

    /// Accounts currently under a legal hold (Enterprise feature). `None` = no
    /// accounts are held.
    pub fn list_legal_hold_accounts() -> BichonResult<Vec<AccountModel>> {
        Ok(Self::list_all()?
            .into_iter()
            .filter(|a| a.is_on_hold())
            .collect())
    }

    /// Retrieves a list of all `AccountEntity` instances.
    pub fn list_all() -> BichonResult<Vec<AccountModel>> {
        list_all_impl::<AccountModel>(DB_MANAGER.db())
    }

    /// Resolve a delivery target by address. Used by the SMTP server's RCPT TO.
    ///
    /// **Views are skipped.** A view is commonly created with the same address
    /// as the person it shows (the WebUI suggests it), and it never receives
    /// mail. Without this filter a duplicate email could resolve to the view
    /// depending on iteration order, and SMTP delivery to a real mailbox would
    /// start failing with "not a local account". A view is not a delivery
    /// target, so it must not shadow a real one.
    pub fn find_by_email(email: &str) -> BichonResult<Option<AccountModel>> {
        let all: Vec<AccountModel> = list_all_impl::<AccountModel>(DB_MANAGER.db())?;
        let target_email = email.trim().to_lowercase();

        let first_match = all.into_iter().find(|acc| {
            !acc.is_view_account() && acc.email.to_lowercase() == target_email
        });

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
                auth_type: account.imap.map(|imap| imap.auth.auth_type),
                account_type: account.account_type,
                kind: account.kind.as_ref().map(|k| k.role.clone()),
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

        // Empty / whitespace-only clears the name to `None` (the store never
        // holds an empty string); `null` leaves it untouched.
        if let Some(account_name) = request.account_name {
            new.account_name = normalize_optional_name(Some(account_name));
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
        if let Some(view) = request.view {
            // Type and source validation happened in
            // `validate_update_request`; assigning here keeps the view's
            // filters editable without recreating the account (which would
            // change its id and drop every permission granted on it).
            new.view = Some(view);
        }
        if request.archive_rules.is_some() {
            new.archive_rules = request.archive_rules;
        }
        if request.clear_archive_rules == Some(true) {
            new.archive_rules = None;
        }
        if request.override_from_headers.is_some() {
            new.override_from_headers =
                crate::account::payload::normalize_override_from_headers(
                    &request.override_from_headers,
                )
                .map_err(|e| {
                    raise_error!(
                        format!("override_from_headers: {e}"),
                        ErrorCode::InvalidParameter
                    )
                })?;
        }
        if request.clear_override_from_headers == Some(true) {
            new.override_from_headers = None;
        }
        if request.clear_extraction_rules == Some(true) {
            new.extraction_rules = None;
        }
        if request.retention_days.is_some() {
            new.retention_days = request.retention_days;
        }
        if request.clear_retention_days == Some(true) {
            new.retention_days = None;
        }
        if request.remote_cleanup.is_some() {
            new.remote_cleanup = request.remote_cleanup;
        }
        if request.clear_remote_cleanup == Some(true) {
            new.remote_cleanup = None;
        }
        new.updated_at = utc_now!();
        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Deletion guards ─────────────────────────────────────────────

    /// The two account kinds that can never be deleted, and the reason the
    /// caller gets back.
    ///
    /// Both branches return before the account collection is read, which is why
    /// this test needs no database — deliberately: the third rule (a view
    /// source) is covered by
    /// `access::tests::views_referencing_finds_only_views_that_name_the_account`,
    /// and a test that reached `list_all` could not run under a cargo filter
    /// (`SETTINGS` is parsed from argv, so a filtered run has no data root).
    #[test]
    fn a_held_or_journal_account_is_never_deletable() {
        let mut held = AccountModel::new(1, AccountCreateRequest::default()).unwrap();
        held.legal_hold = true;
        let error = AccountModel::ensure_deletable(&held).unwrap_err().to_string();
        assert!(error.contains("legal hold"), "{error}");

        let journal = AccountModel::new(
            1,
            AccountCreateRequest {
                kind: Some(AccountKind {
                    role: AccountKindRole::Journal,
                    target_address: None,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let error = AccountModel::ensure_deletable(&journal).unwrap_err().to_string();
        assert!(error.contains("journal archive"), "{error}");
    }

    /// An audit record of a view edit exists only when the scope actually
    /// moved: a rename must not leave a "view changed" line behind.
    #[test]
    fn view_config_change_is_recorded_only_when_the_scope_moved() {
        let source = ViewSource {
            account_id: 1,
            any_participant: vec!["a@example.com".into()],
            any_recipient: Vec::new(),
        };
        let narrow = ViewConfig {
            sources: vec![source.clone()],
        };
        let wide = ViewConfig {
            sources: vec![ViewSource {
                account_id: 1,
                any_participant: Vec::new(),
                any_recipient: Vec::new(),
            }],
        };

        assert!(ViewConfigChange::between(Some(&narrow), Some(&narrow)).is_none());
        assert!(ViewConfigChange::between(None, None).is_none());

        let widening = ViewConfigChange::between(Some(&narrow), Some(&wide))
            .expect("dropping a filter is a change");
        assert_eq!(widening.before, Some(narrow.clone()));
        assert_eq!(widening.after, Some(wide.clone()));

        // Attaching a view configuration to an account that had none.
        let attached = ViewConfigChange::between(None, Some(&narrow)).expect("a change");
        assert_eq!(attached.before, None);
    }

    // ── Account name normalization ──────────────────────────────────

    #[test]
    fn empty_or_whitespace_account_name_becomes_none() {
        for empty in ["".to_string(), "   ".to_string(), "\t\n ".to_string()] {
            assert_eq!(
                normalize_optional_name(Some(empty.clone())),
                None,
                "name: {empty:?}"
            );
        }
    }

    #[test]
    fn real_account_name_is_kept() {
        assert_eq!(
            normalize_optional_name(Some("Alice Lee".to_string())),
            Some("Alice Lee".to_string())
        );
        assert_eq!(normalize_optional_name(None), None);
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
    fn validate_extraction_rules_empty_pattern_rejected() {
        let rules = ExtractionRules {
            folders: FilterRule {
                include: vec!["".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_extraction_rules_whitespace_pattern_rejected() {
        let rules = ExtractionRules {
            senders: FilterRule {
                exclude: vec!["   ".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_extraction_rules_empty_extension_rejected() {
        let rules = ExtractionRules {
            extensions: FilterRule {
                include: vec!["".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_archive_rules_empty_pattern_rejected() {
        let rules = ArchiveRules {
            senders: FilterRule {
                include: vec!["".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_archive_rules_whitespace_subject_rejected() {
        let rules = ArchiveRules {
            subjects: FilterRule {
                exclude: vec![" \t ".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(rules.validate().is_err());
    }

    #[test]
    fn validate_archive_rules_empty_spam_header_rejected() {
        let rules = ArchiveRules {
            spam_headers: vec!["".into()],
            ..Default::default()
        };
        assert!(rules.validate().is_err());
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

    // ── Update: clear flags ─────────────────────────────────────────────

    #[test]
    fn update_clear_archive_rules_resets_to_none() {
        let account = AccountModel {
            archive_rules: Some(ArchiveRules {
                enabled: true,
                senders: FilterRule {
                    include: vec![r"@ok\.com$".into()],
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let updated = Account::apply_update_fields(
            &account,
            AccountUpdateRequest {
                clear_archive_rules: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(updated.archive_rules.is_none());
    }

    #[test]
    fn update_clear_extraction_rules_resets_to_none() {
        let account = AccountModel {
            extraction_rules: Some(ExtractionRules {
                enabled: true,
                extensions: FilterRule {
                    include: vec!["pdf".into()],
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let updated = Account::apply_update_fields(
            &account,
            AccountUpdateRequest {
                clear_extraction_rules: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(updated.extraction_rules.is_none());
    }

    #[test]
    fn validate_update_clear_archive_conflicts_with_archive_rules() {
        let account = AccountModel::default();
        let request = AccountUpdateRequest {
            clear_archive_rules: Some(true),
            archive_rules: Some(ArchiveRules::default()),
            ..Default::default()
        };
        assert!(request.validate_update_request(&account).is_err());
    }

    #[test]
    fn validate_update_clear_extraction_conflicts_with_extraction_rules() {
        let account = AccountModel::default();
        let request = AccountUpdateRequest {
            clear_extraction_rules: Some(true),
            extraction_rules: Some(ExtractionRules::default()),
            ..Default::default()
        };
        assert!(request.validate_update_request(&account).is_err());
    }

    // ── legal hold (Enterprise, DB-backed) ────────────────────────────

    use crate::settings::cli::SETTINGS;
    use crate::settings::dir::DATA_DIR_MANAGER;
    use std::sync::Once;

    /// Shared on-disk root for DB-backed tests. The global statics
    /// (SETTINGS → DATA_DIR_MANAGER → DB_MANAGER) freeze on first deref, so all
    /// tests in this test binary share this root; assertions are scoped to their
    /// own account ids/emails rather than assuming an empty database. The root
    /// is unique per process so a previous `cargo test` run can never leak
    /// memdb/index state into the next one.
    static TEST_ENV: Once = Once::new();
    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
        });
    }

    fn insert_account(id: u64, email: &str) -> AccountModel {
        let request = AccountCreateRequest {
            email: email.to_string(),
            login_name: None,
            account_name: None,
            imap: None,
            enabled: true,
            date_since: None,
            date_before: None,
            account_type: AccountType::NoSync,
            download_interval_min: None,
            download_batch_size: None,
            max_email_size_bytes: None,
            use_dangerous: false,
            pgp_key: None,
            imap_quota_bytes: None,
            imap_quota_window: None,
            auto_download_new_mailboxes: None,
            download_schedule: None,
            archive_rules: None,
            extraction_rules: None,
            override_from_headers: None,
            retention_days: None,
            remote_cleanup: None,
            kind: None,
            view: None,
        };
        let account = AccountModel::new(id, request).unwrap();
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();
        account
    }

    /// **Upgrade compatibility.** Rows written before `kind` and `view`
    /// existed must deserialize into the current model. Both fields are
    /// `Option` + `#[serde(default)]`, and this pins that: a backup restored
    /// from an older release, or an in-place upgrade, must not fail to load an
    /// account because a newer field is absent.
    #[test]
    fn account_rows_without_new_fields_deserialize() {
        let account = AccountModel::new(
            1,
            AccountCreateRequest {
                email: "legacy@test.example".to_string(),
                enabled: true,
                account_type: AccountType::NoSync,
                ..Default::default()
            },
        )
        .unwrap();

        let mut value = serde_json::to_value(&account).unwrap();
        let object = value.as_object_mut().unwrap();
        // What an older build wrote: the keys simply are not there.
        object.remove("kind");
        object.remove("view");
        object.remove("remote_cleanup");

        let parsed: AccountModel = serde_json::from_value(value)
            .expect("an account row from an older release must still parse");
        assert!(parsed.kind.is_none());
        assert!(parsed.view.is_none());
        assert!(parsed.remote_cleanup.is_none());
        assert!(matches!(parsed.account_type, AccountType::NoSync));
    }

    /// The existing storage values are part of the on-disk format: an upgrade
    /// must read `"IMAP"` and `"NoSync"` exactly as before.
    #[test]
    fn account_type_storage_names_are_stable() {
        for (value, expected) in [
            ("\"IMAP\"", AccountType::IMAP),
            ("\"NoSync\"", AccountType::NoSync),
            ("\"View\"", AccountType::View),
        ] {
            let parsed: AccountType = serde_json::from_str(value).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_string(&expected).unwrap(), value);
        }
    }

    /// A view shares its address with the person it shows (the WebUI suggests
    /// exactly that), and it never receives mail. SMTP delivery resolution must
    /// therefore skip views, or a real mailbox could become undeliverable
    /// depending on iteration order.
    #[tokio::test]
    async fn a_view_never_shadows_a_delivery_target() {
        init_test_env();
        let email = "shadow-check@test.example";

        let view = AccountModel::new(
            1,
            AccountCreateRequest {
                email: email.to_string(),
                enabled: true,
                account_type: AccountType::View,
                ..Default::default()
            },
        )
        .unwrap();
        insert_impl(DB_MANAGER.db(), view).unwrap();
        assert!(
            AccountModel::find_by_email(email).unwrap().is_none(),
            "a view is not a delivery target"
        );

        let local = AccountModel::new(
            2,
            AccountCreateRequest {
                email: email.to_string(),
                enabled: true,
                account_type: AccountType::NoSync,
                ..Default::default()
            },
        )
        .unwrap();
        insert_impl(DB_MANAGER.db(), local).unwrap();
        let found = AccountModel::find_by_email(email)
            .unwrap()
            .expect("the local account must be found despite the view sharing its address");
        assert!(matches!(found.account_type, AccountType::NoSync));
    }

    /// The REST extractor does **not** use serde: it uses the `poem_openapi`
    /// parser generated from the field types. That parser is what rejected an
    /// array sent to a field still typed as `Option<String>`, so the accepted
    /// wire shape is pinned here: arrays (including empty ones) are the format,
    /// and an absent filter defaults to an empty list.
    #[test]
    fn view_source_arrays_parse_through_the_openapi_parser() {
        use poem_openapi::types::ParseFromJSON;

        let parsed = <ViewSource as ParseFromJSON>::parse_from_json(Some(
            serde_json::json!({
                "account_id": 7,
                "any_participant": ["a@example.com", "b@example.com"],
                "any_recipient": []
            }),
        ))
        .expect("arrays must be accepted");
        assert_eq!(parsed.account_id, 7);
        assert_eq!(
            parsed.any_participant,
            vec!["a@example.com", "b@example.com"]
        );
        assert!(parsed.any_recipient.is_empty());

        let absent = <ViewSource as ParseFromJSON>::parse_from_json(Some(
            serde_json::json!({ "account_id": 8 }),
        ))
        .expect("an absent filter must default to an empty list");
        assert!(absent.any_participant.is_empty());
        assert!(absent.any_recipient.is_empty());
    }

    /// An older client (or a script) that predates `kind`/`view` keeps
    /// working: both are optional on the request.
    #[test]
    fn create_request_without_new_fields_parses() {
        let request: AccountCreateRequest = serde_json::from_str(
            r#"{"email":"legacy-client@test.example","enabled":true,"use_dangerous":false,"account_type":"NoSync"}"#,
        )
        .expect("a request from an older client must still parse");
        assert!(request.kind.is_none());
        assert!(request.view.is_none());
        assert!(request.remote_cleanup.is_none());

        let update: AccountUpdateRequest = serde_json::from_str(r#"{"enabled":false}"#)
            .expect("an update from an older client must still parse");
        assert!(update.view.is_none());
        assert!(update.remote_cleanup.is_none());
        assert!(update.clear_remote_cleanup.is_none());
    }

    #[tokio::test]
    async fn create_request_preserves_retention_days() {
        init_test_env();
        let request = AccountCreateRequest {
            email: "retention-create@test.example".to_string(),
            enabled: true,
            account_type: AccountType::NoSync,
            retention_days: Some(90),
            ..Default::default()
        };
        let account = AccountModel::new(1, request).unwrap();
        assert_eq!(account.retention_days, Some(90));
    }

    #[tokio::test]
    async fn legal_hold_place_sets_fields_and_persists() {
        init_test_env();
        let account = insert_account(9001, "hold-place@test.example");
        assert!(!account.is_on_hold());

        let placed =
            AccountModel::place_legal_hold(account.id, 7, Some("litigation case #1".into()))
                .unwrap();
        assert!(placed.is_on_hold());
        assert_eq!(placed.hold_reason.as_deref(), Some("litigation case #1"));
        assert_eq!(placed.hold_placed_by, Some(7));
        assert!(placed.hold_placed_at.is_some());

        // Persisted — a fresh read agrees.
        let reloaded = AccountModel::get(account.id).unwrap();
        assert!(reloaded.is_on_hold());
        assert_eq!(reloaded.hold_reason.as_deref(), Some("litigation case #1"));
        assert_eq!(reloaded.hold_placed_by, Some(7));
    }

    #[tokio::test]
    async fn legal_hold_release_clears_and_rearm_resets_release_stamps() {
        init_test_env();
        let account = insert_account(9002, "hold-release@test.example");
        AccountModel::place_legal_hold(account.id, 7, Some("hold".into())).unwrap();

        let released =
            AccountModel::release_legal_hold(account.id, 8, Some("resolved".into())).unwrap();
        assert!(!released.is_on_hold());
        assert_eq!(released.hold_released_by, Some(8));
        assert!(released.hold_released_at.is_some());
        // Releasing clears the placement stamps so a released account reads
        // clean (the placement record lives in the audit log, not the model).
        assert!(released.hold_placed_by.is_none());
        assert!(released.hold_placed_at.is_none());

        // Re-arming a hold clears the previous release stamps (current cycle).
        let rearmed =
            AccountModel::place_legal_hold(account.id, 7, Some("new hold".into())).unwrap();
        assert!(rearmed.is_on_hold());
        assert!(rearmed.hold_released_by.is_none());
        assert!(rearmed.hold_released_at.is_none());
        assert_eq!(rearmed.hold_reason.as_deref(), Some("new hold"));
    }

    #[tokio::test]
    async fn legal_hold_list_returns_only_held_accounts() {
        init_test_env();
        let free = insert_account(9003, "hold-free@test.example");
        let held = insert_account(9004, "hold-held@test.example");
        AccountModel::place_legal_hold(held.id, 7, Some("freeze".into())).unwrap();

        let list = AccountModel::list_legal_hold_accounts().unwrap();
        assert!(
            list.iter().any(|a| a.id == held.id),
            "held account must be listed"
        );
        assert!(
            !list.iter().any(|a| a.id == free.id),
            "free account must not be listed"
        );
    }
}

#[cfg(test)]
mod remote_cleanup_config_tests {
    use super::{RemoteCleanupConfig, REMOTE_CLEANUP_FOLDER};

    /// Older builds stored a `verify_mode` on the cleanup config. Verification
    /// is now metadata-only and has no setting, so the field is gone — but a
    /// stored account must keep loading instead of being rejected as corrupt.
    #[test]
    fn a_legacy_verify_mode_is_ignored() {
        let stored = serde_json::json!({
            "enabled": true,
            "eligible_after_archive_days": 30,
            "max_messages_per_run": 50,
            "skip_flagged": true,
            "verify_mode": "strict",
            "some_future_field": 1
        });
        let config: RemoteCleanupConfig =
            serde_json::from_value(stored).expect("older configs must still load");
        assert!(config.enabled);
        assert_eq!(config.max_messages_per_run, 50);
    }

    #[test]
    fn defaults_are_conservative_and_valid() {
        let config = RemoteCleanupConfig::default();
        assert!(!config.enabled, "cleanup must be opt-in");
        assert_eq!(config.eligible_after_archive_days, 30);
        assert!(config.skip_flagged);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn a_zero_grace_period_is_allowed() {
        // 0 means "eligible on the next run" and must stay a legal choice: the
        // move is reversible, so an operator emptying a source mailbox that is
        // still being archived has a real use for it.
        let mut config = RemoteCleanupConfig::default();
        config.eligible_after_archive_days = 0;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn per_run_cap_is_bounded() {
        let mut config = RemoteCleanupConfig::default();
        config.max_messages_per_run = 0;
        assert!(config.validate().is_err());
        config.max_messages_per_run = super::MAX_REMOTE_CLEANUP_PER_RUN + 1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn destination_cannot_be_a_source_folder() {
        let mut config = RemoteCleanupConfig::default();
        config.folders = Some(vec![REMOTE_CLEANUP_FOLDER.to_string()]);
        assert!(config.validate().is_err());
    }

    #[test]
    fn schedule_must_be_valid_cron() {
        let mut config = RemoteCleanupConfig::default();
        config.schedule = Some("not a cron".into());
        assert!(config.validate().is_err());
        config.schedule = Some("0 0 3 * * *".into());
        assert!(config.validate().is_ok());
    }
}
