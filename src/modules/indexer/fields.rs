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

pub const F_MESSAGE_ID: &str = "message_id";
pub const F_ACCOUNT_ID: &str = "account_id";
pub const F_MAILBOX_ID: &str = "mailbox_id";
pub const F_UID: &str = "uid";
pub const F_SUBJECT: &str = "subject";
pub const F_TEXT: &str = "text";
pub const F_FROM: &str = "from";
pub const F_TO: &str = "to";
pub const F_CC: &str = "cc";
pub const F_BCC: &str = "bcc";
pub const F_DATE: &str = "date";
pub const F_INTERNAL_DATE: &str = "internal_date";
pub const F_SIZE: &str = "size";
pub const F_THREAD_ID: &str = "thread_id";
pub const F_ATTACHMENTS: &str = "attachments";
pub const F_HAS_ATTACHMENT: &str = "has_attachment";
pub const F_TAGS: &str = "tags";

pub const F_MBOX_ID: &str = "mbox_id";
pub const F_MBOX_OFFSET: &str = "mbox_offset";
pub const F_MBOX_LEN: &str = "mbox_len";

pub const F_ID: &str = "id";
pub struct EnvelopeFields {
    pub f_id: Field,
    pub f_message_id: Field,
    pub f_account_id: Field,
    pub f_mailbox_id: Field,
    pub f_uid: Field,
    pub f_subject: Field,
    pub f_text: Field,
    pub f_from: Field,
    pub f_to: Field,
    pub f_cc: Field,
    pub f_bcc: Field,
    pub f_date: Field,
    pub f_internal_date: Field,
    pub f_size: Field,
    pub f_thread_id: Field,
    pub f_attachments: Field,
    pub f_has_attachment: Field,
    pub f_tags: Field,
}

pub struct EmlFields {
    pub f_id: Field,
    pub f_account_id: Field,
    pub f_mailbox_id: Field,
    pub f_mbox_id: Field,
    pub f_mbox_offset: Field,
    pub f_mbox_len: Field,
}
