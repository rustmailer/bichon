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
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use crate::modules::utils::create_hash;
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

impl AttachmentInfo {
    pub fn get_extension(&self) -> String {
        std::path::Path::new(&self.filename)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_lowercase())
            .unwrap_or_default()
    }

    pub fn get_category(&self) -> &'static str {
        let ext = self.get_extension();

        let category = match ext.as_str() {
            "doc" | "docx" | "pdf" | "rtf" | "odt" | "pages" | "pptx" | "ppt" => Some("document"),
            "xls" | "xlsx" | "ods" | "numbers" | "csv" => Some("spreadsheet"),
            "ical" | "ics" | "vcs" | "ifb" | "icalendar" => Some("event"),
            "txt" | "log" | "md" => Some("text"),
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "avif" | "heic" | "heif" | "webp" => {
                Some("image")
            }
            "mp4" | "mkv" | "mov" | "avi" | "webm" => Some("video"),
            "wav" | "mp3" | "aac" | "ogg" | "wma" | "flac" | "aiff" => Some("audio"),
            "psd" | "eps" | "svg" | "cdr" | "ai" => Some("graphics_2d"),
            "stl" | "obj" | "3mf" | "amf" | "f3d" | "sldprt" | "stp" | "step" | "dwg" | "x_t"
            | "x_b" | "sat" | "ipt" => Some("graphics_3d"),
            "c" | "h" | "html" | "css" | "js" | "ts" | "vue" | "tsx" | "svelte" | "py" | "java"
            | "cs" | "go" | "rb" | "php" | "swift" | "rs" | "r" | "jl" | "lua" | "sql" => {
                Some("code")
            }
            "tsv" | "xml" | "json" | "yml" | "yaml" | "toml" | "env" | "ini" => Some("data"),
            "ps1" | "sh" | "bat" | "cmd" | "exe" | "msi" | "dmg" | "pkg" | "deb" | "rpm" => {
                Some("executable")
            }
            "zip" | "gz" | "tgz" | "7z" | "rar" | "tar" | "bz2" | "zst" | "xz" | "iso" | "img" => {
                Some("archive")
            }
            _ => None,
        };

        if let Some(cat) = category {
            return cat;
        }

        let mime = self.file_type.to_lowercase();
        if mime.starts_with("image/") {
            return "image";
        }
        if mime.starts_with("video/") {
            return "video";
        }
        if mime.starts_with("audio/") {
            return "audio";
        }
        if mime.starts_with("text/") {
            return "text";
        }
        if mime.contains("compressed") || mime.contains("zip") || mime.contains("archive") {
            return "archive";
        }
        if mime.contains("pdf") || mime.contains("msword") || mime.contains("officedocument") {
            return "document";
        }

        "other"
    }
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

pub async fn retrieve_email_content(account_id: u64, id: u64) -> BichonResult<FullMessageContent> {
    AccountModel::check_account_exists(account_id).await?;
    let envelope = ENVELOPE_INDEX_MANAGER
        .get_envelope_by_id(account_id, id)
        .await?
        .ok_or_else(|| {
            raise_error!(
                format!(
                    "Email record not found: account_id={} id={}",
                    account_id, id
                ),
                ErrorCode::ResourceNotFound
            )
        })?;
    let eml_id = create_hash(account_id, &envelope.message_id);
    let eml = EML_INDEX_MANAGER
        .get(account_id, eml_id)
        .await?
        .ok_or_else(|| {
            raise_error!(
                format!(
                    "Email record not found: account_id={} id={}",
                    account_id, id
                ),
                ErrorCode::ResourceNotFound
            )
        })?;
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
        //inline attachment will not be displayed in email attachment list
        if inline && attachment.content_id().is_some() {
            continue;
        }

        attachments.push(AttachmentInfo {
            filename,
            size: attachment.contents().len(),
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
