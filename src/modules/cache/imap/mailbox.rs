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

use crate::{
    decode_mailbox_name, encode_mailbox_name,
    modules::{
        database::{
            async_filter_by_secondary_key_impl, async_find_impl, batch_delete_impl,
            batch_insert_impl, batch_upsert_impl, delete_impl, filter_by_secondary_key_impl,
            manager::DB_MANAGER,
        },
        error::{code::ErrorCode, BichonResult},
    },
    raise_error,
};
use async_imap::types::{Name, NameAttribute};
use itertools::Itertools;
use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 1, version = 1)]
#[native_db]
pub struct MailBox {
    /// The unique identifier for the mailbox
    #[primary_key]
    pub id: u64,
    /// The ID of the account associated with the mailbox
    #[secondary_key]
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
}

impl MailBox {
    pub fn encoded_name(&self) -> String {
        encode_mailbox_name!(&self.name)
    }

    pub async fn get(id: u64) -> BichonResult<MailBox> {
        let result = async_find_impl::<MailBox>(DB_MANAGER.envelope_db(), id).await?;
        Ok(result.ok_or_else(|| {
            raise_error!(
                format!("mailbox {} not found", id),
                ErrorCode::InternalError
            )
        })?)
    }

    pub async fn delete(id: u64) -> BichonResult<()> {
        delete_impl(DB_MANAGER.envelope_db(), move |rw| {
            rw.get()
                .primary::<MailBox>(id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| raise_error!("mailbox missing".into(), ErrorCode::InternalError))
        })
        .await
    }

    pub async fn list_all(account_id: u64) -> BichonResult<Vec<MailBox>> {
        async_filter_by_secondary_key_impl(
            DB_MANAGER.envelope_db(),
            MailBoxKey::account_id,
            account_id,
        )
        .await
    }

    pub fn find_mailbox(account_id: u64, mailbox_id: u64) -> BichonResult<Option<MailBox>> {
        let all: Vec<MailBox> = filter_by_secondary_key_impl(
            DB_MANAGER.envelope_db(),
            MailBoxKey::account_id,
            account_id,
        )?;
        Ok(all.into_iter().find(|m| m.id == mailbox_id))
    }

    pub async fn batch_insert(mailboxes: &[MailBox]) -> BichonResult<()> {
        batch_insert_impl(DB_MANAGER.envelope_db(), mailboxes.to_vec()).await
    }

    pub async fn batch_upsert(mailboxes: &[MailBox]) -> BichonResult<()> {
        batch_upsert_impl(DB_MANAGER.envelope_db(), mailboxes.to_vec()).await
    }

    pub async fn clean(account_id: u64) -> BichonResult<()> {
        batch_delete_impl(DB_MANAGER.envelope_db(), move |rw| {
            let mailboxes: Vec<MailBox> = rw
                .scan()
                .secondary::<MailBox>(MailBoxKey::account_id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .start_with(account_id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            Ok(mailboxes)
        })
        .await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct Attribute {
    pub attr: AttributeEnum,
    pub extension: Option<String>,
}

impl Attribute {
    pub fn new(attr: AttributeEnum, extension: Option<String>) -> Self {
        Self { attr, extension }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, Enum)]
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
