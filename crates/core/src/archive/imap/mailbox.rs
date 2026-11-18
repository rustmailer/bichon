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

use crate::{
    decode_mailbox_name, encode_mailbox_name, raise_error,
    {
        database::{
            batch_delete_impl, batch_insert_impl, batch_upsert_impl, delete_impl, filter_impl,
            find_impl, manager::DB_MANAGER, MemDbModel,
        },
        error::{code::ErrorCode, BichonResult},
    },
};
use async_imap::types::{Name, NameAttribute};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct MailBox {
    /// The unique identifier for the mailbox
    pub id: u64,
    /// The ID of the account associated with the mailbox
    pub account_id: u64,
    /// The unique, decoded, human-readable name of the mailbox (e.g., "INBOX", "Sent Items").
    /// This is the decoded name as presented to users, derived from the IMAP server's mailbox name
    /// (e.g., after decoding UTF-7 or other encodings per RFC 3501).
    pub name: String,
    /// Optional delimiter used to separate mailbox names in a hierarchy (e.g., "/" or ".").
    /// Used in IMAP to structure nested mailboxes (e.g., "INBOX/Archive").
    pub delimiter: Option<String>,
    /// List of attributes associated with the mailbox (e.g., `\NoSelect`, `\Deleted`).
    /// These indicate special properties, such as whether the mailbox can hold messages.
    pub attributes: Vec<Attribute>,
    /// The number of messages that currently exist in the mailbox.
    pub exists: u32,
    /// Optional number of unseen messages in the mailbox (i.e., messages without the `\Seen` flag).
    pub unseen: Option<u32>,
    /// The next unique identifier (UID) that will be assigned to a new message in the mailbox.
    /// If `None`, the IMAP server has not provided this information.
    pub uid_next: Option<u32>,
    /// The validity identifier for UIDs in this mailbox, used to ensure UID consistency across sessions.
    /// If `None`, the IMAP server has not provided this information.
    pub uid_validity: Option<u32>,
    /// The highest UID that has been successfully downloaded and stored locally.
    /// Used for incremental sync: next fetch starts from `highest_uid + 1`.
    /// If `None`, a fallback query against the Tantivy index will be performed once.
    pub highest_uid: Option<u32>,
}

impl MemDbModel for MailBox {
    fn collection() -> &'static str {
        "mailboxes"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

impl MailBox {
    /// Ensure an account has an INBOX row and return its id.
    ///
    /// Shared by the SMTP ingest path and the journal handler. The row is
    /// created **only when it is missing**: on a synced account the INBOX row
    /// is owned by the IMAP sync, which maintains `uid_validity`,
    /// `highest_uid` and `uid_next`; a blind upsert with those fields `None`
    /// would make the next reconcile treat the mailbox as invalid and rebuild
    /// it, losing the local copy of a large mailbox if the rebuild is
    /// interrupted (#297). For a local (NoSync) account there is no IMAP owner,
    /// so creating the row is the whole job.
    pub fn ensure_local_inbox(account_id: u64) -> BichonResult<u64> {
        let mailbox_id = crate::utils::create_hash(account_id, "INBOX");
        if Self::find_mailbox(account_id, mailbox_id)?.is_none() {
            let mailbox = MailBox {
                id: mailbox_id,
                account_id,
                name: "INBOX".into(),
                delimiter: Some("/".to_string()),
                attributes: vec![Attribute {
                    attr: AttributeEnum::Extension,
                    extension: Some("CreatedByBichon".into()),
                }],
                exists: 0,
                unseen: None,
                uid_next: None,
                uid_validity: None,
                highest_uid: None,
            };
            Self::batch_upsert(&[mailbox])?;
        }
        Ok(mailbox_id)
    }

    pub fn encoded_name(&self) -> String {
        encode_mailbox_name!(&self.name)
    }

    pub fn get(id: u64) -> BichonResult<MailBox> {
        let result = find_impl::<MailBox>(DB_MANAGER.db(), &id.to_string())?;
        Ok(result.ok_or_else(|| {
            raise_error!(
                format!("mailbox {} not found", id),
                ErrorCode::InternalError
            )
        })?)
    }

    pub fn delete(id: u64) -> BichonResult<()> {
        delete_impl::<MailBox>(DB_MANAGER.db(), &id.to_string())
    }

    pub fn list_all(account_id: u64) -> BichonResult<Vec<MailBox>> {
        filter_impl::<MailBox, _>(DB_MANAGER.db(), move |m| m.account_id == account_id)
    }

    pub fn find_mailbox(account_id: u64, mailbox_id: u64) -> BichonResult<Option<MailBox>> {
        let all = filter_impl::<MailBox, _>(DB_MANAGER.db(), move |m| m.account_id == account_id)?;
        Ok(all.into_iter().find(|m| m.id == mailbox_id))
    }

    pub fn batch_insert(mailboxes: &[MailBox]) -> BichonResult<()> {
        batch_insert_impl(DB_MANAGER.db(), mailboxes.to_vec())
    }

    pub fn batch_upsert(mailboxes: &[MailBox]) -> BichonResult<()> {
        batch_upsert_impl(DB_MANAGER.db(), mailboxes.to_vec())
    }

    /// Advance the incremental-sync watermark after a fully processed download
    /// batch; the watermark never moves backwards.
    ///
    /// The download batches are generated from a UID-sorted list, so every UID
    /// of a later batch is greater than every UID of an earlier one. The batch
    /// maximum is therefore a safe contiguous cursor: everything at or below it
    /// was enumerated and processed in this attempt (archived, deduped,
    /// filtered by archive rules or parse-skipped) — the same set the
    /// folder-completion watermark covers. Persisting it per batch is what lets
    /// a throttled, interrupted or stopped round resume at the last completed
    /// batch instead of re-downloading the whole range.
    ///
    /// The row is read fresh on every call: callers hold a snapshot taken at
    /// the start of the round, and writing that snapshot back would undo the
    /// progress an earlier batch already persisted.
    pub fn advance_watermark(mailbox_id: u64, uid: u32) -> BichonResult<()> {
        let mut mailbox = Self::get(mailbox_id)?;
        if mailbox.highest_uid.is_some_and(|current| current >= uid) {
            return Ok(());
        }
        mailbox.highest_uid = Some(uid);
        Self::batch_upsert(&[mailbox])
    }

    pub fn clean(account_id: u64) -> BichonResult<()> {
        let mailboxes =
            filter_impl::<MailBox, _>(DB_MANAGER.db(), move |m| m.account_id == account_id)?;
        let keys: Vec<String> = mailboxes.iter().map(|m| m.id.to_string()).collect();
        if !keys.is_empty() {
            batch_delete_impl::<MailBox>(DB_MANAGER.db(), keys)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Attribute {
    pub attr: AttributeEnum,
    pub extension: Option<String>,
}

impl Attribute {
    pub fn new(attr: AttributeEnum, extension: Option<String>) -> Self {
        Self { attr, extension }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum AttributeEnum {
    NoInferiors,
    NoSelect,
    Marked,
    Unmarked,
    All,
    Archive,
    Drafts,
    Flagged,
    Junk,
    Sent,
    Trash,
    Extension,
    Unknown,
}

impl From<&Name> for MailBox {
    fn from(value: &Name) -> Self {
        let name = decode_mailbox_name!(value.name().to_string());
        let delimiter = value.delimiter().map(|f| f.to_owned());
        let attributes: Vec<Attribute> = value.attributes().iter().map(|na| na.into()).collect();
        //The remaining parts will be supplemented during the examine_mailbox process.
        MailBox {
            name,
            delimiter,
            attributes,
            ..Default::default() //has_synced is initialized to false here
        }
    }
}

impl From<&NameAttribute<'_>> for Attribute {
    fn from(value: &NameAttribute) -> Self {
        match value {
            NameAttribute::NoInferiors => Attribute::new(AttributeEnum::NoInferiors, None),
            NameAttribute::NoSelect => Attribute::new(AttributeEnum::NoSelect, None),
            NameAttribute::Marked => Attribute::new(AttributeEnum::Marked, None),
            NameAttribute::Unmarked => Attribute::new(AttributeEnum::Unmarked, None),
            NameAttribute::All => Attribute::new(AttributeEnum::All, None),
            NameAttribute::Archive => Attribute::new(AttributeEnum::Archive, None),
            NameAttribute::Drafts => Attribute::new(AttributeEnum::Drafts, None),
            NameAttribute::Flagged => Attribute::new(AttributeEnum::Flagged, None),
            NameAttribute::Junk => Attribute::new(AttributeEnum::Junk, None),
            NameAttribute::Sent => Attribute::new(AttributeEnum::Sent, None),
            NameAttribute::Trash => Attribute::new(AttributeEnum::Trash, None),
            NameAttribute::Extension(s) => {
                Attribute::new(AttributeEnum::Extension, Some(s.to_string()))
            }
            _ => Attribute::new(AttributeEnum::Unknown, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    // The helpers read the process-global DB, so seed the same shared test
    // root the other DB-backed tests use. Assertions are scoped to the
    // mailbox ids each test creates.
    static TEST_ENV: Once = Once::new();

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*crate::settings::cli::SETTINGS;
            let _ = &*crate::database::manager::DB_MANAGER;
        });
    }

    fn mailbox(id: u64, highest_uid: Option<u32>) -> MailBox {
        MailBox {
            id,
            account_id: 9_410_000,
            name: format!("watermark-{id}"),
            exists: 42,
            uid_validity: Some(7),
            highest_uid,
            ..Default::default()
        }
    }

    /// The per-batch cursor must never move backwards: a retried batch, an
    /// out-of-order write or a stale snapshot must not make the next round
    /// re-fetch (or skip) mail.
    #[tokio::test]
    async fn advance_watermark_only_moves_forward() {
        init_test_env();
        let id = 9_410_001;
        MailBox::batch_insert(&[mailbox(id, None)]).unwrap();

        MailBox::advance_watermark(id, 30).unwrap();
        assert_eq!(MailBox::get(id).unwrap().highest_uid, Some(30));

        MailBox::advance_watermark(id, 12).unwrap();
        assert_eq!(
            MailBox::get(id).unwrap().highest_uid,
            Some(30),
            "an earlier batch must not regress the cursor"
        );

        MailBox::advance_watermark(id, 60).unwrap();
        assert_eq!(MailBox::get(id).unwrap().highest_uid, Some(60));
    }

    /// The helper re-reads the row before writing, so it preserves the fields
    /// a round-start snapshot does not carry (or carries stale).
    #[tokio::test]
    async fn advance_watermark_preserves_the_rest_of_the_row() {
        init_test_env();
        let id = 9_410_002;
        MailBox::batch_insert(&[mailbox(id, Some(100))]).unwrap();

        MailBox::advance_watermark(id, 130).unwrap();

        let after = MailBox::get(id).unwrap();
        assert_eq!(after.highest_uid, Some(130));
        assert_eq!(after.uid_validity, Some(7));
        assert_eq!(after.exists, 42);
        assert_eq!(after.name, format!("watermark-{id}"));
    }
}
