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

use std::collections::HashSet;

use crate::modules::account::migration::AccountModel;
use crate::modules::cache::imap::mailbox::MailBox;
use crate::modules::error::code::ErrorCode;
use crate::modules::utils::create_hash;
use crate::modules::{error::BichonResult, indexer::schema::SchemaTools};
use crate::raise_error;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tantivy::schema::Facet;
use tantivy::{doc, schema::Value, TantivyDocument};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct Envelope {
    pub id: u64,
    pub message_id: String,
    pub account_id: u64,
    pub account_email: Option<String>,
    pub mailbox_id: u64,
    pub mailbox_name: Option<String>,
    pub uid: u32,
    pub subject: String,
    pub text: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub date: i64,
    pub internal_date: i64,
    pub size: u32,
    pub thread_id: u64,
    pub attachments: Vec<String>,
    pub tags: Option<Vec<String>>,
}

fn extract_u64_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<u64> {
    let value = document.get_first(field).ok_or_else(|| {
        raise_error!(
            format!("miss '{}' field in tantivy document", stringify!(field)),
            ErrorCode::InternalError
        )
    })?;
    value.as_u64().ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a u64", stringify!(field)),
            ErrorCode::InternalError
        )
    })
}

fn extract_i64_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<i64> {
    let value = document.get_first(field).ok_or_else(|| {
        raise_error!(
            format!("miss '{}' field in tantivy document", stringify!(field)),
            ErrorCode::InternalError
        )
    })?;
    value.as_i64().ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a i64", stringify!(field)),
            ErrorCode::InternalError
        )
    })
}

fn extract_string_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<String> {
    let value = document.get_first(field).ok_or_else(|| {
        raise_error!(
            format!("'{}' field not found", stringify!(field)),
            ErrorCode::InternalError
        )
    })?;
    value.as_str().map(|s| s.to_string()).ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a string", stringify!(field)),
            ErrorCode::InternalError
        )
    })
}

fn extract_vec_string_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<Vec<String>> {
    let value = document
        .get_all(field)
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    Ok(value)
}

impl Envelope {
    pub fn to_document(&self, mailbox_id: u64) -> BichonResult<TantivyDocument> {
        let fields = SchemaTools::envelope_fields();
        let mut doc = doc!();
        doc.add_u64(fields.f_id, self.id);
        doc.add_text(fields.f_message_id, &self.message_id);
        doc.add_u64(fields.f_account_id, self.account_id);
        doc.add_u64(fields.f_mailbox_id, mailbox_id);
        doc.add_u64(fields.f_uid, self.uid as u64);
        doc.add_text(fields.f_subject, &self.subject);
        doc.add_text(fields.f_text, &self.text);
        doc.add_text(fields.f_from, &self.from);
        for to in &self.to {
            doc.add_text(fields.f_to, to);
        }
        for cc in &self.cc {
            doc.add_text(fields.f_cc, cc);
        }
        for bcc in &self.bcc {
            doc.add_text(fields.f_bcc, bcc);
        }
        doc.add_i64(fields.f_date, self.date);
        doc.add_i64(fields.f_internal_date, self.internal_date);
        doc.add_u64(fields.f_size, self.size as u64);
        doc.add_u64(fields.f_thread_id, self.thread_id);
        for att in &self.attachments {
            doc.add_text(fields.f_attachments, att);
        }
        doc.add_bool(fields.f_has_attachment, self.attachments.len() > 0);
        Ok(doc)
    }

    pub async fn from_tantivy_doc(doc: &TantivyDocument) -> BichonResult<Self> {
        let fields = SchemaTools::envelope_fields();
        let account_id = extract_u64_field(doc, fields.f_account_id)?;
        let message_id = extract_string_field(doc, fields.f_message_id)?;
        let mailbox_id = extract_u64_field(doc, fields.f_mailbox_id)?;
        let id = create_hash(account_id, &message_id);
        let full_text = extract_string_field(doc, fields.f_text)?;

        // Take up to the first 500 characters as a preview;
        let preview = if full_text.chars().count() > 500 {
            full_text.chars().take(500).collect::<String>() + "..."
        } else {
            full_text
        };

        let tags: Vec<String> = doc
            .get_all(fields.f_tags)
            .filter_map(|value| value.as_facet())
            .map(|facet_encoded_str| {
                Facet::from_encoded(facet_encoded_str.as_bytes().to_vec())
                    .ok()
                    .map(|facet| facet.to_string())
            })
            .flatten()
            .collect();
        let account_email = AccountModel::find(account_id).await?.map(|a| a.email);

        let mailboxes = MailBox::list_all(account_id).await?;
        let mailbox_name = mailboxes
            .iter()
            .find(|m| m.id == mailbox_id)
            .map(|m| m.name.clone());

        let envelope = Envelope {
            id,
            account_id,
            account_email,
            mailbox_id,
            mailbox_name,
            message_id: extract_string_field(doc, fields.f_message_id)?,
            uid: extract_u64_field(doc, fields.f_uid)? as u32,
            subject: extract_string_field(doc, fields.f_subject)?,
            text: preview,
            from: extract_string_field(doc, fields.f_from)?,
            to: extract_vec_string_field(doc, fields.f_to)?,
            cc: extract_vec_string_field(doc, fields.f_cc)?,
            bcc: extract_vec_string_field(doc, fields.f_bcc)?,
            date: {
                let d = extract_i64_field(doc, fields.f_date)?;
                if d == 0 {
                    extract_i64_field(doc, fields.f_internal_date)?
                } else {
                    d
                }
            },
            internal_date: extract_i64_field(doc, fields.f_internal_date)?,
            size: extract_u64_field(doc, fields.f_size)? as u32,
            thread_id: extract_u64_field(doc, fields.f_thread_id)?,
            attachments: extract_vec_string_field(doc, fields.f_attachments)?,
            tags: Some(tags),
        };
        Ok(envelope)
    }
}

pub async fn extract_contacts(doc: &TantivyDocument) -> BichonResult<HashSet<String>> {
    let fields = SchemaTools::envelope_fields();
    let mut all_contacts = HashSet::new();

    if let Ok(from_val) = extract_string_field(doc, fields.f_from) {
        if !from_val.is_empty() {
            all_contacts.insert(from_val);
        }
    }

    let multi_fields = [fields.f_to, fields.f_cc, fields.f_bcc];

    for field in multi_fields {
        if let Ok(vals) = extract_vec_string_field(doc, field) {
            for v in vals {
                if !v.is_empty() {
                    all_contacts.insert(v);
                }
            }
        }
    }

    Ok(all_contacts)
}
