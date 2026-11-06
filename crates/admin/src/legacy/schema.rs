use tantivy::schema::{FacetOptions, Field, Schema, FAST, INDEXED, STORED, STRING, TEXT};

use crate::legacy::fields::{EmlFields, EnvelopeFields, *};

pub struct SchemaTools;

impl SchemaTools {
    pub fn envelope_schema() -> Schema {
        EnvelopeSchema::build().0
    }
    pub fn eml_schema() -> Schema {
        EmlSchema::build().0
    }

    pub fn envelope_fields() -> EnvelopeFields {
        EnvelopeSchema::fields()
    }
    pub fn eml_fields() -> EmlFields {
        EmlSchema::fields()
    }

    pub fn envelope_default_fields() -> Vec<Field> {
        let f = Self::envelope_fields();
        vec![f.f_subject, f.f_text, f.f_attachments]
    }
}

// ─── Schema builders ──────────────────────────────────────────────────────────

struct EnvelopeSchema;

impl EnvelopeSchema {
    fn build() -> (Schema, EnvelopeFields) {
        let mut b = Schema::builder();

        // The builder assigns field ids in call order, and the migration reads
        // a v0.3.7 index through these `Field` handles. The order below must
        // therefore match `git show 0.3.7:src/modules/indexer/schema.rs`
        // exactly; reordering any call makes the migration read another
        // field's column (e.g. `internal_date` would resolve to `message_id`).
        let f_id = b.add_u64_field(F_ID, INDEXED | STORED | FAST);
        let f_account_id = b.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_uid = b.add_u64_field(F_UID, INDEXED | STORED | FAST);

        let f_subject = b.add_text_field(F_SUBJECT, TEXT | STORED);
        let f_text = b.add_text_field(F_TEXT, TEXT | STORED);

        let f_from = b.add_text_field(F_FROM, STRING | STORED | FAST);
        let f_to = b.add_text_field(F_TO, STRING | STORED);
        let f_cc = b.add_text_field(F_CC, STRING | STORED);
        let f_bcc = b.add_text_field(F_BCC, STRING | STORED);

        let f_date = b.add_i64_field(F_DATE, STORED | FAST);
        let f_internal_date = b.add_i64_field(F_INTERNAL_DATE, STORED | FAST);

        let f_size = b.add_u64_field(F_SIZE, STORED | FAST);
        let f_thread_id = b.add_u64_field(F_THREAD_ID, INDEXED | STORED | FAST);
        let f_message_id = b.add_text_field(F_MESSAGE_ID, STRING | STORED);
        let f_attachments = b.add_text_field(F_ATTACHMENTS, TEXT | STORED);
        let f_has_attachment = b.add_bool_field(F_HAS_ATTACHMENT, INDEXED | STORED | FAST);

        let f_tags = b.add_facet_field(F_TAGS, FacetOptions::default().set_stored());

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

        (b.build(), fields)
    }

    fn fields() -> EnvelopeFields {
        Self::build().1
    }
}

struct EmlSchema;

impl EmlSchema {
    fn build() -> (Schema, EmlFields) {
        let mut b = Schema::builder();

        let f_id = b.add_u64_field(F_ID, INDEXED | FAST);
        let f_account_id = b.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_eml = b.add_bytes_field(F_EML, STORED);

        let fields = EmlFields {
            f_id,
            f_account_id,
            f_mailbox_id,
            f_eml,
        };

        (b.build(), fields)
    }

    fn fields() -> EmlFields {
        Self::build().1
    }
}
