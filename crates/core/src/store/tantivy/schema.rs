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

use std::sync::{Arc, LazyLock};
use tantivy::schema::{
    FacetOptions, Field, IndexRecordOption, TextFieldIndexing, TextOptions, INDEXED,
};
use tantivy::schema::{Schema, FAST, STORED, STRING};

use crate::store::tantivy::fields::{
    AttachmentFields, EmailFields, F_ACCOUNT_ID, F_ATTACHMENTS, F_ATTACHMENT_CATEGORY,
    F_ATTACHMENT_CONTENT_HASH, F_ATTACHMENT_CONTENT_TYPE, F_ATTACHMENT_COUNT, F_ATTACHMENT_EXT,
    F_ATTACHMENT_NAME_EXACT, F_ATTACHMENT_NAME_TEXT, F_ATTACHMENT_NAME_TEXT_TRI, F_AUTO_TAGS,
    F_BCC, F_BCC_TEXT, F_BCC_TEXT_TRI, F_BODY, F_CC, F_CC_TEXT, F_CC_TEXT_TRI, F_CLEANUP_MOVED,
    F_CONTENT_HASH,
    F_DATE, F_ENVELOPE_ID, F_FROM, F_FROM_NAME, F_FROM_TEXT, F_FROM_TEXT_TRI, F_HAS_TEXT, F_ID,
    F_IMAP_UID, F_INGEST_AT, F_INTERNAL_DATE, F_IS_INDEXED, F_IS_MESSAGE, F_IS_OCR, F_MAILBOX_ID,
    F_MESSAGE_ID, F_WIRE_FROM,
    F_NAME_EXACT,
    F_NAME_TEXT, F_NAME_TEXT_TRI, F_PAGE_COUNT, F_PREVIEW, F_REGULAR_ATTACHMENT_COUNT, F_SHARD_ID,
    F_SIZE, F_SUBJECT, F_SUBJECT_TRI, F_TAGS, F_TEXT, F_THREAD_ID, F_TO, F_TO_TEXT, F_TO_TEXT_TRI,
    F_UID,
};

// ─── Lazy Globals ─────────────────────────────────────────────────────────────

static EMAIL_FIELDS: LazyLock<Arc<EmailFields>> = LazyLock::new(|| Arc::new(EmailSchema::fields()));

static ATTACHMENT_FIELDS: LazyLock<Arc<AttachmentFields>> =
    LazyLock::new(|| Arc::new(AttachmentSchema::fields()));

// ─── Public API ───────────────────────────────────────────────────────────────

pub struct SchemaTools;

impl SchemaTools {
    pub fn email_schema() -> Schema {
        EmailSchema::build().0
    }
    pub fn attachment_schema() -> Schema {
        AttachmentSchema::build().0
    }

    pub fn email_fields() -> &'static EmailFields {
        &EMAIL_FIELDS
    }
    pub fn attachment_fields() -> &'static AttachmentFields {
        &ATTACHMENT_FIELDS
    }

    pub fn email_default_fields() -> Vec<Field> {
        let f = Self::email_fields();
        vec![
            f.f_subject,
            f.f_body,
            f.f_attachment_name_text,
            f.f_from_text,
            f.f_to_text,
            f.f_cc_text,
            f.f_bcc_text,
        ]
    }

    pub fn attachment_default_fields() -> Vec<Field> {
        let f = Self::attachment_fields();
        vec![f.f_subject, f.f_text, f.f_name_text, f.f_from_text]
    }

    pub fn create_email_schema() -> (Schema, EmailFields) {
        EmailSchema::build()
    }
    pub fn create_attachment_schema() -> (Schema, AttachmentFields) {
        AttachmentSchema::build()
    }
}

// ─── Schema builders ──────────────────────────────────────────────────────────

struct EmailSchema;

impl EmailSchema {
    fn build() -> (Schema, EmailFields) {
        let mut b = Schema::builder();

        let f_id = b.add_text_field(F_ID, STRING | STORED | FAST);
        let f_message_id = b.add_text_field(F_MESSAGE_ID, STRING | STORED);
        let f_account_id = b.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_uid = b.add_u64_field(F_UID, INDEXED | STORED | FAST);
        let f_imap_uid = b.add_u64_field(F_IMAP_UID, INDEXED | STORED | FAST);
        let f_subject = b.add_text_field(F_SUBJECT, text_store("euro"));
        let f_body = b.add_text_field(F_BODY, text_no_store("euro"));
        let f_preview = b.add_text_field(F_PREVIEW, STORED);
        let f_content_hash = b.add_text_field(F_CONTENT_HASH, STRING | STORED | FAST);
        let f_from = b.add_text_field(F_FROM, STRING | STORED | FAST);
        let f_to = b.add_text_field(F_TO, STRING | STORED);
        let f_cc = b.add_text_field(F_CC, STRING | STORED);
        let f_bcc = b.add_text_field(F_BCC, STRING | STORED);
        let f_from_text = b.add_text_field(F_FROM_TEXT, text_no_store("euro"));
        let f_to_text = b.add_text_field(F_TO_TEXT, text_no_store("euro"));
        let f_cc_text = b.add_text_field(F_CC_TEXT, text_no_store("euro"));
        let f_bcc_text = b.add_text_field(F_BCC_TEXT, text_no_store("euro"));
        let f_subject_tri = b.add_text_field(F_SUBJECT_TRI, text_no_store("tri"));
        let f_from_text_tri = b.add_text_field(F_FROM_TEXT_TRI, text_no_store("tri"));
        let f_to_text_tri = b.add_text_field(F_TO_TEXT_TRI, text_no_store("tri"));
        let f_cc_text_tri = b.add_text_field(F_CC_TEXT_TRI, text_no_store("tri"));
        let f_bcc_text_tri = b.add_text_field(F_BCC_TEXT_TRI, text_no_store("tri"));
        let f_date = b.add_i64_field(F_DATE, INDEXED | STORED | FAST);
        let f_internal_date = b.add_i64_field(F_INTERNAL_DATE, INDEXED | STORED | FAST);
        let f_ingest_at = b.add_i64_field(F_INGEST_AT, INDEXED | STORED | FAST);
        let f_size = b.add_u64_field(F_SIZE, INDEXED | STORED | FAST);
        let f_thread_id = b.add_text_field(F_THREAD_ID, STRING | STORED | FAST);
        let f_attachment_count = b.add_u64_field(F_ATTACHMENT_COUNT, INDEXED | STORED | FAST);
        let f_regular_attachment_count =
            b.add_u64_field(F_REGULAR_ATTACHMENT_COUNT, INDEXED | STORED | FAST);
        let f_attachment_name_text =
            b.add_text_field(F_ATTACHMENT_NAME_TEXT, text_no_store("euro"));
        let f_attachment_name_text_tri =
            b.add_text_field(F_ATTACHMENT_NAME_TEXT_TRI, text_no_store("tri"));
        let f_attachment_name_exact = b.add_text_field(F_ATTACHMENT_NAME_EXACT, STRING);
        let f_attachments = b.add_text_field(F_ATTACHMENTS, STORED);
        let f_attachment_content_hash =
            b.add_text_field(F_ATTACHMENT_CONTENT_HASH, STRING | STORED | FAST);
        let f_attachment_ext = b.add_text_field(F_ATTACHMENT_EXT, STRING | STORED | FAST);
        let f_attachment_category = b.add_text_field(F_ATTACHMENT_CATEGORY, STRING | STORED | FAST);
        let f_attachment_content_type =
            b.add_text_field(F_ATTACHMENT_CONTENT_TYPE, STRING | STORED | FAST);
        let f_tags = b.add_facet_field(F_TAGS, FacetOptions::default().set_stored());
        let f_shard_id = b.add_u64_field(F_SHARD_ID, INDEXED | STORED | FAST);
        // Appended, never inserted earlier: a `Field` is the position of the
        // field in the schema, so adding one in the middle shifts every field
        // after it and an index built with the previous layout silently reads
        // the wrong columns. STORED is what lets `rebuild_envelope_doc` and
        // `bichon-admin reindex` carry the flag forward.
        let f_cleanup_moved = b.add_bool_field(F_CLEANUP_MOVED, INDEXED | STORED | FAST);
        // New fields are appended after everything that already existed: the
        // field id is its position, so inserting earlier would shift them.
        // STORED only — `from_text` carries the searchable copy.
        let f_from_name = b.add_text_field(F_FROM_NAME, STORED);
        let f_wire_from = b.add_text_field(F_WIRE_FROM, STORED);

        let fields = EmailFields {
            f_id,
            f_message_id,
            f_account_id,
            f_mailbox_id,
            f_uid,
            f_imap_uid,
            f_subject,
            f_body,
            f_preview,
            f_content_hash,
            f_from,
            f_to,
            f_cc,
            f_bcc,
            f_from_text,
            f_to_text,
            f_cc_text,
            f_bcc_text,
            f_subject_tri,
            f_from_text_tri,
            f_to_text_tri,
            f_cc_text_tri,
            f_bcc_text_tri,
            f_date,
            f_internal_date,
            f_ingest_at,
            f_size,
            f_thread_id,
            f_attachment_count,
            f_regular_attachment_count,
            f_attachments,
            f_attachment_name_text,
            f_attachment_name_text_tri,
            f_attachment_name_exact,
            f_attachment_content_hash,
            f_attachment_ext,
            f_attachment_category,
            f_attachment_content_type,
            f_tags,
            f_shard_id,
            f_cleanup_moved,
            f_from_name,
            f_wire_from,
        };

        (b.build(), fields)
    }

    fn fields() -> EmailFields {
        Self::build().1
    }
}

struct AttachmentSchema;

impl AttachmentSchema {
    fn build() -> (Schema, AttachmentFields) {
        let mut b = Schema::builder();

        let f_id = b.add_text_field(F_ID, STRING | STORED | FAST);
        let f_envelope_id = b.add_text_field(F_ENVELOPE_ID, STRING | STORED | FAST);
        let f_account_id = b.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_subject = b.add_text_field(F_SUBJECT, text_store("euro"));
        let f_subject_tri = b.add_text_field(F_SUBJECT_TRI, text_no_store("tri"));
        let f_content_hash = b.add_text_field(F_CONTENT_HASH, STRING | STORED | FAST);
        let f_from = b.add_text_field(F_FROM, STRING | STORED | FAST);
        let f_from_text = b.add_text_field(F_FROM_TEXT, text_no_store("euro"));
        let f_from_text_tri = b.add_text_field(F_FROM_TEXT_TRI, text_no_store("tri"));
        let f_date = b.add_i64_field(F_DATE, INDEXED | STORED | FAST);
        let f_ingest_at = b.add_i64_field(F_INGEST_AT, INDEXED | STORED | FAST);
        let f_size = b.add_u64_field(F_SIZE, INDEXED | STORED | FAST);
        let f_ext = b.add_text_field(F_ATTACHMENT_EXT, STRING | STORED | FAST);
        let f_category = b.add_text_field(F_ATTACHMENT_CATEGORY, STRING | STORED | FAST);
        let f_content_type = b.add_text_field(F_ATTACHMENT_CONTENT_TYPE, STRING | STORED | FAST);
        let f_shard_id = b.add_u64_field(F_SHARD_ID, INDEXED | STORED | FAST);
        let f_text = b.add_text_field(F_TEXT, text_no_store("euro"));
        let f_has_text = b.add_bool_field(F_HAS_TEXT, INDEXED | STORED | FAST);
        let f_is_ocr = b.add_bool_field(F_IS_OCR, INDEXED | STORED | FAST);
        let f_page_count = b.add_u64_field(F_PAGE_COUNT, INDEXED | STORED | FAST);
        let f_is_indexed = b.add_bool_field(F_IS_INDEXED, INDEXED | STORED | FAST);
        let f_is_message = b.add_bool_field(F_IS_MESSAGE, INDEXED | STORED | FAST);
        let f_name_text = b.add_text_field(F_NAME_TEXT, text_no_store("euro"));
        let f_name_text_tri = b.add_text_field(F_NAME_TEXT_TRI, text_no_store("tri"));
        let f_name_exact = b.add_text_field(F_NAME_EXACT, STRING | STORED);
        let f_tags = b.add_facet_field(F_TAGS, FacetOptions::default().set_stored());
        let f_auto_tags = b.add_facet_field(F_AUTO_TAGS, FacetOptions::default().set_stored());
        // Appended last, like the email schema: see the comment there.
        let f_from_name = b.add_text_field(F_FROM_NAME, STORED);
        let f_wire_from = b.add_text_field(F_WIRE_FROM, STORED);

        let fields = AttachmentFields {
            f_id,
            f_envelope_id,
            f_account_id,
            f_mailbox_id,
            f_subject,
            f_subject_tri,
            f_content_hash,
            f_from,
            f_from_text,
            f_from_text_tri,
            f_date,
            f_ingest_at,
            f_size,
            f_ext,
            f_category,
            f_content_type,
            f_shard_id,
            f_text,
            f_has_text,
            f_is_ocr,
            f_page_count,
            f_is_indexed,
            f_is_message,
            f_name_text,
            f_name_text_tri,
            f_name_exact,
            f_tags,
            f_auto_tags,
            f_from_name,
            f_wire_from,
        };

        (b.build(), fields)
    }

    fn fields() -> AttachmentFields {
        Self::build().1
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn text_no_store(tokenizer: &str) -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(tokenizer)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    )
}

fn text_store(tokenizer: &str) -> TextOptions {
    text_no_store(tokenizer).set_stored()
}

#[cfg(test)]
mod tests {
    use super::SchemaTools;

    /// A `tantivy::schema::Field` is the *position* of the field in the schema,
    /// and documents written by an older binary are read back through the
    /// current `SchemaTools` handles. Inserting a field anywhere but the end
    /// therefore shifts every later field: the reader asks for `size` and gets
    /// whatever now sits at that position.
    ///
    /// That is not hypothetical — inserting `cleanup_moved` before `size` made
    /// every search fail with "'size' field is not a u64", because the handle
    /// pointed at `thread_id`.
    ///
    /// This pins the positions of the pre-existing email fields. When adding a
    /// field, append it and update the expected count; do **not** renumber.
    #[test]
    fn existing_email_field_positions_are_stable() {
        let f = SchemaTools::email_fields();

        // Positions as of the schema that shipped before cleanup_moved. These
        // must never change: an index on disk maps positions to data.
        let locked: [(&str, tantivy::schema::Field); 41] = [
            ("f_id", f.f_id),
            ("f_message_id", f.f_message_id),
            ("f_account_id", f.f_account_id),
            ("f_mailbox_id", f.f_mailbox_id),
            ("f_uid", f.f_uid),
            ("f_imap_uid", f.f_imap_uid),
            ("f_subject", f.f_subject),
            ("f_body", f.f_body),
            ("f_preview", f.f_preview),
            ("f_content_hash", f.f_content_hash),
            ("f_from", f.f_from),
            ("f_to", f.f_to),
            ("f_cc", f.f_cc),
            ("f_bcc", f.f_bcc),
            ("f_from_text", f.f_from_text),
            ("f_to_text", f.f_to_text),
            ("f_cc_text", f.f_cc_text),
            ("f_bcc_text", f.f_bcc_text),
            ("f_subject_tri", f.f_subject_tri),
            ("f_from_text_tri", f.f_from_text_tri),
            ("f_to_text_tri", f.f_to_text_tri),
            ("f_cc_text_tri", f.f_cc_text_tri),
            ("f_bcc_text_tri", f.f_bcc_text_tri),
            ("f_date", f.f_date),
            ("f_internal_date", f.f_internal_date),
            ("f_ingest_at", f.f_ingest_at),
            ("f_size", f.f_size),
            ("f_thread_id", f.f_thread_id),
            ("f_attachment_count", f.f_attachment_count),
            ("f_regular_attachment_count", f.f_regular_attachment_count),
            ("f_attachment_name_text", f.f_attachment_name_text),
            ("f_attachment_name_text_tri", f.f_attachment_name_text_tri),
            ("f_attachment_name_exact", f.f_attachment_name_exact),
            ("f_attachments", f.f_attachments),
            ("f_attachment_content_hash", f.f_attachment_content_hash),
            ("f_attachment_ext", f.f_attachment_ext),
            ("f_attachment_category", f.f_attachment_category),
            ("f_attachment_content_type", f.f_attachment_content_type),
            ("f_tags", f.f_tags),
            ("f_shard_id", f.f_shard_id),
            ("f_cleanup_moved", f.f_cleanup_moved),
        ];

        for (position, (name, field)) in locked.iter().enumerate() {
            assert_eq!(
                field.field_id(),
                position as u32,
                "'{name}' moved to position {position}; appending is the only safe change"
            );
        }
    }

    /// The cleanup flag must be STORED, or a tag edit or reindex silently drops
    /// every moved marker and previously moved mail becomes a candidate again.
    #[test]
    fn the_cleanup_flag_is_stored() {
        use tantivy::schema::{FieldType, Schema};
        let schema: Schema = SchemaTools::email_schema();
        let entry = schema.get_field_entry(SchemaTools::email_fields().f_cleanup_moved);
        assert!(entry.is_stored(), "cleanup_moved must be STORED");
        assert!(matches!(entry.field_type(), FieldType::Bool(_)));
    }
}
