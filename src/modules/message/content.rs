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


use crate::base64_encode;
use crate::modules::account::migration::AccountModel;
use crate::modules::error::code::ErrorCode;
use crate::modules::indexer::manager::EML_INDEX_MANAGER;
use crate::{modules::error::BichonResult, raise_error};
use mail_parser::{MessageParser, MimeHeaders};

use poem_openapi::Object;
use serde::{Deserialize, Serialize};

/// Represents metadata of an attachment in a Gmail message.
///
/// This struct stores information required to identify, download,
/// and render an attachment, including inline images embedded
/// in HTML emails.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AttachmentInfo {
    /// MIME content type of the attachment (e.g., `image/png`, `application/pdf`).
    pub file_type: String,
    /// Whether the attachment is marked as inline (true) or a regular file (false).
    pub inline: bool,
    /// Original filename of the attachment, if provided.
    pub filename: String,
    /// Size of the attachment in bytes.
    pub size: usize,
    pub content_id: Option<String>,
}

/// Represents the content of an email message in both plain text and HTML formats.
///
/// This struct contains optional fields for plain text and HTML versions of
/// the email message body. At least one of them may be present.
///
/// # Fields
///
/// - `plain`: The plain text version of the message, if available.
/// - `html`: The HTML version of the message, if available.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct FullMessageContent {
    /// Optional plain text version of the message.
    pub text: Option<String>,
    /// Optional HTML version of the message.
    pub html: Option<String>,
    // all Attachments include inline attachments
    pub attachments: Option<Vec<AttachmentInfo>>,
}

use crate::modules::mbox::migration::MboxFileModel as MboxFile;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use std::io::SeekFrom;

pub async fn retrieve_email_content(
    account_id: u64,
    id: u64,
) -> BichonResult<FullMessageContent> {
    AccountModel::check_account_exists(account_id).await?;

    let eml = if let Some((mbox_id, offset, len)) =
        EML_INDEX_MANAGER.get_eml_location(account_id, id).await?
    {
        let mbox_file = MboxFile::find_by_id(mbox_id).await?.ok_or_else(|| {
            raise_error!(
                format!("Mbox file with id {} not found", mbox_id),
                ErrorCode::ResourceNotFound
            )
        })?;

        let mut file = File::open(mbox_file.path).await?;
        file.seek(SeekFrom::Start(offset)).await?;
        let mut buffer = vec![0; len as usize];
        file.read_exact(&mut buffer).await?;
        buffer
    } else {
        EML_INDEX_MANAGER
            .get(account_id, id)
            .await?
            .ok_or_else(|| {
                raise_error!(
                    format!(
                        "Email record not found: account_id={} id={}",
                        account_id, id
                    ),
                    ErrorCode::ResourceNotFound
                )
            })?
    };

    let message = MessageParser::default().parse(&eml).ok_or_else(|| {
        raise_error!(
            format!(
                "Failed to parse EML data (id={}) — the message may be corrupted.",
                id
            ),
            ErrorCode::InternalError
        )
    })?;
    let mut html: Option<String> = message.body_html(0).map(|cow| cow.into_owned());
    let text: Option<String> = message.body_text(0).map(|cow| cow.into_owned());
    let mut attachments = Vec::new();
    for attachment in message.attachments() {
        let content_type = attachment.content_type().ok_or_else(|| {
            raise_error!(
                format!("Attachment is missing Content-Type (email id={})", id),
                ErrorCode::InternalError
            )
        })?;
        let filename = attachment
            .attachment_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| format!("email{}_attachment{}", id, attachment.raw_body_offset()));

        let disposition = attachment.content_disposition();

        let file_type = format!(
            "{}/{}",
            content_type.c_type.as_ref(),
            content_type.c_subtype.as_deref().unwrap_or("")
        );

        let inline = disposition.map(|d| d.is_inline()).unwrap_or(false);

        if inline {
            if let Some(html1) = html.as_deref() {
                if let Some(cid) = attachment.content_id() {
                    if html1.contains(cid) {
                        let data = attachment.contents();
                        let base64_encoded = base64_encode!(data);
                        let html_content = html1.replace(
                            &format!("cid:{}", cid),
                            &format!("data:{};base64,{}", file_type, base64_encoded),
                        );
                        html = Some(html_content);
                    }
                }
            }
        }

        attachments.push(AttachmentInfo {
            filename,
            size: attachment.len(),
            inline,
            file_type,
            content_id: attachment.content_id().map(Into::into),
        });
    }
    Ok(FullMessageContent {
        text,
        html,
        attachments: Some(attachments),
    })
}
