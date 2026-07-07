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

use crate::account::migration::AccountModel;
use crate::base64_encode;
use crate::envelope::extractor::{
    extract_envelope_from_nested_message, reattach_eml_content, reattach_eml_content_self_healing,
    recover_message_blob,
};
use crate::error::code::ErrorCode;
use crate::store::envelope::Envelope;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::utils::compute_content_hash;
use crate::utils::html::block_remote_content;
use crate::{error::BichonResult, raise_error};
use bytes::Bytes;
use mail_parser::{MessageParser, MimeHeaders};
//use poem_openapi::Object;
use serde::{Deserialize, Serialize};
/// Represents metadata of an attachment in a Gmail message.
///
/// This struct stores information required to identify, download,
/// and render an attachment, including inline images embedded
/// in HTML emails.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AttachmentInfo {
    /// MIME content type of the attachment (e.g., `image/png`, `application/pdf`).
    pub file_type: String,
    /// Whether the attachment is marked as inline (true) or a regular file (false).
    pub inline: bool,
    /// Original filename of the attachment, if provided.
    pub filename: Option<String>,
    /// Size of the attachment in bytes.
    pub size: usize,
    pub content_id: Option<String>,
    /// Hash of the content.
    pub content_hash: String,
    pub is_message: bool,
    /// Text extracted from the attachment body (Pro/Enterprise feature).
    /// Populated during IMAP sync; None for inline attachments and unsupported file types.
    pub extracted_text: Option<String>,
    /// Page count reported by the extractor, if any.
    pub extracted_page_count: Option<u32>,
    /// Whether the extracted text came from OCR.
    #[serde(default)]
    pub extracted_is_ocr: bool,
}

impl AttachmentInfo {
    pub fn is_inline(&self) -> bool {
        self.inline && self.content_id.is_some()
    }

    pub fn get_extension(&self) -> Option<String> {
        self.filename
            .as_deref()
            .and_then(|f| std::path::Path::new(f).extension())
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
    }

    pub fn get_category(&self) -> &'static str {
        if let Some(ext) = self.get_extension() {
            let category = match ext.as_str() {
                "doc" | "docx" | "pdf" | "rtf" | "odt" | "pages" | "pptx" | "ppt" => {
                    Some("document")
                }
                "xls" | "xlsx" | "ods" | "numbers" | "csv" => Some("spreadsheet"),
                "ical" | "ics" | "vcs" | "ifb" | "icalendar" => Some("event"),
                "txt" | "log" | "md" => Some("text"),
                "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "avif" | "heic" | "heif"
                | "webp" => Some("image"),
                "mp4" | "mkv" | "mov" | "avi" | "webm" => Some("video"),
                "wav" | "mp3" | "aac" | "ogg" | "wma" | "flac" | "aiff" => Some("audio"),
                "psd" | "eps" | "svg" | "cdr" | "ai" => Some("graphics_2d"),
                "stl" | "obj" | "3mf" | "amf" | "f3d" | "sldprt" | "stp" | "step" | "dwg"
                | "x_t" | "x_b" | "sat" | "ipt" => Some("graphics_3d"),
                "c" | "h" | "html" | "css" | "js" | "ts" | "vue" | "tsx" | "svelte" | "py"
                | "java" | "cs" | "go" | "rb" | "php" | "swift" | "rs" | "r" | "jl" | "lua"
                | "sql" => Some("code"),
                "tsv" | "xml" | "json" | "yml" | "yaml" | "toml" | "env" | "ini" => Some("data"),
                "ps1" | "sh" | "bat" | "cmd" | "exe" | "msi" | "dmg" | "pkg" | "deb" | "rpm" => {
                    Some("executable")
                }
                "zip" | "gz" | "tgz" | "7z" | "rar" | "tar" | "bz2" | "zst" | "xz" | "iso"
                | "img" => Some("archive"),
                "eml" | "msg" => Some("message"),
                _ => None,
            };

            if let Some(cat) = category {
                return cat;
            }
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
        if mime == "message/rfc822" {
            return "message";
        }
        if mime.contains("compressed") || mime.contains("zip") || mime.contains("archive") {
            return "archive";
        }
        if mime.contains("pdf") || mime.contains("msword") || mime.contains("officedocument") {
            return "document";
        }
        if mime.contains("spreadsheet") || mime.contains("excel") {
            return "spreadsheet";
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
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct FullMessageContent {
    /// Optional plain text version of the message.
    pub text: Option<String>,
    /// Optional HTML version of the message.
    pub html: Option<String>,
    // all Attachments include inline attachments
    pub attachments: Option<Vec<AttachmentInfo>>,
    /// True when remote content (http/https URLs) was detected and stripped from html.
    #[serde(default)]
    pub has_remote_content: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct FullNestedMessageContent {
    /// Optional plain text version of the message.
    pub text: Option<String>,
    /// Optional HTML version of the message.
    pub html: Option<String>,
    // all Attachments include inline attachments
    pub attachments: Option<Vec<AttachmentInfo>>,
    /// Metadata for the email envelope.
    pub envelope: Envelope,
    /// True when remote content (http/https URLs) was detected and stripped from html.
    #[serde(default)]
    pub has_remote_content: bool,
}

pub fn retrieve_email_content(
    account_id: u64,
    envelope_id: String,
    block_remote: bool,
) -> BichonResult<FullMessageContent> {
    AccountModel::check_account_exists(account_id)?;
    let (envelope, eml) = reattach_eml_content(account_id, envelope_id)?;
    build_full_message_content(&envelope, eml, block_remote)
}

pub async fn retrieve_email_content_self_healing(
    account_id: u64,
    envelope_id: String,
    block_remote: bool,
) -> BichonResult<FullMessageContent> {
    AccountModel::check_account_exists(account_id)?;
    let (envelope, eml) = reattach_eml_content_self_healing(account_id, envelope_id).await?;
    let indexed_attachments = indexed_attachments(account_id, &envelope.id)?;
    let parsed_content = match build_full_message_content(&envelope, eml, block_remote) {
        Ok(content) => content,
        Err(error) if envelope.regular_attachment_count > 0 => {
            tracing::warn!(
                account_id,
                envelope_id = %envelope.id,
                error = %error,
                expected_regular_attachments = envelope.regular_attachment_count,
                "Recovered message content could not be parsed; refetching archived message from IMAP"
            );
            let raw_body = recover_message_blob(&envelope).await?;
            return Ok(with_indexed_attachments(
                build_full_message_content(&envelope, raw_body, block_remote)?,
                indexed_attachments.as_ref(),
            ));
        }
        Err(error) => return Err(error),
    };
    let parsed_count = parsed_content
        .attachments
        .as_ref()
        .map(|attachments| attachments.len())
        .unwrap_or(0);
    if envelope.regular_attachment_count > 0 && parsed_count == 0 {
        tracing::warn!(
            account_id,
            envelope_id = %envelope.id,
            expected_regular_attachments = envelope.regular_attachment_count,
            "Recovered message content had no attachment metadata; refetching archived message from IMAP"
        );
        let raw_body = recover_message_blob(&envelope).await?;
        return Ok(with_indexed_attachments(
            build_full_message_content(&envelope, raw_body, block_remote)?,
            indexed_attachments.as_ref(),
        ));
    }
    Ok(with_indexed_attachments(
        parsed_content,
        indexed_attachments.as_ref(),
    ))
}

fn indexed_attachments(
    account_id: u64,
    envelope_id: &str,
) -> BichonResult<Option<Vec<AttachmentInfo>>> {
    Ok(ENVELOPE_MANAGER
        .get_envelope_by_id(account_id, envelope_id)?
        .and_then(|envelope| envelope.attachments))
}

fn with_indexed_attachments(
    mut content: FullMessageContent,
    indexed_attachments: Option<&Vec<AttachmentInfo>>,
) -> FullMessageContent {
    if let Some(indexed) = indexed_attachments {
        let parsed_count = content
            .attachments
            .as_ref()
            .map(|attachments| attachments.len())
            .unwrap_or(0);
        if parsed_count == 0 && !indexed.is_empty() {
            content.attachments = Some(indexed.clone());
        }
    }
    content
}

fn build_full_message_content(
    envelope: &Envelope,
    eml: Bytes,
    block_remote: bool,
) -> BichonResult<FullMessageContent> {
    let message = MessageParser::default().parse(&eml).ok_or_else(|| {
        raise_error!(
            "Failed to parse EML data — the message may be corrupted.".into(),
            ErrorCode::InternalError
        )
    })?;
    let mut html: Option<String> = message.body_html(0).map(|cow| cow.into_owned());
    let text: Option<String> = message.body_text(0).map(|cow| cow.into_owned());
    let mut attachments = Vec::new();
    for attachment in message.attachments() {
        let content_type = attachment.content_type().ok_or_else(|| {
            raise_error!(
                format!(
                    "Attachment is missing Content-Type (email id={})",
                    envelope.id
                ),
                ErrorCode::InternalError
            )
        })?;
        let filename = attachment.attachment_name().map(|name| name.to_string());
        let disposition = attachment.content_disposition();
        let file_type = format!(
            "{}/{}",
            content_type.c_type.as_ref(),
            content_type.c_subtype.as_deref().unwrap_or("")
        );

        let inline = disposition
            .map(|d| d.is_inline())
            .unwrap_or_else(|| attachment.content_id().is_some());

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
        let is_message = attachment.is_message();
        let content_hash = compute_content_hash(attachment.contents());
        attachments.push(AttachmentInfo {
            filename: filename.or(Some(content_hash.clone())),
            size: attachment.contents().len(),
            inline,
            file_type,
            is_message,
            content_hash,
            content_id: attachment.content_id().map(Into::into),
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        });
    }
    let mut has_remote_content = false;
    if let Some(ref html_body) = html {
        let filtered = block_remote_content(html_body);
        has_remote_content = *html_body != filtered;
        if block_remote {
            html = Some(filtered);
        }
    }
    Ok(FullMessageContent {
        text,
        html,
        attachments: Some(attachments),
        has_remote_content,
    })
}

pub fn retrieve_nested_eml_content(
    account_id: u64,
    envelope_id: String,
    content_hash: &str,
    block_remote: bool,
) -> BichonResult<FullNestedMessageContent> {
    let (_, eml) = reattach_eml_content(account_id, envelope_id)?;
    let parent_message = MessageParser::default().parse(&eml).ok_or_else(|| {
        raise_error!(
            "Failed to parse parent EML".into(),
            ErrorCode::InternalError
        )
    })?;

    let attachment_content = parent_message
        .attachments()
        .find(|att| compute_content_hash(att.contents()) == content_hash)
        .map(|att| att.contents())
        .ok_or_else(|| {
            raise_error!(
                "Target nested EML not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;

    let nested_message = MessageParser::default()
        .parse(attachment_content)
        .ok_or_else(|| {
            raise_error!(
                "Failed to parse nested EML".into(),
                ErrorCode::InternalError
            )
        })?;

    let mut html = nested_message.body_html(0).map(|c| c.into_owned());
    let text = nested_message.body_text(0).map(|c| c.into_owned());

    let mut attachments = Vec::new();

    let has_html = html.is_some();

    for attachment in nested_message.attachments() {
        let cid = attachment.content_id();
        let disposition = attachment.content_disposition();
        let is_inline = disposition
            .map(|d| d.is_inline())
            .unwrap_or_else(|| cid.is_some());

        if has_html && is_inline && cid.is_some() {
            let content_id = cid.unwrap();
            let html_ref = html.as_mut().unwrap();

            let cid_pattern = format!("cid:{}", content_id);
            if html_ref.contains(&cid_pattern) {
                let data = attachment.contents();
                let ct = attachment
                    .content_type()
                    .map(|ct| format!("{}/{}", ct.c_type, ct.c_subtype.as_deref().unwrap_or("")))
                    .unwrap_or_else(|| "image/png".to_string());

                let base64_data = format!("data:{};base64,{}", ct, base64_encode!(data));
                *html_ref = html_ref.replace(&cid_pattern, &base64_data);
                continue;
            }
        }

        let file_type = attachment.content_type().map_or_else(
            || "application/octet-stream".to_string(),
            |ct| format!("{}/{}", ct.c_type, ct.c_subtype.as_deref().unwrap_or("")),
        );
        let content_hash = compute_content_hash(attachment.contents());
        attachments.push(AttachmentInfo {
            filename: attachment
                .attachment_name()
                .map(|n| n.to_string())
                .or(Some(content_hash.clone())),
            size: attachment.contents().len(),
            inline: is_inline,
            file_type,
            content_hash,
            is_message: attachment.is_message(),
            content_id: cid.map(Into::into),
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        });
    }

    let envelope = extract_envelope_from_nested_message(nested_message, account_id)?;

    let mut has_remote_content = false;
    if let Some(ref html_body) = html {
        let filtered = block_remote_content(html_body);
        has_remote_content = *html_body != filtered;
        if block_remote {
            html = Some(filtered);
        }
    }
    Ok(FullNestedMessageContent {
        text,
        html,
        attachments: Some(attachments),
        envelope,
        has_remote_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates JSON written by a version before `extracted_text`, `extracted_page_count`,
    /// and `extracted_is_ocr` were added to [`AttachmentInfo`]. Deserialization must
    /// succeed and fill the missing fields with their defaults.
    #[test]
    fn attachment_info_backward_compat_no_extracted_fields() {
        let old_json = r#"[
            {
                "file_type": "application/pdf",
                "inline": false,
                "filename": "report.pdf",
                "size": 12345,
                "content_id": null,
                "content_hash": "abc123",
                "is_message": false
            },
            {
                "file_type": "image/png",
                "inline": true,
                "filename": "logo.png",
                "size": 6789,
                "content_id": "cid:logo@example.com",
                "content_hash": "def456",
                "is_message": false
            }
        ]"#;

        let attachments: Vec<AttachmentInfo> =
            serde_json::from_str(old_json).expect("should deserialize legacy JSON");

        assert_eq!(attachments.len(), 2);

        // First attachment (regular file)
        assert_eq!(attachments[0].file_type, "application/pdf");
        assert!(!attachments[0].inline);
        assert_eq!(attachments[0].filename.as_deref(), Some("report.pdf"));
        assert_eq!(attachments[0].size, 12345);
        assert_eq!(attachments[0].content_id, None);
        assert_eq!(attachments[0].content_hash, "abc123");
        assert!(!attachments[0].is_message);
        // Fields added after the legacy format — must default correctly
        assert_eq!(attachments[0].extracted_text, None);
        assert_eq!(attachments[0].extracted_page_count, None);
        assert!(!attachments[0].extracted_is_ocr);

        // Second attachment (inline image with content-id)
        assert_eq!(attachments[1].file_type, "image/png");
        assert!(attachments[1].inline);
        assert_eq!(attachments[1].filename.as_deref(), Some("logo.png"));
        assert_eq!(attachments[1].size, 6789);
        assert_eq!(
            attachments[1].content_id.as_deref(),
            Some("cid:logo@example.com")
        );
        assert_eq!(attachments[1].content_hash, "def456");
        assert!(!attachments[1].is_message);
        assert_eq!(attachments[1].extracted_text, None);
        assert_eq!(attachments[1].extracted_page_count, None);
        assert!(!attachments[1].extracted_is_ocr);
    }

    /// Current struct must round-trip through serde_json without data loss.
    #[test]
    fn attachment_info_round_trip() {
        let attachments = vec![
            AttachmentInfo {
                file_type: "text/html".into(),
                inline: false,
                filename: Some("page.html".into()),
                size: 42,
                content_id: None,
                content_hash: "hash1".into(),
                is_message: true,
                extracted_text: Some("hello world".into()),
                extracted_page_count: Some(1),
                extracted_is_ocr: false,
            },
            AttachmentInfo {
                file_type: "application/zip".into(),
                inline: false,
                filename: Some("archive.zip".into()),
                size: 99999,
                content_id: None,
                content_hash: "hash2".into(),
                is_message: false,
                extracted_text: None,
                extracted_page_count: None,
                extracted_is_ocr: true,
            },
        ];

        let json = serde_json::to_string(&attachments).expect("serialize");
        let round_tripped: Vec<AttachmentInfo> = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(attachments, round_tripped);
    }
}
