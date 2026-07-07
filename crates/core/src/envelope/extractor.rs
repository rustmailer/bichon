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
use crate::cache::imap::mailbox::MailBox;
use crate::common::AddrVec;
use crate::envelope::meta::parse_bichon_metadata;
use crate::envelope::utils::normalize_subject;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::imap::executor::ImapExecutor;
use crate::message::content::AttachmentInfo;
use crate::store::blob::{BlobManager, DetachedEmail, BLOB_MANAGER};
use crate::store::tantivy::attachment::ATTACHMENT_MANAGER;
use crate::store::tantivy::dedup_cache::DEDUP_CACHE;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::store::tantivy::model::{AttachmentModel, EnvelopeWithAttachments};
use crate::utils::html::extract_text;
use crate::utils::{compute_content_hash, hex_hash};
use crate::{id, store::envelope::Envelope};
use crate::{raise_error, utc_now};
use async_imap::types::Fetch;
use bytes::Bytes;
use mail_parser::{Address, HeaderName, Message, MessageParser, MimeHeaders};
use tantivy::schema::Facet;
use tantivy::TantivyDocument;
use tracing::error;
use uuid::Uuid;

pub async fn extract_envelope_and_store_it(
    fetch: Fetch,
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<()> {
    let internal_date = fetch
        .internal_date()
        .map(|d| d.timestamp_millis())
        .unwrap_or(0);
    let uid = fetch.uid.unwrap_or(0);
    let body = match fetch.body() {
        Some(b) => b,
        None => {
            tracing::warn!(
                account_id,
                uid = fetch.uid,
                "FETCH response has no body, skipping message"
            );
            return Ok(());
        }
    };
    let size = fetch.size.unwrap_or(body.len() as u32);
    extract_envelope_core(body, uid, size, internal_date, account_id, mailbox_id).await
}

pub async fn extract_envelope_from_eml(
    body: &[u8],
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<()> {
    extract_envelope_core(body, 0, body.len() as u32, 0, account_id, mailbox_id).await
}

pub async fn extract_envelope_from_smtp(
    body: &[u8],
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<()> {
    extract_envelope_core(
        body,
        0,
        body.len() as u32,
        utc_now!(),
        account_id,
        mailbox_id,
    )
    .await
}

async fn extract_envelope_core(
    body: &[u8],
    uid: u32,
    size: u32,
    internal_date: i64,
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<()> {
    //The content hash of the original raw EML
    let email_content_hash = compute_content_hash(body);
    if DEDUP_CACHE.contains(account_id, mailbox_id, &email_content_hash) {
        tracing::debug!("Duplicate email detected");
        //println!("Duplicate email detected");
        return Ok(());
    }
    let message: Message<'_> = MessageParser::new().parse(body).ok_or_else(|| {
        raise_error!(
            "Email header parse result is not available".into(),
            ErrorCode::InternalError
        )
    })?;

    if let Ok(account) = AccountModel::get(account_id) {
        if let Some(ref rules) = account.archive_rules {
            let sender = message.from().and_then(|addr| {
                AddrVec::from(addr)
                    .0
                    .into_iter()
                    .next()
                    .and_then(|a| a.address)
            });
            let subject = message.subject().map(|s| s.to_string());

            let is_spam = !rules.spam_headers.is_empty()
                && rules.spam_headers.iter().any(|h| {
                    message
                        .header_raw(h.clone())
                        .map(|v| matches!(v.trim().to_lowercase().as_str(), "yes" | "true"))
                        .unwrap_or(false)
                });

            if !rules.should_archive(sender.as_deref(), subject.as_deref(), size, is_spam) {
                tracing::debug!(
                    account_id,
                    uid,
                    sender = sender.as_deref().unwrap_or("?"),
                    subject = subject.as_deref().unwrap_or("?"),
                    "Email filtered out by archive rules"
                );
                return Ok(());
            }
        }
    }

    let preview_limit = 100;
    let text = if let Some(text) = message.body_text(0).map(|cow| cow.into_owned()) {
        text
    } else if let Some(html) = message.body_html(0).map(|cow| cow.into_owned()) {
        extract_text(html)
    } else {
        String::new()
    };

    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    let preview = if text.chars().count() > preview_limit {
        text.chars().take(preview_limit).collect::<String>() + "..."
    } else {
        text.clone()
    };

    let body_text = text;

    let message_id = message
        .message_id()
        .map(String::from)
        .unwrap_or_else(generate_message_id);

    let in_reply_to = message.in_reply_to().as_text().map(String::from);
    let references = extract_references(&message);
    let thread_id = compute_thread_id(in_reply_to, references, &message_id);

    let mut subject = message.subject().map(String::from).unwrap_or_default();
    if subject.contains('\u{FFFD}') {
        subject = normalize_subject(message.header_raw(HeaderName::Subject));
    }

    let date = message.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0);
    let internal_date = if internal_date == 0 {
        date
    } else {
        internal_date
    };
    let parse_addrs = |addrs: Option<&Address<'_>>| {
        addrs
            .map(|addr| {
                AddrVec::from(addr)
                    .0
                    .into_iter()
                    .filter_map(|a| a.address)
                    .collect()
            })
            .unwrap_or_default()
    };

    let bcc = parse_addrs(message.bcc());
    let cc = parse_addrs(message.cc());
    let to = parse_addrs(message.to());

    let from = message
        .from()
        .and_then(|addr| AddrVec::from(addr).0.into_iter().next())
        .and_then(|add| add.address)
        .unwrap_or_else(|| "unknown".to_string());
    let attachment_count = message.attachment_count();
    let attachments =
        detach_and_store_attachments(body, &message, &email_content_hash, account_id, mailbox_id)
            .await;

    let envelope_id = Uuid::new_v4().to_string();
    let now = utc_now!();

    let mut final_tags = Vec::new();

    if let Some(meta_header) = message.header_raw("X-Bichon-Metadata") {
        if let Some(bmd) = parse_bichon_metadata(meta_header) {
            if let Some(tags) = bmd.tags {
                let validated_tags: Result<Vec<String>, _> = tags
                    .iter()
                    .map(|tag| Facet::from_text(tag).map(|_| tag.clone()).map_err(|e| e))
                    .collect();

                match validated_tags {
                    Ok(valid_list) => {
                        final_tags = valid_list;
                    }
                    Err(e) => {
                        eprintln!("Tag validation failed, ignoring all tags: {:#?}", e);
                    }
                }
            }
        }
    }

    let attachment_docs: Vec<TantivyDocument> = attachments
        .iter()
        .filter(|a| !a.inline || a.content_id.is_none())
        .map(|a| {
            let has_text = a.extracted_text.is_some();
            AttachmentModel {
                id: Uuid::new_v4().to_string(),
                envelope_id: envelope_id.clone(),
                account_id,
                account_email: None,
                mailbox_id,
                mailbox_name: None,
                subject: subject.clone(),
                content_hash: a.content_hash.clone(),
                from: from.clone(),
                date,
                ingest_at: now,
                size: a.size as u64,
                ext: a.get_extension(),
                category: a.get_category().to_string(),
                content_type: a.file_type.clone(),
                shard_id: 0,
                text: a.extracted_text.clone(),
                has_text,
                is_ocr: a.extracted_is_ocr,
                page_count: a.extracted_page_count.map(|n| n as u64),
                is_indexed: has_text,
                is_message: a.is_message,
                name: a.filename.clone(),
                tags: None,
                auto_tags: None,
            }
        })
        .map(|a| a.into_document())
        .collect();

    let envelope = Envelope {
        id: envelope_id,
        message_id,
        account_id,
        mailbox_id,
        uid,
        subject,
        preview,
        from,
        to,
        cc,
        bcc,
        date,
        internal_date,
        ingest_at: now,
        size,
        thread_id,
        attachment_count,
        regular_attachment_count: attachment_docs.len(),
        tags: (!final_tags.is_empty()).then_some(final_tags),
        account_email: None,
        mailbox_name: None,
        content_hash: email_content_hash.clone(),
        account_name: None,
    };
    // 'attachments' contains both regular and inline attachments
    let ea = EnvelopeWithAttachments {
        envelope,
        attachments: Some(attachments),
    };
    let doc = ea.to_document(&body_text, 0)?;
    tracing::debug!(
        "[account {}][mailbox {}] extract: uid={} msg_id={} content_hash={}",
        account_id,
        mailbox_id,
        uid,
        &ea.envelope.message_id,
        &ea.envelope.content_hash,
    );
    ENVELOPE_MANAGER.queue(doc).await;
    DEDUP_CACHE.insert(account_id, mailbox_id, &email_content_hash);
    for doc in attachment_docs {
        ATTACHMENT_MANAGER.queue(doc).await;
    }
    Ok(())
}

pub fn extract_envelope_from_nested_message(
    message: Message<'_>,
    account_id: u64,
) -> BichonResult<Envelope> {
    let text = if let Some(text) = message.body_text(0).map(|cow| cow.into_owned()) {
        text
    } else if let Some(html) = message.body_html(0).map(|cow| cow.into_owned()) {
        extract_text(html)
    } else {
        String::new()
    };

    let message_id = message
        .message_id()
        .map(String::from)
        .unwrap_or_else(generate_message_id);

    let in_reply_to = message.in_reply_to().as_text().map(String::from);
    let references = extract_references(&message);
    let thread_id = compute_thread_id(in_reply_to, references, &message_id);

    let mut subject = message.subject().map(String::from).unwrap_or_default();
    if subject.contains('\u{FFFD}') {
        subject = normalize_subject(message.header_raw(HeaderName::Subject));
    }

    let date = message.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0);

    let parse_addrs = |addrs: Option<&Address<'_>>| {
        addrs
            .map(|addr| {
                AddrVec::from(addr)
                    .0
                    .into_iter()
                    .filter_map(|a| a.address)
                    .collect()
            })
            .unwrap_or_default()
    };

    let bcc = parse_addrs(message.bcc());
    let cc = parse_addrs(message.cc());
    let to = parse_addrs(message.to());

    let from = message
        .from()
        .and_then(|addr| AddrVec::from(addr).0.into_iter().next())
        .and_then(|add| add.address)
        .unwrap_or_else(|| "unknown".to_string());

    let envelope = Envelope {
        id: Default::default(),
        message_id,
        account_id,
        mailbox_id: Default::default(),
        uid: Default::default(),
        subject,
        preview: text,
        from,
        to,
        cc,
        bcc,
        date,
        internal_date: Default::default(),
        ingest_at: Default::default(),
        size: Default::default(),
        thread_id,
        attachment_count: Default::default(),
        regular_attachment_count: Default::default(),
        tags: Default::default(),
        account_email: Default::default(),
        account_name: Default::default(),
        mailbox_name: Default::default(),
        content_hash: Default::default(),
    };

    Ok(envelope)
}

pub fn compute_thread_id(
    in_reply_to: Option<String>,
    references: Option<Vec<String>>,
    message_id: &str,
) -> String {
    if in_reply_to.is_some() && references.as_ref().map_or(false, |r| !r.is_empty()) {
        return hex_hash(&references.as_ref().unwrap()[0]);
    }
    hex_hash(message_id)
}

pub fn generate_message_id() -> String {
    let ts = utc_now!();
    let pid = std::process::id();
    format!("<{:016x}.{}.{}@{}>", id!(128), ts, pid, "bichon")
}

pub fn extract_references(message: &Message<'_>) -> Option<Vec<String>> {
    match message.references() {
        mail_parser::HeaderValue::Text(cow) => Some(vec![cow.to_string()]),
        mail_parser::HeaderValue::TextList(vec) => {
            Some(vec.iter().map(|cow| cow.to_string()).collect())
        }
        _ => None,
    }
}

pub async fn detach_and_store_attachments(
    original_body: &[u8],
    message: &Message<'_>,
    eml_content_hash: &str,
    account_id: u64,
    mailbox_id: u64,
) -> Vec<AttachmentInfo> {
    let rules = if account_id > 0 {
        AccountModel::get(account_id)
            .ok()
            .and_then(|a| a.extraction_rules)
    } else {
        None
    };

    let mailbox_name = match rules.as_ref().map(|r| !r.folders.is_empty()) {
        Some(true) => MailBox::get(mailbox_id).ok().map(|mb| mb.name),
        _ => None,
    };

    let sender = message
        .from()
        .and_then(|addr| AddrVec::from(addr).0.into_iter().next())
        .and_then(|add| add.address);

    let mut stripped_eml = original_body.to_vec();
    let mut attachment_infos = Vec::new();
    // Step 1: Collect and sort attachment ranges in reverse to maintain offset integrity
    let mut ranges: Vec<_> = message
        .attachments()
        .map(|att| {
            (
                att.raw_body_offset() as usize,
                att.raw_end_offset() as usize,
                att,
            )
        })
        .collect();

    ranges.sort_by(|a, b| b.0.cmp(&a.0));
    let mut attachments = Vec::with_capacity(ranges.len());

    // Collect candidates for text extraction (non-inline, known document types).
    struct TextCandidate {
        content_hash: String,
        file_type: String,
        ext: String,
        bytes: Vec<u8>,
    }
    let mut text_candidates: Vec<TextCandidate> = Vec::new();

    for (raw_start, raw_end, att) in ranges {
        // mail-parser may report attachment offsets past the body end for
        // malformed messages; clamp the range to avoid a slice panic.
        let body_len = original_body.len();
        let raw_start = raw_start.min(body_len);
        let raw_end = raw_end.min(body_len);
        let range_valid = raw_start < raw_end;

        // content hash is computed from the decoded attachment contents,
        // which is always available regardless of raw offset validity.
        let content_hash = compute_content_hash(att.contents());

        if range_valid {
            let raw_bytes = &original_body[raw_start..raw_end];
            // The actual content stored in the blob is the raw undecoded data.
            attachments.push((
                BlobManager::raw_attachment_key(&content_hash),
                Bytes::copy_from_slice(raw_bytes),
            ));
            attachments.push((
                BlobManager::decoded_attachment_key(&content_hash),
                Bytes::copy_from_slice(att.contents()),
            ));

            // Replace raw attachment content with a hash-based placeholder
            let placeholder = format!("<<BICHON_DETACH_HASH:{}>>", &content_hash);
            stripped_eml.splice(raw_start..raw_end, placeholder.as_bytes().iter().cloned());
        } else {
            // Invalid range: store a zero-length blob so the consistency
            // check passes; reattachment will log a warning for the missing
            // blob data but won't panic.
            attachments.push((BlobManager::raw_attachment_key(&content_hash), Bytes::new()));
            attachments.push((
                BlobManager::decoded_attachment_key(&content_hash),
                Bytes::copy_from_slice(att.contents()),
            ));
        }

        let inline = att
            .content_disposition()
            .map(|d| d.is_inline())
            .unwrap_or_else(|| att.content_id().is_some());
        let file_type = att
            .content_type()
            .map(|ct| {
                format!(
                    "{}/{}",
                    ct.c_type.as_ref(),
                    ct.c_subtype.as_deref().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let has_cid = att.content_id().is_some();
        let att_name = att.attachment_name().map(|n| n.to_string());
        let ext = att_name
            .as_deref()
            .and_then(|n| {
                std::path::Path::new(n)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
            })
            .unwrap_or_default();

        let should_extract = rules.as_ref().map_or(true, |r| {
            r.should_extract(
                &ext,
                mailbox_name.as_deref(),
                att_name.as_deref(),
                sender.as_deref(),
            )
        });

        if !inline || !has_cid {
            let decoded_len = att.contents().len();
            if should_extract
                && decoded_len <= crate::ext::text_extractor::MAX_EXTRACT_BYTES
                && crate::ext::text_extractor::should_try_extract(&file_type, &ext)
            {
                text_candidates.push(TextCandidate {
                    content_hash: content_hash.clone(),
                    file_type: file_type.clone(),
                    ext: ext.clone(),
                    bytes: att.contents().to_vec(),
                });
            }
        }

        let info = AttachmentInfo {
            filename: att.attachment_name().map(|n| n.to_string()),
            size: att.contents().len(),
            inline,
            file_type,
            content_id: att.content_id().map(|id| id.to_string()),
            content_hash: content_hash.clone(),
            is_message: att.is_message(),
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        };

        attachment_infos.push(info);
    }

    // Run text extraction in a single spawn_blocking batch.
    if !text_candidates.is_empty() {
        if let Ok(mut extracted_map) = tokio::task::spawn_blocking(move || {
            let mut map: std::collections::HashMap<String, (String, Option<u32>, bool)> =
                std::collections::HashMap::new();
            for c in text_candidates {
                if let Some(r) =
                    crate::ext::text_extractor::extract_text(&c.file_type, &c.ext, &c.bytes)
                {
                    map.insert(c.content_hash, (r.text, r.page_count, r.is_ocr));
                }
            }
            map
        })
        .await
        {
            for info in &mut attachment_infos {
                if let Some((text, pages, is_ocr)) = extracted_map.remove(&info.content_hash) {
                    info.extracted_text = Some(text);
                    info.extracted_page_count = pages;
                    info.extracted_is_ocr = is_ocr;
                }
            }
        }
    }
    // Step 4: Store the final stripped EML content
    BLOB_MANAGER
        .queue(DetachedEmail {
            email: (eml_content_hash.to_string(), Bytes::from(stripped_eml)),
            attachments: Some(attachments),
        })
        .await;

    attachment_infos
}

pub fn reattach_eml_content(
    account_id: u64,
    envelope_id: String,
) -> BichonResult<(Envelope, Bytes)> {
    let e = ENVELOPE_MANAGER
        .get_envelope_by_id(account_id, &envelope_id)?
        .ok_or_else(|| {
            raise_error!(
                format!(
                    "Envelope not found: account_id={} envelope_id={}",
                    account_id, &envelope_id
                ),
                ErrorCode::ResourceNotFound
            )
        })?;

    let restored_eml = BLOB_MANAGER
        .get_email(&e.envelope.content_hash)?
        .ok_or_else(|| {
            raise_error!(
                format!(
                "Original email content not found: account_id={} envelope_id={} content_hash={}",
                account_id, &envelope_id, &e.envelope.content_hash
            ),
                ErrorCode::ResourceNotFound
            )
        })?;

    if !e.envelope.has_any_attachments() {
        return Ok((e.envelope, restored_eml));
    }

    let mut restored_eml = restored_eml.to_vec();
    let actual_count = e.attachments.as_ref().map(|a| a.len()).unwrap_or(0);
    if e.envelope.attachment_count != actual_count {
        return Err(raise_error!(
            format!(
                "Consistency check failed: envelope.attachment_count ({}) does not match attachments.len ({})",
                e.envelope.attachment_count,
                actual_count
            ),
            ErrorCode::InternalError
        ));
    }

    let mut tasks = Vec::new();
    for detail in e.attachments.unwrap() {
        let placeholder_str = format!("<<BICHON_DETACH_HASH:{}>>", &detail.content_hash);
        let pattern = placeholder_str.as_bytes();
        let pattern_len = pattern.len();

        let mut search_cursor = 0;
        while let Some(pos) = restored_eml[search_cursor..]
            .windows(pattern_len)
            .position(|window| window == pattern)
        {
            let absolute_start = search_cursor + pos;
            let absolute_end = absolute_start + pattern_len;

            tasks.push((absolute_start, absolute_end, detail.content_hash.clone()));
            search_cursor = absolute_end;
        }
    }

    tasks.sort_by(|a, b| b.0.cmp(&a.0));

    for (start, end, hash) in tasks {
        if let Some(original_data) = BLOB_MANAGER.get_attachment(&hash)? {
            restored_eml.splice(start..end, original_data.iter().cloned());
        } else {
            error!("[ERROR] Missing attachment blob for hash: {}", hash);
        }
    }

    Ok((e.envelope, Bytes::from(restored_eml)))
}

/// Returns the raw EML for an indexed message, self-healing a missing content blob.
///
/// Behaves like [`reattach_eml_content`], but when the message's content blob is
/// absent from the blob store it fetches that single message on demand from the
/// IMAP server (`UID FETCH <uid> (BODY.PEEK[])`), persists it for future requests,
/// and returns it. If the on-demand fetch itself fails, the original "content not
/// found" error from [`reattach_eml_content`] is surfaced unchanged so the caller
/// still produces its 404.
pub async fn reattach_eml_content_self_healing(
    account_id: u64,
    envelope_id: String,
) -> BichonResult<(Envelope, Bytes)> {
    let envelope = ENVELOPE_MANAGER
        .get_envelope_by_id(account_id, &envelope_id)?
        .ok_or_else(|| {
            raise_error!(
                format!(
                    "Envelope not found: account_id={} envelope_id={}",
                    account_id, &envelope_id
                ),
                ErrorCode::ResourceNotFound
            )
        })?
        .envelope;

    // Fast path: the content blob is present, reuse the regular reattach logic.
    if BLOB_MANAGER.get_email(&envelope.content_hash)?.is_some() {
        return reattach_eml_content(account_id, envelope_id);
    }

    // The blob is missing. Try to recover it directly from the IMAP server.
    match recover_message_blob(&envelope).await {
        Ok(raw_body) => {
            tracing::info!(
                account_id,
                envelope_id = %envelope_id,
                uid = envelope.uid,
                "Self-healed missing email content blob via on-demand IMAP fetch"
            );
            Ok((envelope, raw_body))
        }
        Err(e) => {
            tracing::warn!(
                account_id,
                envelope_id = %envelope_id,
                uid = envelope.uid,
                error = %e,
                "On-demand IMAP fetch for missing content blob failed; returning not-found"
            );
            Err(e)
        }
    }
}

/// Fetches one message from IMAP and re-stores its detached blob.
///
/// On success the freshly fetched raw RFC822 body is returned; it is also queued
/// (in detached form) into the blob store so subsequent requests hit the cache.
/// Fails if the message cannot be fetched, or if the fetched bytes do not match
/// the archived `content_hash` (the server-side message no longer matches what
/// Bichon archived, so it cannot be treated as a recovery of that blob).
pub async fn recover_message_blob(envelope: &Envelope) -> BichonResult<Bytes> {
    let mailbox =
        MailBox::find_mailbox(envelope.account_id, envelope.mailbox_id)?.ok_or_else(|| {
            raise_error!(
                format!(
                    "Mailbox not found: account_id={} mailbox_id={}",
                    envelope.account_id, envelope.mailbox_id
                ),
                ErrorCode::ResourceNotFound
            )
        })?;

    let mut session = ImapExecutor::create_connection(envelope.account_id).await?;
    let result = ImapExecutor::fetch_single_message_body(
        &mut session,
        &mailbox.encoded_name(),
        envelope.uid,
    )
    .await;
    session.logout().await.ok();
    let raw_body = result?;

    let fetched_hash = compute_content_hash(&raw_body);
    if fetched_hash != envelope.content_hash {
        return Err(raise_error!(
            format!(
                "Fetched message does not match archived content: expected content_hash={} got={}",
                envelope.content_hash, fetched_hash
            ),
            ErrorCode::ImapUnexpectedResult
        ));
    }

    // Re-create the detached blob (stripped EML + attachments) so the missing
    // blob is repopulated for future requests. The detached EML is queued under
    // `fetched_hash`, which equals `envelope.content_hash`.
    let message = MessageParser::new()
        .parse(raw_body.as_slice())
        .ok_or_else(|| {
            raise_error!(
                "Failed to parse fetched email content".into(),
                ErrorCode::InternalError
            )
        })?;
    detach_and_store_attachments(
        &raw_body,
        &message,
        &fetched_hash,
        envelope.account_id,
        envelope.mailbox_id,
    )
    .await;

    Ok(Bytes::from(raw_body))
}

#[cfg(test)]
mod test {
    use html2text::config;

    #[test]
    fn test_various_html_with_overflow_enabled() {
        let cases = [
            ("<p>Hello World</p>", "Simple paragraph"),
            ("<h1>Title</h1><p>Content</p>", "Heading + paragraph"),
            ("<ul><li>Item1</li><li>Item2</li></ul>", "Unordered list"),
            (
                "<strong>Bold</strong> and <em>italic</em>",
                "Inline formatting",
            ),
            (
                "<div><span>Nested</span> elements</div>",
                "Nested inline elements inside block",
            ),
            (
                "<table><tr><td>A</td><td>B</td></tr></table>",
                "Simple table",
            ),
            (
                "<pre>  preformatted text\n  line2</pre>",
                "Preformatted block",
            ),
            ("😃 emoji test", "Wide emoji"),
            ("<a href=\"#\">link</a>", "Anchor tag"),
            (
                "<blockquote><p>Quoted text</p></blockquote>",
                "Blockquote with paragraph",
            ),
        ];

        for (html, desc) in cases {
            let result = config::plain()
                .allow_width_overflow()
                .string_from_read(html.as_bytes(), 100);

            match result {
                Ok(output) => {
                    println!("✓ Rendered ({}) =>\n{}", desc, output);
                }
                Err(e) => panic!("Unexpected error for {}: {:?}", desc, e),
            }
        }
    }

    /// Verifies that [`super::detach_and_store_attachments`] does not panic
    /// when mail-parser reports attachment offsets past the raw body length.
    ///
    /// Regression test for: "range end index X out of range for slice of
    /// length Y" panic caused by a malformed email whose attachment
    /// `raw_end_offset` exceeded the actual body size.
    #[tokio::test]
    async fn detach_attachments_bounds_check() {
        let raw = concat!(
            "From: sender@example.com\r\n",
            "To: recipient@example.com\r\n",
            "Subject: Test\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"bnd\"\r\n",
            "\r\n",
            "--bnd\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "Hello\r\n",
            "--bnd\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=\"test.bin\"\r\n",
            "\r\n",
            "AAAAABBBBBCCCCCDDDDDEEEEEAAAAABBBBBCCCCCDDDDDEEEEE\r\n",
            "--bnd--\r\n",
        )
        .as_bytes()
        .to_vec();

        let message = mail_parser::MessageParser::new()
            .parse(&raw)
            .expect("parse valid MIME message");
        assert_eq!(message.attachment_count(), 1);

        // Truncate the raw body so the attachment's raw_end_offset lies
        // past the body end — exactly the scenario reported by users.
        let truncated = &raw[..raw.len() - 20];
        assert!(truncated.len() < raw.len());

        // Must not panic.
        let infos =
            super::detach_and_store_attachments(truncated, &message, "test_content_hash", 0, 0)
                .await;

        // The attachment count must still match so the consistency check
        // in reattach_eml_content doesn't fail later.
        assert_eq!(infos.len(), 1);
    }
}
