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

//use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tantivy::{
    schema::{Facet, Value},
    TantivyDocument,
};

use crate::{
    raise_error,
    {
        account::migration::AccountModel,
        archive::imap::mailbox::MailBox,
        error::{code::ErrorCode, BichonResult},
        message::content::AttachmentInfo,
        store::{
            envelope::Envelope,
            tantivy::{
                fields::{
                    F_ACCOUNT_ID, F_ATTACHMENTS, F_ATTACHMENT_CATEGORY, F_ATTACHMENT_CONTENT_TYPE,
                    F_ATTACHMENT_COUNT, F_CLEANUP_MOVED, F_CONTENT_HASH, F_DATE, F_ENVELOPE_ID,
                    F_FROM, F_HAS_TEXT,
                    F_ID, F_IMAP_UID, F_INGEST_AT, F_INTERNAL_DATE, F_IS_INDEXED, F_IS_MESSAGE,
                    F_IS_OCR,
                    F_MAILBOX_ID, F_MESSAGE_ID, F_PREVIEW, F_REGULAR_ATTACHMENT_COUNT, F_SHARD_ID,
                    F_SIZE, F_SUBJECT, F_TEXT, F_THREAD_ID, F_UID,
                },
                schema::SchemaTools,
            },
        },
    },
};

#[derive(Debug, Clone)]
pub struct EnvelopeWithAttachments {
    pub envelope: Envelope,
    pub attachments: Option<Vec<AttachmentInfo>>,
}

#[cfg(test)]
mod attachment_storage_tests {
    use super::*;

    #[test]
    fn deletion_references_follow_raw_and_legacy_blob_keys() {
        let envelope = EnvelopeWithAttachments {
            envelope: Envelope {
                imap_uid: 1,
                ..Default::default()
            },
            attachments: Some(vec![
                AttachmentInfo {
                    content_hash: "decoded-file".into(),
                    raw_content_hash: Some("encoded-body".into()),
                    ..Default::default()
                },
                AttachmentInfo {
                    content_hash: "legacy-body".into(),
                    ..Default::default()
                },
            ]),
        };
        let doc = envelope.to_document("", 0).expect("index attachments");
        let keys: Vec<_> = doc
            .get_all(SchemaTools::email_fields().f_attachment_content_hash)
            .filter_map(|value| value.as_str())
            .collect();
        assert_eq!(keys, vec!["encoded-body", "legacy-body"]);
    }

    /// No write path may index a document without a stable UID. Zero is not a
    /// valid IMAP UID, and the IMAP server sorts a mailbox by that field, so it
    /// would be served to clients as UID 0. The refusal here is what turns a
    /// forgotten assignment into an immediate error at the offending path.
    #[test]
    fn a_document_without_a_stable_uid_is_refused() {
        let envelope = EnvelopeWithAttachments {
            envelope: Envelope::default(),
            attachments: None,
        };
        let err = envelope
            .to_document("", 0)
            .expect_err("imap_uid 0 must not be indexable");
        assert!(
            format!("{err:?}").contains("no stable IMAP UID"),
            "unexpected error: {err:?}"
        );

        // The same document with an assigned UID is fine.
        let envelope = EnvelopeWithAttachments {
            envelope: Envelope {
                imap_uid: 7,
                ..Default::default()
            },
            attachments: None,
        };
        assert!(envelope.to_document("", 0).is_ok());
    }
}

impl EnvelopeWithAttachments {
    pub fn to_document(&self, body_text: &str, shard_id: u64) -> BichonResult<TantivyDocument> {
        let fields = SchemaTools::email_fields();
        // A document must never reach the index without a stable UID. 0 is not a
        // valid IMAP UID (RFC 3501), and `ordered_mailbox_uids` sorts by that
        // field, so a zero would be served to clients as UID 0. Refusing here
        // turns "a write path forgot to assign one" into an immediate error at
        // that path instead of a document only a later audit notices.
        if self.envelope.imap_uid == 0 {
            return Err(raise_error!(
                format!(
                    "envelope {} has no stable IMAP UID (0); assign one with \
                     archive::imap::stable_uid::next_uid before writing it",
                    self.envelope.id
                ),
                ErrorCode::InternalError
            ));
        }
        let mut doc = TantivyDocument::new();

        doc.add_text(fields.f_id, &self.envelope.id);
        doc.add_text(fields.f_message_id, &self.envelope.message_id);
        doc.add_u64(fields.f_account_id, self.envelope.account_id);
        doc.add_u64(fields.f_mailbox_id, self.envelope.mailbox_id);
        doc.add_u64(fields.f_uid, self.envelope.uid as u64);
        doc.add_u64(fields.f_imap_uid, self.envelope.imap_uid as u64);
        doc.add_text(fields.f_subject, &self.envelope.subject);
        doc.add_text(fields.f_subject_tri, &self.envelope.subject);
        doc.add_text(fields.f_preview, &self.envelope.preview);
        doc.add_text(fields.f_content_hash, &self.envelope.content_hash);
        doc.add_text(fields.f_from, &self.envelope.from);
        doc.add_text(fields.f_from_text, &self.envelope.from);
        doc.add_text(fields.f_from_text_tri, &self.envelope.from);
        // Names and the pre-override address are searchable through the same
        // text/trigram fields: "search the sender" must keep working for the
        // alias the user actually received the mail at.
        if let Some(name) = &self.envelope.from_name {
            doc.add_text(fields.f_from_name, name);
            doc.add_text(fields.f_from_text, name);
            doc.add_text(fields.f_from_text_tri, name);
        }
        if let Some(wire_from) = &self.envelope.wire_from {
            doc.add_text(fields.f_wire_from, wire_from);
            doc.add_text(fields.f_from_text, wire_from);
            doc.add_text(fields.f_from_text_tri, wire_from);
        }
        doc.add_text(fields.f_body, body_text);

        for to in &self.envelope.to {
            doc.add_text(fields.f_to, to);
            doc.add_text(fields.f_to_text, to);
            doc.add_text(fields.f_to_text_tri, to);
        }
        for cc in &self.envelope.cc {
            doc.add_text(fields.f_cc, cc);
            doc.add_text(fields.f_cc_text, cc);
            doc.add_text(fields.f_cc_text_tri, cc);
        }
        for bcc in &self.envelope.bcc {
            doc.add_text(fields.f_bcc, bcc);
            doc.add_text(fields.f_bcc_text, bcc);
            doc.add_text(fields.f_bcc_text_tri, bcc);
        }

        doc.add_i64(fields.f_date, self.envelope.date);
        doc.add_i64(fields.f_internal_date, self.envelope.internal_date);
        doc.add_u64(fields.f_size, self.envelope.size as u64);
        doc.add_i64(fields.f_ingest_at, self.envelope.ingest_at);
        doc.add_bool(fields.f_cleanup_moved, self.envelope.cleanup_moved);
        doc.add_text(fields.f_thread_id, &self.envelope.thread_id);

        if let Some(ref atts) = self.attachments {
            let atts_json = serde_json::to_string(atts).unwrap_or_else(|_| "[]".to_string());
            doc.add_text(fields.f_attachments, atts_json);
            for att in atts {
                if !att.is_inline() {
                    if let Some(ref filename) = att.filename {
                        doc.add_text(fields.f_attachment_name_text, filename);
                        doc.add_text(fields.f_attachment_name_text_tri, filename);
                        doc.add_text(fields.f_attachment_name_exact, filename);
                    }
                    if let Some(ext) = att.get_extension() {
                        doc.add_text(fields.f_attachment_ext, ext);
                    }
                    let category = att.get_category().to_string();
                    doc.add_text(fields.f_attachment_category, category);
                    let file_type = att.file_type.to_lowercase();
                    doc.add_text(fields.f_attachment_content_type, file_type);
                }
                // Blob deletion must follow the encoded storage key, including
                // shared blobs referenced by other messages and legacy records.
                doc.add_text(fields.f_attachment_content_hash, att.storage_hash());
            }
        }

        if let Some(tags) = &self.envelope.tags {
            for tag in tags {
                doc.add_facet(fields.f_tags, tag);
            }
        }

        doc.add_u64(
            fields.f_attachment_count,
            self.envelope.attachment_count as u64,
        );
        doc.add_u64(
            fields.f_regular_attachment_count,
            self.envelope.regular_attachment_count as u64,
        );
        doc.add_u64(fields.f_shard_id, shard_id);
        Ok(doc)
    }

    pub fn from_tantivy_doc(doc: &TantivyDocument) -> BichonResult<Self> {
        let fields = SchemaTools::email_fields();

        let attachments_raw = extract_string_field(doc, fields.f_attachments, F_ATTACHMENTS).ok();
        let attachments: Option<Vec<AttachmentInfo>> =
            attachments_raw.and_then(|json| serde_json::from_str(&json).ok());

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

        let account_id = extract_u64_field(doc, fields.f_account_id, F_ACCOUNT_ID)?;
        let mailbox_id = extract_u64_field(doc, fields.f_mailbox_id, F_MAILBOX_ID)?;

        let account = AccountModel::get(account_id)?;
        let mailbox = MailBox::get(mailbox_id)?;
        let envelope = Envelope {
            id: extract_string_field(doc, fields.f_id, F_ID)?,
            message_id: extract_string_field(doc, fields.f_message_id, F_MESSAGE_ID)?,
            account_id,
            account_email: Some(account.email), //https://github.com/rustmailer/bichon/issues/306
            account_name: account.account_name,
            mailbox_id,
            mailbox_name: Some(mailbox.name),
            uid: extract_u64_field(doc, fields.f_uid, F_UID)? as u32,
            // Required on purpose: every document written by the current schema
            // carries it, so a miss means a schema/format mismatch (see
            // store::tantivy::format) rather than a recoverable gap.
            imap_uid: extract_u64_field(doc, fields.f_imap_uid, F_IMAP_UID)? as u32,
            // A document written before this field existed reads as "not
            // moved", which is also what a reindex produces (see
            // `bichon-admin reindex`): the cleanup ledger is not rebuilt, so
            // every message becomes a candidate again.
            cleanup_moved: extract_bool_field(doc, fields.f_cleanup_moved, F_CLEANUP_MOVED)
                .unwrap_or(false),
            subject: extract_string_field(doc, fields.f_subject, F_SUBJECT)?,
            preview: extract_string_field(doc, fields.f_preview, F_PREVIEW).unwrap_or_default(),
            from: extract_string_field(doc, fields.f_from, F_FROM)?,
            from_name: extract_option_string_field(doc, fields.f_from_name)?,
            wire_from: extract_option_string_field(doc, fields.f_wire_from)?,
            to: extract_vec_string_field(doc, fields.f_to)?,
            cc: extract_vec_string_field(doc, fields.f_cc)?,
            bcc: extract_vec_string_field(doc, fields.f_bcc)?,
            date: extract_i64_field(doc, fields.f_date, F_DATE)?,
            internal_date: extract_i64_field(doc, fields.f_internal_date, F_INTERNAL_DATE)?,
            size: extract_u64_field(doc, fields.f_size, F_SIZE)? as u32,
            thread_id: extract_string_field(doc, fields.f_thread_id, F_THREAD_ID)?,
            attachment_count: extract_u64_field(doc, fields.f_attachment_count, F_ATTACHMENT_COUNT)?
                as usize,
            regular_attachment_count: extract_u64_field(
                doc,
                fields.f_regular_attachment_count,
                F_REGULAR_ATTACHMENT_COUNT,
            )? as usize,
            tags: (!tags.is_empty()).then_some(tags),
            content_hash: extract_string_field(doc, fields.f_content_hash, F_CONTENT_HASH)?,
            ingest_at: extract_i64_field(doc, fields.f_ingest_at, F_INGEST_AT)?,
        };

        Ok(EnvelopeWithAttachments {
            envelope,
            attachments,
        })
    }
}

fn extract_u64_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
    field_name: &str,
) -> BichonResult<u64> {
    extract_option_u64_field(document, field)?.ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a u64", field_name),
            ErrorCode::InternalError
        )
    })
}

fn extract_option_u64_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<Option<u64>> {
    Ok(document.get_first(field).and_then(|v| v.as_u64()))
}

fn extract_bool_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
    field_name: &str,
) -> BichonResult<bool> {
    let value = document.get_first(field).ok_or_else(|| {
        raise_error!(
            format!("miss '{}' field in tantivy document", field_name),
            ErrorCode::InternalError
        )
    })?;
    value.as_bool().ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a u64", field_name),
            ErrorCode::InternalError
        )
    })
}

fn extract_i64_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
    field_name: &str,
) -> BichonResult<i64> {
    let value = document.get_first(field).ok_or_else(|| {
        raise_error!(
            format!("miss '{}' field in tantivy document", field_name),
            ErrorCode::InternalError
        )
    })?;
    value.as_i64().ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a i64", field_name),
            ErrorCode::InternalError
        )
    })
}

fn extract_string_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
    field_name: &str,
) -> BichonResult<String> {
    extract_option_string_field(document, field)?.ok_or_else(|| {
        raise_error!(
            format!("'{}' field is not a string", field_name),
            ErrorCode::InternalError
        )
    })
}

fn extract_option_string_field(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> BichonResult<Option<String>> {
    Ok(document
        .get_first(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
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

pub fn extract_contacts(doc: &TantivyDocument) -> BichonResult<HashSet<String>> {
    let fields = SchemaTools::email_fields();
    let mut all_contacts = HashSet::new();

    if let Ok(from_val) = extract_string_field(doc, fields.f_from, F_FROM) {
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

pub fn extract_senders(doc: &TantivyDocument) -> BichonResult<HashSet<String>> {
    let fields = SchemaTools::attachment_fields();
    let mut senders = HashSet::new();

    if let Ok(from_val) = extract_string_field(doc, fields.f_from, F_FROM) {
        if !from_val.is_empty() {
            senders.insert(from_val);
        }
    }

    Ok(senders)
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AttachmentModel {
    pub id: String,
    pub envelope_id: String,
    pub account_id: u64,
    pub account_email: Option<String>,
    pub mailbox_id: u64,
    pub mailbox_name: Option<String>,
    pub subject: String,
    pub content_hash: String,
    pub from: String,
    /// Display name paired with `from`; mirrors the parent envelope.
    pub from_name: Option<String>,
    /// Original `From` address when a header rule replaced `from`.
    pub wire_from: Option<String>,
    pub date: i64,
    pub ingest_at: i64,
    pub size: u64,
    pub ext: Option<String>,
    pub category: String,
    pub content_type: String,
    pub shard_id: u64,
    pub text: Option<String>,
    pub has_text: bool,
    pub is_ocr: bool,
    pub page_count: Option<u64>,
    pub is_indexed: bool,
    pub is_message: bool,
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub auto_tags: Option<Vec<String>>,
}

impl AttachmentModel {
    pub fn into_document(self) -> TantivyDocument {
        let f = SchemaTools::attachment_fields();
        let mut doc = TantivyDocument::new();

        doc.add_text(f.f_id, self.id);
        doc.add_text(f.f_envelope_id, self.envelope_id);
        doc.add_u64(f.f_account_id, self.account_id);
        doc.add_u64(f.f_mailbox_id, self.mailbox_id);
        doc.add_text(f.f_subject, &self.subject);
        doc.add_text(f.f_subject_tri, &self.subject);
        doc.add_text(f.f_content_hash, self.content_hash);
        doc.add_text(f.f_from, &self.from);
        doc.add_text(f.f_from_text, &self.from);
        doc.add_text(f.f_from_text_tri, &self.from);
        if let Some(name) = &self.from_name {
            doc.add_text(f.f_from_name, name);
            doc.add_text(f.f_from_text, name);
            doc.add_text(f.f_from_text_tri, name);
        }
        if let Some(wire_from) = &self.wire_from {
            doc.add_text(f.f_wire_from, wire_from);
            doc.add_text(f.f_from_text, wire_from);
            doc.add_text(f.f_from_text_tri, wire_from);
        }
        doc.add_i64(f.f_date, self.date);
        doc.add_i64(f.f_ingest_at, self.ingest_at);
        doc.add_u64(f.f_size, self.size);
        if let Some(ext) = self.ext {
            doc.add_text(f.f_ext, ext);
        }
        doc.add_text(f.f_category, self.category);
        doc.add_text(f.f_content_type, self.content_type);
        doc.add_u64(f.f_shard_id, self.shard_id);

        if let Some(text) = self.text {
            doc.add_text(f.f_text, text);
        }

        doc.add_bool(f.f_has_text, self.has_text);
        doc.add_bool(f.f_is_ocr, self.is_ocr);

        if let Some(page_count) = self.page_count {
            doc.add_u64(f.f_page_count, page_count);
        }

        doc.add_bool(f.f_is_indexed, self.is_indexed);
        doc.add_bool(f.f_is_message, self.is_message);

        if let Some(name) = self.name {
            doc.add_text(f.f_name_text, name.clone());
            doc.add_text(f.f_name_text_tri, name.clone());
            doc.add_text(f.f_name_exact, name);
        }

        if let Some(tags) = &self.tags {
            for tag in tags {
                doc.add_facet(f.f_tags, tag);
            }
        }

        if let Some(tags) = &self.auto_tags {
            for tag in tags {
                doc.add_facet(f.f_auto_tags, tag);
            }
        }

        doc
    }

    pub fn from_tantivy_doc(doc: &TantivyDocument) -> BichonResult<Self> {
        let f = SchemaTools::attachment_fields();

        let tags: Vec<String> = doc
            .get_all(f.f_tags)
            .filter_map(|value| value.as_facet())
            .map(|facet_encoded_str| {
                Facet::from_encoded(facet_encoded_str.as_bytes().to_vec())
                    .ok()
                    .map(|facet| facet.to_string())
            })
            .flatten()
            .collect();

        let auto_tags: Vec<String> = doc
            .get_all(f.f_auto_tags)
            .filter_map(|value| value.as_facet())
            .map(|facet_encoded_str| {
                Facet::from_encoded(facet_encoded_str.as_bytes().to_vec())
                    .ok()
                    .map(|facet| facet.to_string())
            })
            .flatten()
            .collect();

        let account_id = extract_u64_field(doc, f.f_account_id, F_ACCOUNT_ID)?;
        let mailbox_id = extract_u64_field(doc, f.f_mailbox_id, F_MAILBOX_ID)?;
        let account = AccountModel::get(account_id)?;
        let mailbox = MailBox::get(mailbox_id)?;
        Ok(Self {
            id: extract_string_field(doc, f.f_id, F_ID)?,
            envelope_id: extract_string_field(doc, f.f_envelope_id, F_ENVELOPE_ID)?,
            account_id,
            account_email: Some(account.email),
            mailbox_id,
            mailbox_name: Some(mailbox.name),
            subject: extract_string_field(doc, f.f_subject, F_SUBJECT)?,
            content_hash: extract_string_field(doc, f.f_content_hash, F_CONTENT_HASH)?,
            from: extract_string_field(doc, f.f_from, F_FROM)?,
            from_name: extract_option_string_field(doc, f.f_from_name)?,
            wire_from: extract_option_string_field(doc, f.f_wire_from)?,
            date: extract_i64_field(doc, f.f_date, F_DATE)?,
            ingest_at: extract_i64_field(doc, f.f_ingest_at, F_INGEST_AT)?,
            size: extract_u64_field(doc, f.f_size, F_SIZE)?,
            ext: extract_option_string_field(doc, f.f_ext)?,
            category: extract_string_field(doc, f.f_category, F_ATTACHMENT_CATEGORY)?,
            content_type: extract_string_field(doc, f.f_content_type, F_ATTACHMENT_CONTENT_TYPE)?,
            shard_id: extract_u64_field(doc, f.f_shard_id, F_SHARD_ID)?,
            text: extract_string_field(doc, f.f_text, F_TEXT).ok(),
            has_text: extract_bool_field(doc, f.f_has_text, F_HAS_TEXT)?,
            is_ocr: extract_bool_field(doc, f.f_is_ocr, F_IS_OCR)?,
            page_count: extract_option_u64_field(doc, f.f_page_count)?,
            is_indexed: extract_bool_field(doc, f.f_is_indexed, F_IS_INDEXED)?,
            is_message: extract_bool_field(doc, f.f_is_message, F_IS_MESSAGE)?,
            name: extract_option_string_field(doc, f.f_name_exact)?,
            tags: if tags.is_empty() { None } else { Some(tags) },
            auto_tags: if auto_tags.is_empty() {
                None
            } else {
                Some(auto_tags)
            },
        })
    }
}

#[cfg(test)]
mod sender_field_tests {
    use super::*;

    /// The display name and the pre-override address must be stored so the UI
    /// can render them, and copied into `from_text` so sender search matches
    /// them without a second query path.
    #[test]
    fn sender_name_and_wire_are_stored_and_searchable() {
        let envelope = EnvelopeWithAttachments {
            envelope: Envelope {
                imap_uid: 1,
                from: "jane@example.com".into(),
                from_name: Some("Jane Doe".into()),
                wire_from: Some("alias@addy.io".into()),
                ..Default::default()
            },
            attachments: None,
        };
        let doc = envelope.to_document("body", 0).unwrap();
        let f = SchemaTools::email_fields();

        let name: Vec<&str> = doc
            .get_all(f.f_from_name)
            .filter_map(|v| v.as_str())
            .collect();
        let wire: Vec<&str> = doc
            .get_all(f.f_wire_from)
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(name, vec!["Jane Doe"]);
        assert_eq!(wire, vec!["alias@addy.io"]);

        let text: Vec<&str> = doc
            .get_all(f.f_from_text)
            .filter_map(|v| v.as_str())
            .collect();
        for expected in ["jane@example.com", "Jane Doe", "alias@addy.io"] {
            assert!(text.contains(&expected), "from_text is missing {expected:?}");
        }
    }
}
