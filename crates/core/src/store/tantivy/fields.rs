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

use tantivy::schema::Field;

pub const F_ID: &str = "id";
pub const F_MESSAGE_ID: &str = "message_id";
pub const F_ACCOUNT_ID: &str = "account_id";
pub const F_MAILBOX_ID: &str = "mailbox_id";
pub const F_UID: &str = "uid";
/// Stable archive-side IMAP UID, assigned once per envelope at ingest and
/// never reused within a mailbox generation. Distinct from `uid` (the
/// source server's UID).
pub const F_IMAP_UID: &str = "imap_uid";
pub const F_SUBJECT: &str = "subject";
pub const F_BODY: &str = "body";
pub const F_PREVIEW: &str = "preview";
pub const F_CONTENT_HASH: &str = "content_hash";

pub const F_FROM: &str = "from";
/// Display name paired with [`F_FROM`] (decoded, verbatim). Empty = no name.
pub const F_FROM_NAME: &str = "from_name";
/// Original `From` address, kept only when an account's header rule replaced
/// `F_FROM`. The raw message remains the source of truth; this exists for the
/// UI's "via …" hint and for audits.
pub const F_WIRE_FROM: &str = "wire_from";
pub const F_TO: &str = "to";
pub const F_CC: &str = "cc";
pub const F_BCC: &str = "bcc";

pub const F_FROM_TEXT: &str = "from_text";
pub const F_TO_TEXT: &str = "to_text";
pub const F_CC_TEXT: &str = "cc_text";
pub const F_BCC_TEXT: &str = "bcc_text";

// Trigram sibling fields (tokenizer "tri"). Only the small fields get a trigram
// sibling; bodies (f_body / attachment f_text) deliberately do not.
pub const F_SUBJECT_TRI: &str = "subject_tri";
pub const F_FROM_TEXT_TRI: &str = "from_text_tri";
pub const F_TO_TEXT_TRI: &str = "to_text_tri";
pub const F_CC_TEXT_TRI: &str = "cc_text_tri";
pub const F_BCC_TEXT_TRI: &str = "bcc_text_tri";
pub const F_ATTACHMENT_NAME_TEXT_TRI: &str = "attachment_name_text_tri";
pub const F_NAME_TEXT_TRI: &str = "name_text_tri";

pub const F_DATE: &str = "date";
pub const F_INTERNAL_DATE: &str = "internal_date";
pub const F_INGEST_AT: &str = "ingest_at";
pub const F_SIZE: &str = "size";
pub const F_THREAD_ID: &str = "thread_id";
/// Set once source-mailbox cleanup has moved this message into
/// `bichon_auto_created_trash`.
///
/// Lives on the archived document rather than in a side table so the flag can
/// never drift from the message it describes, and so it survives a reindex
/// (the rebuild reads stored fields). Candidate collection filters on it, which
/// is what keeps a moved message from consuming a slot in every later run.
pub const F_CLEANUP_MOVED: &str = "cleanup_moved";
pub const F_ATTACHMENT_COUNT: &str = "attachment_count";
pub const F_REGULAR_ATTACHMENT_COUNT: &str = "regular_attachment_count";
pub const F_ATTACHMENTS: &str = "attachments";
pub const F_ATTACHMENT_NAME_TEXT: &str = "attachment_name_text";
pub const F_ATTACHMENT_NAME_EXACT: &str = "attachment_name_exact";
pub const F_ATTACHMENT_CONTENT_HASH: &str = "attachment_content_hash";
pub const F_ATTACHMENT_EXT: &str = "attachment_ext";
pub const F_ATTACHMENT_CATEGORY: &str = "attachment_category";
pub const F_ATTACHMENT_CONTENT_TYPE: &str = "attachment_content_type";

pub const F_ENVELOPE_ID: &str = "eid";
pub const F_TEXT: &str = "text";
pub const F_HAS_TEXT: &str = "has_text";
pub const F_IS_OCR: &str = "is_ocr";
pub const F_IS_INDEXED: &str = "is_indexed";
pub const F_IS_MESSAGE: &str = "is_message";
pub const F_NAME_TEXT: &str = "name_text";
pub const F_NAME_EXACT: &str = "name_exact";
pub const F_PAGE_COUNT: &str = "page_count";
pub const F_TAGS: &str = "tags";
pub const F_AUTO_TAGS: &str = "auto_tags";
pub const F_SHARD_ID: &str = "shard_id";

pub struct EmailFields {
    pub f_id: Field,
    pub f_message_id: Field,
    pub f_account_id: Field,
    pub f_mailbox_id: Field,
    pub f_uid: Field,
    pub f_imap_uid: Field,
    pub f_subject: Field,
    pub f_body: Field,
    pub f_preview: Field,
    pub f_content_hash: Field,
    pub f_from: Field,
    pub f_to: Field,
    pub f_cc: Field,
    pub f_bcc: Field,
    pub f_from_text: Field,
    pub f_to_text: Field,
    pub f_cc_text: Field,
    pub f_bcc_text: Field,
    pub f_subject_tri: Field,
    pub f_from_text_tri: Field,
    pub f_to_text_tri: Field,
    pub f_cc_text_tri: Field,
    pub f_bcc_text_tri: Field,
    pub f_date: Field,
    pub f_internal_date: Field,
    pub f_ingest_at: Field,
    pub f_cleanup_moved: Field,
    pub f_size: Field,
    pub f_thread_id: Field,
    pub f_attachment_count: Field,
    pub f_regular_attachment_count: Field,
    pub f_attachments: Field,
    pub f_attachment_name_text: Field,
    pub f_attachment_name_text_tri: Field,
    pub f_attachment_name_exact: Field,
    pub f_attachment_content_hash: Field,
    pub f_attachment_ext: Field,
    pub f_attachment_category: Field,
    pub f_attachment_content_type: Field,
    pub f_tags: Field,
    pub f_shard_id: Field,
    /// Appended last: a `Field` is a position, so new fields may never be
    /// inserted before existing ones.
    pub f_from_name: Field,
    pub f_wire_from: Field,
}

pub struct AttachmentFields {
    pub f_id: Field,
    pub f_envelope_id: Field, // envelope id
    pub f_account_id: Field,
    pub f_mailbox_id: Field,
    pub f_from: Field,
    pub f_from_text: Field,
    pub f_from_text_tri: Field,
    pub f_subject: Field,
    pub f_subject_tri: Field,
    pub f_content_hash: Field,
    pub f_text: Field,
    pub f_has_text: Field,
    pub f_is_ocr: Field,
    pub f_page_count: Field,
    pub f_is_indexed: Field,
    pub f_ingest_at: Field,
    pub f_date: Field,
    pub f_size: Field,
    pub f_is_message: Field,
    pub f_name_text: Field,  // TEXT
    pub f_name_text_tri: Field,
    pub f_name_exact: Field, // STRING
    pub f_ext: Field,
    pub f_category: Field,
    pub f_content_type: Field,
    pub f_shard_id: Field,
    pub f_tags: Field,
    pub f_auto_tags: Field,
    /// Appended last: a `Field` is a position, so new fields may never be
    /// inserted before existing ones.
    pub f_from_name: Field,
    pub f_wire_from: Field,
}
