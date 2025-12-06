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

use crate::modules::indexer::fields::{EnvelopeFields, *};
use tantivy::schema::{FacetOptions, Field, INDEXED};
use tantivy::schema::{Schema, FAST, STORED, STRING, TEXT};

static ENVELOPE_FIELDS: LazyLock<Arc<EnvelopeFields>> = LazyLock::new(|| {
    let (_, fields) = SchemaTools::create_envelope_schema();
    Arc::new(fields)
});

static EML_FIELDS: LazyLock<Arc<EmlFields>> = LazyLock::new(|| {
    let (_, fields) = SchemaTools::create_eml_schema();
    Arc::new(fields)
});

pub struct SchemaTools;

impl SchemaTools {
    pub fn envelope_schema() -> Schema {
        let (schema, _) = Self::create_envelope_schema();
        schema
    }

    pub fn eml_schema() -> Schema {
        let (schema, _) = Self::create_eml_schema();
        schema
    }

    pub fn envelope_fields() -> &'static EnvelopeFields {
        &ENVELOPE_FIELDS
    }

    pub fn eml_fields() -> &'static EmlFields {
        &EML_FIELDS
    }

    pub fn envelope_default_fields() -> Vec<Field> {
        let fields = Self::envelope_fields();
        vec![fields.f_subject, fields.f_text, fields.f_attachments]
    }

    pub fn create_envelope_schema() -> (Schema, EnvelopeFields) {
        let mut builder = Schema::builder();
        let f_id = builder.add_u64_field(F_ID, INDEXED | STORED | FAST);
        // Account/ Mailbox IDs: numeric, for filtering/aggregation
        let f_account_id = builder.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = builder.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        // UID: numeric, locate message
        let f_uid = builder.add_u64_field(F_UID, INDEXED | STORED | FAST);
        // Subject/body: tokenized for full-text search
        let f_subject = builder.add_text_field(F_SUBJECT, TEXT | STORED);
        let f_text = builder.add_text_field(F_TEXT, TEXT | STORED);
        // Email addresses: exact match search
        let f_from = builder.add_text_field(F_FROM, STRING | STORED | FAST);
        let f_to = builder.add_text_field(F_TO, STRING | STORED);
        let f_cc = builder.add_text_field(F_CC, STRING | STORED);
        let f_bcc = builder.add_text_field(F_BCC, STRING | STORED);
        // Date fields: numeric, range filtering
        let f_date = builder.add_i64_field(F_DATE, STORED | FAST);
        let f_internal_date = builder.add_i64_field(F_INTERNAL_DATE, STORED | FAST);
        // Size: numeric, range filtering
        let f_size = builder.add_u64_field(F_SIZE, STORED | FAST);
        // Thread ID: numeric, filter by thread
        let f_thread_id = builder.add_u64_field(F_THREAD_ID, INDEXED | STORED | FAST);
        // Message-ID: unique identifier, no tokenization
        let f_message_id = builder.add_text_field(F_MESSAGE_ID, STRING | STORED);
        // Attachments: exact match search
        let f_attachments = builder.add_text_field(F_ATTACHMENTS, TEXT | STORED);
        let f_has_attachment = builder.add_bool_field(F_HAS_ATTACHMENT, INDEXED | STORED | FAST);
        let f_tags = builder.add_facet_field(F_TAGS, FacetOptions::default().set_stored());
        let fields = EnvelopeFields {
            f_id,
            f_account_id,
            f_mailbox_id,
            f_uid,
            f_subject,
            f_text,
            f_from,
            f_to,
            f_cc,
            f_bcc,
            f_date,
            f_internal_date,
            f_size,
            f_thread_id,
            f_message_id,
            f_attachments,
            f_has_attachment,
            f_tags,
        };
        (builder.build(), fields)
    }

    pub fn create_eml_schema() -> (Schema, EmlFields) {
        let mut builder = Schema::builder();
        let f_id = builder.add_u64_field(F_ID, INDEXED | FAST);
        let f_account_id = builder.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = builder.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_mbox_id = builder.add_u64_field(F_MBOX_ID, STORED | FAST);
        let f_mbox_offset = builder.add_u64_field(F_MBOX_OFFSET, STORED | FAST);
        let f_mbox_len = builder.add_u64_field(F_MBOX_LEN, STORED | FAST);
        let fields = EmlFields {
            f_id,
            f_account_id,
            f_mailbox_id,
            f_mbox_id,
            f_mbox_offset,
            f_mbox_len,
        };
        (builder.build(), fields)
    }
}
