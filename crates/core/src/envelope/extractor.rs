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

use async_imap::types::Fetch;
use bytes::Bytes;
use mail_parser::{Address, HeaderName, Message, MessageParser, MimeHeaders};
use tantivy::{schema::Facet, TantivyDocument};
use tracing::error;
use uuid::Uuid;

use crate::{
    account::migration::{AccountModel, ArchiveRules, ExtractionRules},
    archive::imap::mailbox::MailBox,
    backup::gate::{BACKUP_ACQUIRE_TIMEOUT, WRITE_GATE, WriteGate},
    common::AddrVec,
    envelope::{meta::parse_bichon_metadata, utils::normalize_subject},
    error::{code::ErrorCode, BichonResult},
    id,
    imap::executor::ImapExecutor,
    message::content::AttachmentInfo,
    metrics,
    raise_error,
    store::{
        blob::{DetachedEmail, BLOB_MANAGER},
        envelope::Envelope,
        tantivy::{
            attachment::ATTACHMENT_MANAGER,
            dedup_cache::DEDUP_CACHE,
            envelope::ENVELOPE_MANAGER,
            model::{AttachmentModel, EnvelopeWithAttachments},
        },
    },
    utc_now,
    utils::{compute_content_hash, hex_hash, html::extract_text},
};

/// The outcome of extracting an envelope. `Duplicate` means the message was
/// skipped because its content hash was already archived. `Filtered` means the
/// account's archive rules rejected it (it was never stored). `Imported` covers
/// every other processed message; import surfaces still count `Filtered` as a
/// success because the file itself was processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ExtractOutcome {
    Imported,
    Duplicate,
    Filtered,
}

pub async fn extract_envelope_and_store_it(
    fetch: Fetch,
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<ExtractOutcome> {
    let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
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
            // The message was not archived, so it counts as a failed ingest.
            metrics::count_email_ingested(account_id, "imap", "failed");
            return Ok(ExtractOutcome::Imported);
        }
    };
    let size = fetch.size.unwrap_or(body.len() as u32);
    record_ingest_outcome(
        account_id,
        "imap",
        extract_envelope_core(
            body,
            uid,
            size,
            internal_date,
            account_id,
            mailbox_id,
            None,
        )
        .await,
    )
}

pub async fn extract_envelope_from_eml(
    body: &[u8],
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<ExtractOutcome> {
    let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    record_ingest_outcome(
        account_id,
        "import",
        extract_envelope_core(body, 0, body.len() as u32, 0, account_id, mailbox_id, None).await,
    )
}

pub async fn extract_envelope_from_smtp(
    body: &[u8],
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<ExtractOutcome> {
    let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    record_ingest_outcome(
        account_id,
        "smtp",
        extract_envelope_core(
            body,
            0,
            body.len() as u32,
            utc_now!(),
            account_id,
            mailbox_id,
            None,
        )
        .await,
    )
}

/// Archive a journal delivery: `inner` is the original message the journal
/// report carried, `report` is the report exactly as the tenant delivered it.
///
/// The archived email is the **inner** message — that is the mail a user
/// searches, opens and exports, and it is what the content hash, the Merkle
/// leaf and the dedup cache refer to. The report is preserved beside it as a
/// `message/rfc822` attachment, so the artifact the tenant actually sent stays
/// retrievable byte for byte without polluting every envelope's headers with
/// the report's outer From/To.
///
/// The journal wrapper is vendor-specific in its *body* but not in its
/// structure: Exchange wraps the original in a `message/rfc822` part, Google's
/// routing rule forwards it as a plain message. The caller decides which case
/// applies; this function only assembles the result.
pub async fn extract_envelope_from_journal(
    inner: &[u8],
    report: Option<&[u8]>,
    account_id: u64,
    mailbox_id: u64,
) -> BichonResult<ExtractOutcome> {
    let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    // `size` describes the archived message (the inner one); the report is a
    // separate attachment and has its own size.
    record_ingest_outcome(
        account_id,
        "journal",
        extract_envelope_core(
            inner,
            0,
            inner.len() as u32,
            utc_now!(),
            account_id,
            mailbox_id,
            report,
        )
        .await,
    )
}

/// Count one ingestion attempt by its final outcome, then hand the result
/// back unchanged. The counter helpers are infallible, so this never alters
/// the ingest path's behavior.
fn record_ingest_outcome(
    account_id: u64,
    source: &str,
    result: BichonResult<ExtractOutcome>,
) -> BichonResult<ExtractOutcome> {
    match &result {
        Ok(ExtractOutcome::Imported) => {
            metrics::count_email_ingested(account_id, source, "imported")
        }
        Ok(ExtractOutcome::Duplicate) => {
            metrics::count_email_ingested(account_id, source, "duplicate")
        }
        Ok(ExtractOutcome::Filtered) => {
            metrics::count_email_ingested(account_id, source, "filtered")
        }
        Err(_) => metrics::count_email_ingested(account_id, source, "failed"),
    }
    result
}

/// First mailbox of `From`: `(display name, address)`.
fn first_mailbox(message: &Message<'_>) -> Option<(Option<String>, Option<String>)> {
    let first = match message.from()? {
        Address::List(list) => list.first(),
        Address::Group(groups) => groups.iter().find_map(|group| group.addresses.first()),
    }?;
    Some((
        first.name.as_ref().map(|name| name.to_string()),
        first.address.as_ref().map(|address| address.to_string()),
    ))
}

/// The sender archive rules see: the first address in `From`, without the
/// display name.
fn first_sender(message: &Message<'_>) -> Option<String> {
    first_mailbox(message).and_then(|(_, address)| address)
}

/// The sender an envelope is stored with.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ResolvedSender {
    /// Address written to `Envelope.from`.
    pub address: Option<String>,
    /// Display name paired with `address`.
    pub name: Option<String>,
    /// Original `From` address, set only when a header rule replaced it.
    pub wire_from: Option<String>,
}

/// Parse one header value as an RFC 5322 mailbox.
///
/// The value is fed through the same `From:` parser the real header uses, so
/// MIME encoded-words are decoded exactly once and both shapes a provider may
/// emit (a bare address, or `Name <address>`) are handled.
fn parse_header_mailbox(raw: &str) -> Option<(Option<String>, String)> {
    let raw = raw.trim();
    // A header value never legitimately contains a newline; allowing one
    // would let a crafted header inject extra header lines into the synthetic
    // message below.
    if raw.is_empty() || raw.contains('\r') || raw.contains('\n') {
        return None;
    }
    let synthetic = format!("From: {raw}\r\n\r\n");
    let message = MessageParser::new().parse(synthetic.as_bytes())?;
    let (name, address) = first_mailbox(&message)?;
    Some((name, address?))
}

/// Resolve the stored sender for an account.
///
/// `override_from_headers` is the account's ordered rule list. The first
/// header that parses to a mailbox wins; an invalid or absent header leaves
/// the real `From` untouched. The original address is preserved in
/// `wire_from` only when the rule actually changed it.
pub fn resolve_sender(
    message: &Message<'_>,
    override_from_headers: Option<&[String]>,
) -> ResolvedSender {
    let (actual_name, actual_address) = first_mailbox(message).unwrap_or((None, None));
    let mut resolved = ResolvedSender {
        address: actual_address.clone(),
        name: actual_name,
        wire_from: None,
    };

    let Some(headers) = override_from_headers else {
        return resolved;
    };
    for header in headers {
        let Some(raw) = message.header_raw(header.as_str()) else {
            continue;
        };
        let Some((header_name, header_address)) = parse_header_mailbox(raw) else {
            continue;
        };
        match &actual_address {
            Some(actual) if actual.eq_ignore_ascii_case(&header_address) => {
                // Same mailbox: keep the real value, but prefer a display name
                // supplied by the rule header when it has one.
                if header_name.is_some() {
                    resolved.name = header_name;
                }
            }
            Some(actual) => {
                resolved.wire_from = Some(actual.clone());
                resolved.address = Some(header_address);
                resolved.name = header_name.or(resolved.name);
            }
            None => {
                resolved.address = Some(header_address);
                resolved.name = header_name;
            }
        }
        break;
    }
    resolved
}

/// Whether any configured spam header carries exactly `yes` / `true`.
fn has_spam_header(message: &Message<'_>, spam_headers: &[String]) -> bool {
    !spam_headers.is_empty()
        && spam_headers.iter().any(|h| {
            message
                .header_raw(h.clone())
                .map(|v| matches!(v.trim().to_lowercase().as_str(), "yes" | "true"))
                .unwrap_or(false)
        })
}

/// Apply an account's archive rules to a parsed message.
///
/// Shared by ingestion and gap-fill so both make the same decision. Gap-fill
/// calls [`should_archive_from_headers`] with a header-only blob before it
/// downloads the body.
pub fn should_archive_message(rules: &ArchiveRules, message: &Message<'_>, size: u32) -> bool {
    let sender = first_sender(message);
    let subject = message.subject().map(|s| s.to_string());
    let is_spam = has_spam_header(message, &rules.spam_headers);
    rules.should_archive(sender.as_deref(), subject.as_deref(), size, is_spam)
}

/// Apply archive rules to a raw header block, without fetching the body.
///
/// `None` means the header block could not be parsed. Callers should then fall
/// back to the normal body path, which makes the decision after a full parse.
pub fn should_archive_from_headers(
    rules: &ArchiveRules,
    headers: &[u8],
    size: u32,
) -> Option<bool> {
    let message = MessageParser::new().parse(headers)?;
    Some(should_archive_message(rules, &message, size))
}

/// If `raw` is a journal report that wraps the original message in a
/// `message/rfc822` part, return the wrapped original.
///
/// **Standard MIME only.** Exchange puts the original in a `message/rfc822`
/// part and wraps it in a vendor-specific report body; that body is where the
/// *envelope* recipients (including BCC) live, and it is deliberately **not**
/// inspected: its format differs between providers and versions, and a parser
/// for it would be a liability. The consequence is documented rather than
/// hidden — a BCC-only recipient is not visible in the original's headers, so
/// per-recipient discovery cannot find them. Completeness of the journal itself
/// is unaffected: the message is archived either way.
///
/// `None` means the delivery already *is* the original message — Google
/// Workspace's routing rule forwards a plain copy rather than wrapping it.
pub fn journal_delivery_inner(raw: &[u8]) -> Option<Vec<u8>> {
    let message = MessageParser::default().parse(raw)?;
    // Bound to a local so the iterator temporary is dropped before `message`
    // is; the returned bytes are a copy.
    let inner = message
        .attachments()
        .find(|att| att.is_message())
        .map(|att| att.contents().to_vec());
    inner
}

/// Archive one message. `journal_report` — present only on the journal path —
/// is the report the message arrived in; it is stored as an attachment so the
/// tenant-delivered artifact stays retrievable.
async fn extract_envelope_core(
    body: &[u8],
    uid: u32,
    size: u32,
    internal_date: i64,
    account_id: u64,
    mailbox_id: u64,
    journal_report: Option<&[u8]>,
) -> BichonResult<ExtractOutcome> {
    // The content hash of the original raw EML
    let email_content_hash = compute_content_hash(body);
    if DEDUP_CACHE.contains(account_id, mailbox_id, &email_content_hash) {
        tracing::debug!("Duplicate email detected");
        // println!("Duplicate email detected");
        return Ok(ExtractOutcome::Duplicate);
    }
    // A message that cannot be parsed is a per-message failure, not a system
    // failure: streaming callers match on EmailParseError to skip the message
    // (it is NOT archived) and continue with the rest of the batch instead of
    // letting one broken email abort the whole folder (#377).
    let message: Message<'_> = MessageParser::new().parse(body).ok_or_else(|| {
        raise_error!(
            format!(
                "Email parse failed (uid {uid}, {size} bytes): message not archived"
            ),
            ErrorCode::EmailParseError
        )
    })?;

    let account = AccountModel::get(account_id).ok();
    if let Some(ref account) = account {
        if let Some(ref rules) = account.archive_rules {
            if !should_archive_message(rules, &message, size) {
                let sender = first_sender(&message);
                tracing::debug!(
                    account_id,
                    uid,
                    sender = sender.as_deref().unwrap_or("?"),
                    subject = message.subject().unwrap_or("?"),
                    "Email filtered out by archive rules"
                );
                return Ok(ExtractOutcome::Filtered);
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

    // Retention floor: never ingest a message whose effective date is already
    // outside the account's retention window. The sweep purges those envelopes,
    // so without this a full UID re-sync (UIDVALIDITY change / mailbox rebuild)
    // would treat them as missing and re-download them, fighting the sweep
    // forever. While an account is on legal hold retention is suspended, so the
    // floor is disabled and nothing is dropped.
    if let Some(ref account) = account {
        let days = account.retention_days_effective();
        if days > 0 && !account.is_on_hold() {
            if let Some(effective) = crate::retention::effective_date_ms(date, internal_date) {
                if effective < crate::retention::retention_cutoff_ms(days) {
                    tracing::debug!(
                        account_id,
                        uid,
                        effective,
                        "Email outside retention window, skipping ingest"
                    );
                    return Ok(ExtractOutcome::Imported);
                }
            }
        }
    }
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

    // Stored sender: a configured alias header may replace the address (see
    // `resolve_sender`); archive rules above deliberately keep using the real
    // `From` through `first_sender`.
    let sender = resolve_sender(
        &message,
        account
            .as_ref()
            .and_then(|a| a.override_from_headers.as_deref()),
    );
    let from = sender.address.clone().unwrap_or_else(|| "unknown".to_string());
    let attachment_count = message.attachment_count();
    let attachments = detach_and_store_attachments(
        body,
        &message,
        &email_content_hash,
        account_id,
        mailbox_id,
        journal_report,
    )
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
                from_name: sender.name.clone(),
                wire_from: sender.wire_from.clone(),
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

    // Assign the stable archive-side UID here — the last point before a
    // document exists, and after every path that can decide not to create one
    // (dedup, archive rules, retention floor). Write-once: every later
    // reconstruction carries it from the document instead of recomputing.
    let imap_uid = crate::archive::imap::stable_uid::next_uid(account_id)?;

    let envelope = Envelope {
        id: envelope_id,
        message_id,
        account_id,
        mailbox_id,
        uid,
        imap_uid,
        // Just archived: nothing has moved it out of the source mailbox yet.
        cleanup_moved: false,
        subject,
        preview,
        from,
        from_name: sender.name,
        wire_from: sender.wire_from,
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
    Ok(ExtractOutcome::Imported)
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

    let account = AccountModel::get(account_id).ok();
    let sender = resolve_sender(
        &message,
        account
            .as_ref()
            .and_then(|a| a.override_from_headers.as_deref()),
    );
    let from = sender.address.clone().unwrap_or_else(|| "unknown".to_string());

    let envelope = Envelope {
        id: Default::default(),
        message_id,
        account_id,
        mailbox_id: Default::default(),
        uid: Default::default(),
        imap_uid: Default::default(),
        cleanup_moved: false,
        subject,
        preview: text,
        from,
        from_name: sender.name,
        wire_from: sender.wire_from,
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

/// Suffix of the synthetic Message-ID minted by [`generate_message_id`] for a
/// message that carries no `Message-ID` header. Such an id is generated locally
/// and cannot exist on the IMAP server, so the Message-ID search fallback in
/// [`fetch_archived_body`] skips it instead of paying a guaranteed-empty round
/// trip. Kept next to the format string so the two cannot drift apart.
const SYNTHETIC_MESSAGE_ID_SUFFIX: &str = "@bichon>";

pub fn generate_message_id() -> String {
    let ts = utc_now!();
    let pid = std::process::id();
    format!(
        "<{:016x}.{}.{}{}",
        id!(128),
        ts,
        pid,
        SYNTHETIC_MESSAGE_ID_SUFFIX
    )
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

/// Prefix of the placeholder that replaces an attachment body inside a
/// stripped EML. Full form: `<<BICHON_DETACH_HASH:{64 hex chars}>>`.
///
/// Which hash the 64 hex chars carry depends on when the record was written,
/// so tooling that scans placeholders must consult the attachment metadata
/// (`AttachmentInfo::raw_content_hash`) — the prefix alone cannot tell the
/// domains apart, because both produce 64-character blake3 hex digests:
///
/// - **Current records** (`raw_content_hash` is `Some`): the
///   domain-separated hash of the *original encoded* MIME body
///   (`compute_raw_attachment_hash`). The blob is stored under the same key.
/// - **Legacy records** (pre-#375, `raw_content_hash` is `None`): the plain
///   hash of the *decoded* attachment contents (`compute_content_hash`),
///   which is also the blob key. `AttachmentInfo::storage_hash()` selects
///   the right one per record.
pub const DETACH_PLACEHOLDER_PREFIX: &str = "<<BICHON_DETACH_HASH:";

/// Decoded contents of an attachment selected for text extraction, collected
/// while detaching so the extraction can run in one background batch.
pub struct TextCandidate {
    pub content_hash: String,
    pub file_type: String,
    pub ext: String,
    pub bytes: Vec<u8>,
}

/// Context deciding which detached attachments are queued for text
/// extraction. Supplied by the live ingest path; offline tools pass `None`
/// to [`strip_attachment_bodies`] and skip extraction entirely.
pub struct ExtractionFilter<'a> {
    /// Per-account rules; `None` means no rules are configured and the
    /// default is to extract every supported type.
    pub rules: Option<&'a ExtractionRules>,
    pub mailbox_name: Option<&'a str>,
    pub sender: Option<&'a str>,
}

/// Output of [`strip_attachment_bodies`].
pub struct DetachedBody {
    /// The EML with every valid attachment body replaced by a
    /// `<<BICHON_DETACH_HASH:...>>` placeholder.
    pub stripped_eml: Vec<u8>,
    /// `(storage key, raw encoded MIME body)` pairs to persist in the blob
    /// store. Invalid attachment ranges contribute a shared empty record in
    /// the raw key domain.
    pub blobs: Vec<(String, Bytes)>,
    /// Attachment metadata, in descending raw-offset order.
    pub infos: Vec<AttachmentInfo>,
    /// Attachments selected for text extraction (empty unless an
    /// [`ExtractionFilter`] was supplied).
    pub text_candidates: Vec<TextCandidate>,
}

/// Strips attachment bodies out of a raw message and computes everything the
/// storage layer needs to persist and later reconstruct it: the stripped EML
/// with hash placeholders, the raw blob records, attachment metadata, and
/// text-extraction candidates.
///
/// This is the single canonical implementation shared by the live ingest path
/// ([`detach_and_store_attachments`]) and the offline store migration; the
/// two must never diverge, or a migrated store stops matching what the
/// runtime writes and reads back.
pub fn strip_attachment_bodies(
    original_body: &[u8],
    message: &Message<'_>,
    filter: Option<&ExtractionFilter<'_>>,
) -> DetachedBody {
    let mut stripped_eml = original_body.to_vec();
    let mut infos = Vec::new();
    let mut blobs = Vec::with_capacity(message.attachment_count());
    let mut text_candidates = Vec::new();

    // Collect and sort attachment ranges in reverse to maintain offset
    // integrity while splicing placeholders.
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

        let raw_content_hash = if range_valid {
            let raw_bytes = &original_body[raw_start..raw_end];
            // Different transfer encodings of one file must not share a raw blob.
            let raw_hash = crate::utils::compute_raw_attachment_hash(raw_bytes);
            blobs.push((raw_hash.clone(), Bytes::copy_from_slice(raw_bytes)));

            // Replace raw attachment content with a hash-based placeholder
            let placeholder = format!("{DETACH_PLACEHOLDER_PREFIX}{raw_hash}>>");
            stripped_eml.splice(raw_start..raw_end, placeholder.as_bytes().iter().cloned());
            Some(raw_hash)
        } else {
            // No replacement is needed, but keep the empty storage record in
            // the raw key domain so it cannot shadow a decoded or email blob.
            let raw_hash = crate::utils::compute_raw_attachment_hash(b"");
            blobs.push((raw_hash.clone(), Bytes::new()));
            Some(raw_hash)
        };

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

        // Collect candidates for text extraction (non-inline, known document
        // types allowed by the filter).
        if let Some(filter) = filter {
            let should_extract = filter.rules.map_or(true, |r| {
                r.should_extract(
                    &ext,
                    filter.mailbox_name,
                    att_name.as_deref(),
                    filter.sender,
                )
            });

            if (!inline || !has_cid)
                && should_extract
                && att.contents().len() <= crate::ext::text_extractor::max_extract_bytes()
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

        infos.push(AttachmentInfo {
            filename: att.attachment_name().map(|n| n.to_string()),
            size: att.contents().len(),
            inline,
            file_type,
            content_id: att.content_id().map(|id| id.to_string()),
            content_hash,
            raw_content_hash,
            is_message: att.is_message(),
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        });
    }

    DetachedBody {
        stripped_eml,
        blobs,
        infos,
        text_candidates,
    }
}

pub async fn detach_and_store_attachments(
    original_body: &[u8],
    message: &Message<'_>,
    eml_content_hash: &str,
    account_id: u64,
    mailbox_id: u64,
    journal_report: Option<&[u8]>,
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

    let sender = first_sender(&message);

    let filter = ExtractionFilter {
        rules: rules.as_ref(),
        mailbox_name: mailbox_name.as_deref(),
        sender: sender.as_deref(),
    };
    let mut detached = strip_attachment_bodies(original_body, message, Some(&filter));

    // A journal delivery keeps the tenant's report beside the message it
    // carried. Stored as an ordinary attachment so it survives export, the
    // integrity check and the reattach path with no special casing.
    if let Some(report) = journal_report {
        let hash = compute_content_hash(report);
        detached.infos.push(AttachmentInfo {
            file_type: "message/rfc822".to_string(),
            inline: false,
            filename: Some("journal-report.eml".to_string()),
            size: report.len(),
            content_id: None,
            content_hash: hash.clone(),
            raw_content_hash: None,
            is_message: true,
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        });
        detached.blobs.push((hash, Bytes::from(report.to_vec())));
    }

    let mut attachment_infos = detached.infos;

    // Run text extraction in a single spawn_blocking batch.
    let text_candidates = detached.text_candidates;
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
            email: (
                eml_content_hash.to_string(),
                Bytes::from(detached.stripped_eml),
            ),
            attachments: Some(detached.blobs),
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

    let restored = reattach_parts(restored_eml.to_vec(), e.attachments.unwrap(), |hash| {
        BLOB_MANAGER.get_attachment(hash)
    })?;
    Ok((e.envelope, restored))
}

fn reattach_parts(
    mut restored_eml: Vec<u8>,
    attachments: Vec<AttachmentInfo>,
    load: impl Fn(&str) -> BichonResult<Option<Bytes>>,
) -> BichonResult<Bytes> {
    let mut tasks = Vec::new();
    for detail in attachments {
        let storage_hash = detail.storage_hash();

        // Primary scan: placeholders written by the record's own format
        // (raw-body hash for new records, decoded hash for legacy records).
        let found = schedule_placeholder(&mut tasks, &restored_eml, storage_hash, None);

        // Mixed-format recovery: a re-ingest refreshes the envelope metadata
        // to raw storage keys, but the blob store's `exists` dedup keeps the
        // previously stored stripped EML, whose placeholders still carry the
        // legacy decoded hash. When the primary scan finds nothing, retry with
        // the legacy placeholder; the old blob is still stored under that key.
        // The raw key is kept as an alternate load target for the case where
        // the legacy blob was garbage-collected after the original envelope
        // was deleted (the re-ingest stored the identical bytes under it).
        if found == 0 && detail.raw_content_hash.is_some() {
            schedule_placeholder(
                &mut tasks,
                &restored_eml,
                &detail.content_hash,
                Some(storage_hash),
            );
        }
    }

    tasks.sort_by(|a, b| b.0.cmp(&a.0));
    tasks.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

    for (start, end, hash, alt) in tasks {
        let original_data = match load(&hash)? {
            Some(data) => Some(data),
            None => match alt.as_deref() {
                Some(alt) => load(alt)?,
                None => None,
            },
        };
        if let Some(original_data) = original_data {
            restored_eml.splice(start..end, original_data.iter().cloned());
        } else {
            error!("[ERROR] Missing attachment blob for hash: {}", hash);
        }
    }

    Ok(Bytes::from(restored_eml))
}

/// Schedules one replacement task per occurrence of the placeholder carrying
/// `hash` and returns the number of occurrences found. `alt` is a secondary
/// storage key tried at load time when the primary blob is absent.
fn schedule_placeholder(
    tasks: &mut Vec<(usize, usize, String, Option<String>)>,
    eml: &[u8],
    hash: &str,
    alt: Option<&str>,
) -> usize {
    let placeholder_str = format!("{DETACH_PLACEHOLDER_PREFIX}{hash}>>");
    let pattern = placeholder_str.as_bytes();
    let pattern_len = pattern.len();

    let mut found = 0;
    let mut search_cursor = 0;
    while let Some(pos) = eml[search_cursor..]
        .windows(pattern_len)
        .position(|window| window == pattern)
    {
        let absolute_start = search_cursor + pos;
        let absolute_end = absolute_start + pattern_len;

        tasks.push((
            absolute_start,
            absolute_end,
            hash.to_string(),
            alt.map(str::to_string),
        ));
        search_cursor = absolute_end;
        found += 1;
    }
    found
}

/// Returns the raw EML for an indexed message, self-healing a missing content
/// blob.
///
/// Behaves like [`reattach_eml_content`], but when the message's content blob
/// is absent from the blob store it fetches that single message on demand from
/// the IMAP server (`UID FETCH <uid> (BODY.PEEK[])`), persists it for future
/// requests, and returns it. If the on-demand fetch itself fails, the original
/// "content not found" error from [`reattach_eml_content`] is surfaced
/// unchanged so the caller still produces its 404.
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
        Ok((raw_body, _persisted)) => {
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
/// On success the freshly fetched raw RFC822 body is returned; it is also
/// queued (in detached form) into the blob store so subsequent requests hit the
/// cache. Fails if the message cannot be fetched, or if the fetched bytes do
/// not match the archived `content_hash` (the server-side message no longer
/// matches what Bichon archived, so it cannot be treated as a recovery of that
/// blob).
/// Returns the fetched body together with whether the detached blob was
/// actually persisted. `false` means the write was skipped because a backup
/// window was open — the caller still gets the content, but the blob store was
/// left untouched so the running snapshot stays byte-stable.
async fn recover_message_blob(envelope: &Envelope) -> BichonResult<(Bytes, bool)> {
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
    let result = fetch_archived_body(&mut session, &mailbox.encoded_name(), envelope).await;
    session.logout().await.ok();
    let raw_body = result?;
    // `fetch_archived_body` only returns bytes whose content hash equals the
    // archived one, so the detached EML is queued under the envelope's own hash.
    let fetched_hash = envelope.content_hash.clone();

    // Re-create the detached blob (stripped EML + attachments) so the missing
    // blob is repopulated for future requests.
    let message = MessageParser::new()
        .parse(raw_body.as_slice())
        .ok_or_else(|| {
            raise_error!(
                "Failed to parse fetched email content".into(),
                ErrorCode::InternalError
            )
        })?;
    // Persist the recovered blob for future requests — unless a backup window
    // is open, in which case we still return the content to the caller but skip
    // the write so the blob store stays byte-stable for the running snapshot.
    // Reads remain available during a backup. If the gate is open, hold the
    // admission guard through queueing so the backup cannot race the write.
    let persisted = persist_recovered_blob_if_allowed(
        &raw_body,
        &message,
        &fetched_hash,
        envelope.account_id,
        envelope.mailbox_id,
        &WRITE_GATE,
    )
    .await;

    Ok((Bytes::from(raw_body), persisted))
}

async fn persist_recovered_blob_if_allowed(
    raw_body: &[u8],
    message: &Message<'_>,
    fetched_hash: &str,
    account_id: u64,
    mailbox_id: u64,
    gate: &WriteGate,
) -> bool {
    let Ok(_write_guard) = gate.check() else {
        return false;
    };
    detach_and_store_attachments(raw_body, message, fetched_hash, account_id, mailbox_id, None)
        .await;
    true
}


/// Fetches the archived message from IMAP and returns it only if its content
/// hash equals the archived `content_hash`. Tries the stored UID first, then
/// the UIDs returned by `UID SEARCH HEADER Message-ID` (UID changed or the
/// message moved within the folder).
async fn fetch_archived_body(
    session: &mut async_imap::Session<Box<dyn crate::imap::session::SessionStream>>,
    encoded_mailbox: &str,
    envelope: &Envelope,
) -> BichonResult<Vec<u8>> {
    let mut last_err =
        match ImapExecutor::fetch_single_message_body(session, encoded_mailbox, envelope.uid).await {
            Ok(body) if compute_content_hash(&body) == envelope.content_hash => return Ok(body),
            Ok(body) => raise_error!(
                format!(
                    "Fetched message does not match archived content: expected content_hash={} got={}",
                    envelope.content_hash,
                    compute_content_hash(&body)
                ),
                ErrorCode::ImapUnexpectedResult
            ),
            Err(e) => e,
        };

    // The stored UID failed, so the message was most likely renumbered or
    // moved. Search by Message-ID instead — but only when the id can actually
    // exist on the server (see `searchable_message_id`).
    //
    // `uid_search` runs against the mailbox the preceding
    // `fetch_single_message_body` selected with EXAMINE, so the search and the
    // fetches below stay in the same folder.
    let Some(message_id) = searchable_message_id(&envelope.message_id) else {
        return Err(last_err);
    };
    let uids = session
        .uid_search(format!("HEADER Message-ID \"{}\"", message_id))
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapUnexpectedResult))?;
    let uids = bounded_message_id_candidates(uids, envelope.uid);
    for uid in uids {
        match ImapExecutor::fetch_single_message_body(session, encoded_mailbox, uid).await {
            Ok(body) if recovered_body_matches(&body, envelope) => return Ok(body),
            Ok(_) => {}
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// The trimmed Message-ID to search for, or `None` when a `UID SEARCH HEADER
/// Message-ID` cannot possibly match.
///
/// Two cases yield nothing: an id minted by [`generate_message_id`] for a
/// message archived without a `Message-ID` header (no server-side header can
/// equal it), and an id that would break out of the IMAP quoted string. Both
/// are skipped rather than sent, so a renumbering repair never pays a
/// guaranteed-empty round trip.
fn searchable_message_id(message_id: &str) -> Option<&str> {
    let message_id = message_id.trim();
    if message_id.is_empty()
        || message_id.ends_with(SYNTHETIC_MESSAGE_ID_SUFFIX)
        || message_id.contains(['"', '\\', '\r', '\n'])
    {
        return None;
    }
    Some(message_id)
}

const MAX_MESSAGE_ID_CANDIDATES: usize = 32;

fn bounded_message_id_candidates<I>(uids: I, stored_uid: u32) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut candidates: Vec<u32> = uids.into_iter().filter(|&uid| uid != stored_uid).collect();
    candidates.sort_unstable();
    candidates.truncate(MAX_MESSAGE_ID_CANDIDATES);
    candidates
}

fn recovered_body_matches(body: &[u8], envelope: &Envelope) -> bool {
    if compute_content_hash(body) != envelope.content_hash {
        return false;
    }
    if envelope.message_id.is_empty() {
        return true;
    }
    MessageParser::new()
        .parse(body)
        .and_then(|message| message.message_id().map(String::from))
        .is_some_and(|message_id| message_id == envelope.message_id)
}

/// [`BlobRepairReport::status`] for a record that was already intact. Named so
/// the repair-history ring can skip these without matching a bare literal.
pub const STATUS_COMPLETE: &str = "complete";

/// Outcome of [`repair_envelope_blobs`].
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BlobRepairReport {
    pub envelope_id: String,
    /// Account that owns the record. Part of the identity carried on the report
    /// so a caller holding no envelope can still label the outcome — the
    /// integrity failures table has only a failure row, not an envelope.
    pub account_id: u64,
    /// Mailbox the record lives in. See [`Self::account_id`].
    pub mailbox_id: u64,
    /// Subject of the record, for display. See [`Self::account_id`].
    pub subject: String,
    /// RFC `Message-ID` of the record, for display. See [`Self::account_id`].
    pub message_id: String,
    /// `complete` (nothing missing), `repaired`, `partial` or `failed`.
    pub status: String,
    /// Whether the email blob was missing before repair.
    pub email_missing_before: bool,
    /// Whether the email blob is still missing after repair.
    pub email_missing_after: bool,
    /// Mirrors [`Self::email_missing_before`]; kept so a client that reads a
    /// single email-missing flag keeps working after the before/after split.
    pub email_blob_missing: bool,
    /// `true` when the fetched content was validated but the write was skipped
    /// because a backup window was open. The repair is then reported as
    /// `partial` even though the source data was recovered — retry once the
    /// backup finishes.
    pub skipped_due_to_backup: bool,
    /// Attachment hashes to suspect before the repair. Best-effort naming only:
    /// the verdict is [`Self::status`], so this can be empty while the record is
    /// broken (see [`known_placeholder_hashes`]).
    pub missing_before: Vec<String>,
    /// Attachment hashes to suspect after the repair. Same caveat as
    /// [`Self::missing_before`]: `status: "partial"` with an empty list is a
    /// valid outcome.
    pub missing_after: Vec<String>,
    pub error: Option<String>,
}

/// Diagnosis of a record's blob store state, from [`diagnose_blobs`].
struct BlobDiagnosis {
    /// The email blob is absent from the store.
    email_missing: bool,
    /// The archived body cannot be reconstructed byte-for-byte. Authoritative:
    /// this — not [`Self::unresolved`] — decides whether a repair is attempted.
    broken: bool,
    /// Best-effort naming of the attachment hashes to suspect. See
    /// [`known_placeholder_hashes`] for what it does and does not promise.
    unresolved: Vec<String>,
}

/// Inspects a record's blob store state.
///
/// The verdict is an *integrity* check, not a presence check: splicing every
/// stored blob back into the detached EML must reproduce the exact body the
/// envelope's `content_hash` was taken from. `strip_attachment_bodies` stores
/// the identical raw bytes it removes, so a healthy record hashes back to
/// itself and any gap at all — a missing attachment blob, a placeholder the
/// metadata cannot account for, a blob sitting under an unexpected key —
/// changes the hash.
///
/// Presence alone instead reports a record as complete whenever the parts it
/// happened to look for are there, which is how a record whose *metadata* is
/// unreadable slips through as `complete` while nothing is repaired.
///
/// Reuses [`reattach_parts`] on purpose, at the cost of materialising the
/// reattached body: re-deriving the placeholder/alternate-key resolution here
/// is exactly the duplication that lets the two views drift apart.
fn diagnose_blobs(
    envelope: &Envelope,
    attachments: Option<&[AttachmentInfo]>,
) -> BichonResult<BlobDiagnosis> {
    let Some(eml) = BLOB_MANAGER.get_email(&envelope.content_hash)? else {
        return Ok(BlobDiagnosis {
            email_missing: true,
            broken: true,
            unresolved: Vec::new(),
        });
    };

    let attachments = attachments.unwrap_or_default();
    let restored = reattach_parts(eml.to_vec(), attachments.to_vec(), |hash| {
        BLOB_MANAGER.get_attachment(hash)
    })?;
    let intact = compute_content_hash(&restored) == envelope.content_hash;
    Ok(BlobDiagnosis {
        email_missing: false,
        broken: !intact,
        unresolved: if intact {
            Vec::new()
        } else {
            known_placeholder_hashes(&eml, attachments)
        },
    })
}

/// Content hashes referenced by `<<BICHON_DETACH_HASH:...>>` placeholders in a
/// detached EML.
fn placeholder_hashes(detached_eml: &[u8]) -> Vec<String> {
    const PREFIX: &[u8] = b"<<BICHON_DETACH_HASH:";
    let mut hashes = Vec::new();
    let mut rest = detached_eml;
    while let Some(pos) = rest.windows(PREFIX.len()).position(|w| w == PREFIX) {
        rest = &rest[pos + PREFIX.len()..];
        let Some(end) = rest.windows(2).position(|w| w == b">>") else {
            break;
        };
        if let Ok(h) = std::str::from_utf8(&rest[..end]) {
            if !hashes.iter().any(|x| x == h) {
                hashes.push(h.to_string());
            }
        }
        rest = &rest[end + 2..];
    }
    hashes
}

/// Naming aid for a [`BlobDiagnosis`]: the placeholders in a detached EML that
/// the record's own attachment metadata can account for.
///
/// A placeholder carries the *storage* hash — the raw MIME key for records
/// written since the raw/decoded split, the decoded hash for legacy ones —
/// while `content_hash` is always the decoded hash. Both domains are indexed
/// because a re-ingest can leave them mixed: the metadata refreshed to raw keys
/// while the stored EML still holds legacy decoded placeholders.
///
/// Best-effort only, and never a verdict: a placeholder the metadata does not
/// list is omitted, so a literal `<<BICHON_DETACH_HASH:...>>` in the body can
/// never be named as a missing attachment. That also means this can return
/// nothing while the record is genuinely broken — when the attachment metadata
/// itself is what cannot be read. [`diagnose_blobs`] decides; this only helps a
/// human see which parts to look at.
fn known_placeholder_hashes(detached_eml: &[u8], attachments: &[AttachmentInfo]) -> Vec<String> {
    let indexed = attachments
        .iter()
        .flat_map(|item| [item.storage_hash(), item.content_hash.as_str()])
        .collect::<std::collections::HashSet<_>>();
    placeholder_hashes(detached_eml)
        .into_iter()
        .filter(|hash| indexed.contains(hash.as_str()))
        .collect()
}

/// Re-fetches one archived message from IMAP and restores its missing email
/// or attachment blobs **in place**: the envelope, its id and its index entry
/// are kept, so no duplicate is created. Nothing is written unless the
/// fetched message hashes to the archived `content_hash`.
///
/// Only *missing* blobs are healable. A blob that is present but corrupt is
/// reported — `status: "partial"` on every attempt, however often it is
/// retried — and deliberately left alone: the blob store keeps the first write
/// for a key, so this path could only overwrite by deleting first, and nothing
/// has needed that yet. A permanently `partial` record is therefore not
/// necessarily a bad IMAP fetch; read the diagnosis before assuming so.
pub async fn repair_envelope_blobs(
    account_id: u64,
    envelope_id: &str,
) -> BichonResult<BlobRepairReport> {
    let envelope_with_attachments = ENVELOPE_MANAGER
        .get_envelope_by_id(account_id, envelope_id)?
        .ok_or_else(|| {
            raise_error!(
                format!("Envelope not found: account_id={account_id} envelope_id={envelope_id}"),
                ErrorCode::ResourceNotFound
            )
        })?;
    let attachments = envelope_with_attachments.attachments;
    let envelope = envelope_with_attachments.envelope;

    // Attachment metadata that cannot be read back (or no longer matches the
    // count the envelope records) leaves the record's parts unenumerable, so
    // the integrity check below could not verify it and this endpoint — which
    // only ever writes blobs — could not heal it either. Fail loudly instead of
    // hashing a record we cannot enumerate and reporting the result as
    // `complete`. `reattach_eml_content` already rejects the same condition on
    // the read path, so this cannot regress a record that reads back fine.
    let actual_count = attachments.as_ref().map_or(0, Vec::len);
    if envelope.attachment_count != actual_count {
        return Ok(attachment_count_mismatch_report(
            envelope_id,
            &envelope,
            actual_count,
        ));
    }

    let diagnosis = diagnose_blobs(&envelope, attachments.as_deref())?;
    let mut report = BlobRepairReport {
        envelope_id: envelope_id.to_string(),
        account_id: envelope.account_id,
        mailbox_id: envelope.mailbox_id,
        subject: envelope.subject.clone(),
        message_id: envelope.message_id.clone(),
        email_missing_before: diagnosis.email_missing,
        email_missing_after: diagnosis.email_missing,
        email_blob_missing: diagnosis.email_missing,
        missing_before: diagnosis.unresolved,
        ..Default::default()
    };
    if !diagnosis.broken {
        report.status = STATUS_COMPLETE.into();
        return Ok(report);
    }
    let persisted = match recover_message_blob(&envelope).await {
        Ok((_, persisted)) => persisted,
        Err(e) => {
            report.status = "failed".into();
            report.error = Some(e.to_string());
            // Nothing was written, so the post-state is the pre-state: report
            // the same suspect set instead of leaving the client with an empty
            // `missing_after` and no way to tell what is still broken.
            report.missing_after = report.missing_before.clone();
            return Ok(report);
        }
    };
    report.skipped_due_to_backup = !persisted;
    // recover_message_blob only queues the write; wait for it to land.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), BLOB_MANAGER.drain()).await;
    let after = diagnose_blobs(&envelope, attachments.as_deref())?;
    apply_post_state(&mut report, &after);
    Ok(report)
}

/// The report for a record whose attachment metadata cannot be enumerated —
/// the only thing this endpoint can say about it is that it is broken.
///
/// Carries the envelope's identity so the repair-history ring can still label
/// the failure: a caller opening this from the integrity failures table holds a
/// failure row, not an envelope.
fn attachment_count_mismatch_report(
    envelope_id: &str,
    envelope: &Envelope,
    actual_count: usize,
) -> BlobRepairReport {
    BlobRepairReport {
        envelope_id: envelope_id.to_string(),
        account_id: envelope.account_id,
        mailbox_id: envelope.mailbox_id,
        subject: envelope.subject.clone(),
        message_id: envelope.message_id.clone(),
        status: "failed".into(),
        error: Some(format!(
            "Consistency check failed: envelope.attachment_count ({}) does not match attachments.len ({})",
            envelope.attachment_count, actual_count
        )),
        ..Default::default()
    }
}

/// Records the post-repair state. `status` follows the integrity verdict, so a
/// record that is still broken reads `partial` even when nothing can be named —
/// `unresolved` is a naming aid, not the verdict.
fn apply_post_state(report: &mut BlobRepairReport, after: &BlobDiagnosis) {
    report.email_missing_after = after.email_missing;
    report.missing_after = after.unresolved.clone();
    report.status = if after.broken { "partial" } else { "repaired" }.into();
}

#[cfg(test)]
mod test {
    use html2text::config;
    use std::sync::Once;

    /// Extract tests touch the process-global settings/DB/DEDUP_CACHE, so they
    /// need the same one-time environment as the backup config tests, and they
    /// serialize with those via the shared lock (convention: see
    /// `backup::BACKUP_STATE_TESTS`).
    static TEST_ENV: Once = Once::new();

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-extractor-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*crate::settings::cli::SETTINGS;
            let _ = &*crate::database::manager::DB_MANAGER;
        });
    }

    // Keep the blob writer alive across these storage tests.
    static STORAGE_RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
        std::sync::LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

    #[test]
    fn empty_attachment_uses_the_raw_storage_key_space() {
        STORAGE_RUNTIME.block_on(async {
            let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
            init_test_env();
            let raw = b"From: test@example.com\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=empty.bin\r\n\r\n";
            let message = mail_parser::MessageParser::new().parse(raw).unwrap();
            let infos = super::detach_and_store_attachments(
                raw,
                &message,
                &crate::utils::compute_content_hash(raw),
                0,
                0,
                None,
            )
            .await;
            assert_ne!(infos[0].storage_hash(), infos[0].content_hash);
        });
    }

    /// Ingest must hand every archived message a stable UID, and a duplicate
    /// must not consume one.
    ///
    /// The assertions are on the counter, not on the index: `ENVELOPE_MANAGER`
    /// is process-global and its writer task belongs to whichever test's runtime
    /// initialized it, so a document queued here is not reliably committed (the
    /// same coupling that retired the blob roundtrip test below). The document
    /// side is covered where it can be observed deterministically:
    /// `to_document` refuses an unassigned UID, `rebuild_envelope_doc` carries
    /// it (see `store::tantivy::envelope`), and the IMAP-side ordering reads it
    /// back (see `bichon-imap::uid_map`).
    #[test]
    fn ingest_assigns_a_stable_uid_and_dedup_does_not_burn_one() {
        use crate::account::migration::{Account, AccountType};
        use crate::database::{manager::DB_MANAGER, upsert_impl};

        STORAGE_RUNTIME.block_on(async {
            let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
            // Ingest queues into the process-global envelope index, so hold the
            // lock that serializes every test writing it (concurrent commits to
            // one tantivy directory are refused by Windows).
            let _index_guard = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;
            init_test_env();

            let account_id = 9_300_501;
            let mailbox_id = 77;
            // Ingest requires the account row: the UID counter lives on it.
            upsert_impl(
                DB_MANAGER.db(),
                Account {
                    id: account_id,
                    account_type: AccountType::IMAP,
                    ..Default::default()
                },
            )
            .unwrap();

            let eml = b"From: alice@example.com\r\n\
                        To: bob@example.com\r\n\
                        Subject: stable uid\r\n\
                        Date: Thu, 01 Jan 2023 00:00:00 +0000\r\n\
                        Message-ID: <uid-t5@example.com>\r\n\
                        \r\n\
                        body of the archived message\r\n";

            let first = super::extract_envelope_from_eml(eml, account_id, mailbox_id)
                .await
                .expect("first ingest");
            assert!(matches!(first, super::ExtractOutcome::Imported));
            let after_first = Account::get(account_id).unwrap().imap_uid_next;
            assert_eq!(
                after_first,
                crate::archive::imap::stable_uid::UID_BLOCK + 1,
                "the first ingest must reserve a block, i.e. hand out UID 1"
            );

            // The same bytes again: dedup short-circuits before the UID is
            // assigned, so the counter must not move.
            let second = super::extract_envelope_from_eml(eml, account_id, mailbox_id)
                .await
                .expect("second ingest");
            assert!(matches!(second, super::ExtractOutcome::Duplicate));
            assert_eq!(
                Account::get(account_id).unwrap().imap_uid_next,
                after_first,
                "a duplicate must not consume a UID"
            );
        });
    }

    // NOTE: a full detach→blob-store→reattach roundtrip test existed here but
    // was removed: it relied on the process-global BLOB_MANAGER, which other
    // test modules re-initialize against their own temp data dirs, so it only
    // passed when run in isolation. The reattach logic is covered by the
    // `reattach_parts` tests below with injected blob loads.

    /// The journal wrapper is detected by MIME structure, not by a vendor
    /// header: Exchange wraps the original in a `message/rfc822` part, Google's
    /// routing rule forwards a plain message. Both must land in the archive
    /// with the correct envelope, and a plain message must not be wrapped
    /// around itself.
    #[test]
    fn journal_delivery_inner_unwraps_only_a_real_wrapper() {
        let wrapped = b"From: MicrosoftExchange329e@example.com\r\n\
To: journal@example.com\r\n\
Subject: Journal report\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
Journal report body\r\n\
--b\r\n\
Content-Type: message/rfc822\r\n\
\r\n\
From: customer@example.org\r\n\
To: employee@example.com\r\n\
Subject: Inner subject\r\n\
\r\n\
Inner body\r\n\
--b--\r\n";
        let inner =
            super::journal_delivery_inner(wrapped).expect("a wrapped report must be unwrapped");
        let text = String::from_utf8_lossy(&inner);
        assert!(
            text.contains("Inner subject"),
            "the inner message was not returned: {text}"
        );
        assert!(
            !text.contains("Journal report body"),
            "the vendor report body leaked into the archived message: {text}"
        );

        // A plain message is its own inner. Returning it as a wrapper would
        // attach the message to itself and double its size.
        let plain =
            b"From: a@example.com\r\nTo: b@example.com\r\nSubject: plain\r\n\r\nbody\r\n";
        assert!(super::journal_delivery_inner(plain).is_none());
    }

    #[test]
    fn repeated_attachment_is_restored_once_per_placeholder() {
        let payload = b"BEGIN:VCALENDAR\r\nDESCRIPTION:Repeated invitation\r\nEND:VCALENDAR\r\n";
        let hash = crate::utils::compute_content_hash(payload);
        let placeholder = format!("<<BICHON_DETACH_HASH:{hash}>>");
        let stripped = format!("part-one\r\n{placeholder}\r\npart-two\r\n{placeholder}");
        let info = crate::message::content::AttachmentInfo {
            content_hash: hash,
            ..Default::default()
        };
        let restored =
            super::reattach_parts(stripped.into_bytes(), vec![info.clone(), info], |_| {
                Ok(Some(bytes::Bytes::from_static(payload)))
            })
            .expect("reattach repeated invitation");
        let expected = [
            b"part-one\r\n".as_slice(),
            payload,
            b"\r\npart-two\r\n",
            payload,
        ]
        .concat();
        assert_eq!(restored.as_ref(), expected);
    }

    #[test]
    fn distinct_transfer_encodings_keep_their_original_bytes() {
        let plain = b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n";
        let base64 = b"QkVHSU46VkNBTEVOREFSDQpFTkQ6VkNBTEVOREFSDQo=\r\n";
        let decoded_hash = crate::utils::compute_content_hash(plain);
        let plain_hash = crate::utils::compute_raw_attachment_hash(plain);
        let base64_hash = crate::utils::compute_raw_attachment_hash(base64);
        let infos = vec![
            crate::message::content::AttachmentInfo {
                content_hash: decoded_hash.clone(),
                raw_content_hash: Some(plain_hash.clone()),
                ..Default::default()
            },
            crate::message::content::AttachmentInfo {
                content_hash: decoded_hash,
                raw_content_hash: Some(base64_hash.clone()),
                ..Default::default()
            },
        ];
        let stripped = format!(
            "plain\r\n<<BICHON_DETACH_HASH:{plain_hash}>>base64\r\n<<BICHON_DETACH_HASH:{base64_hash}>>"
        );
        let restored = super::reattach_parts(stripped.into_bytes(), infos, |hash| {
            Ok(Some(if hash == plain_hash {
                bytes::Bytes::from_static(plain)
            } else {
                bytes::Bytes::from_static(base64)
            }))
        })
        .expect("reattach independently encoded copies");
        assert_eq!(
            restored.as_ref(),
            [b"plain\r\n".as_slice(), plain, b"base64\r\n", base64].concat()
        );
    }

    #[test]
    fn mixed_format_metadata_falls_back_to_legacy_placeholders() {
        // A stripped EML written by the pre-#375 format (decoded-hash
        // placeholders) combined with metadata refreshed by a re-ingest
        // (raw storage keys) must still reconstruct: the legacy blob under
        // the decoded key is still present in the blob store.
        let decoded_hash = "d".repeat(64);
        let raw_hash = "r".repeat(64);
        let info = crate::message::content::AttachmentInfo {
            content_hash: decoded_hash.clone(),
            raw_content_hash: Some(raw_hash),
            ..Default::default()
        };
        let stripped = format!("head\r\n<<BICHON_DETACH_HASH:{decoded_hash}>>\r\ntail");
        let restored =
            super::reattach_parts(stripped.into_bytes(), vec![info], |hash| {
                Ok(Some(if hash == decoded_hash {
                    bytes::Bytes::from_static(b"legacy body")
                } else {
                    bytes::Bytes::from_static(b"raw body")
                }))
            })
            .expect("reattach mixed-format record");
        assert_eq!(restored.as_ref(), b"head\r\nlegacy body\r\ntail".as_slice());
    }

    #[test]
    fn mixed_format_fallback_also_loads_the_raw_blob() {
        // Same mixed state, but the legacy blob under the decoded key was
        // garbage-collected after the original envelope was deleted. The
        // re-ingest stored the identical bytes under the raw key, which must
        // be used as the alternate load target.
        let decoded_hash = "d".repeat(64);
        let raw_hash = "r".repeat(64);
        let info = crate::message::content::AttachmentInfo {
            content_hash: decoded_hash.clone(),
            raw_content_hash: Some(raw_hash.clone()),
            ..Default::default()
        };
        let stripped = format!("<<BICHON_DETACH_HASH:{decoded_hash}>>");
        let restored =
            super::reattach_parts(stripped.into_bytes(), vec![info], |hash| {
                if hash == raw_hash {
                    Ok(Some(bytes::Bytes::from_static(b"raw body")))
                } else {
                    Ok(None)
                }
            })
            .expect("reattach via raw-key alternate");
        assert_eq!(restored.as_ref(), b"raw body".as_slice());
    }

    #[test]
    fn raw_blob_key_cannot_alias_legacy_attachment_or_email_key() {
        let payload = b"same bytes in a legacy decoded attachment or original email";
        assert_ne!(crate::utils::compute_raw_attachment_hash(payload), crate::utils::compute_content_hash(payload));
    }

    #[test]
    fn legacy_metadata_uses_the_original_decoded_storage_key() {
        let info: crate::message::content::AttachmentInfo = serde_json::from_str(
            r#"{"file_type":"application/pdf","inline":false,"filename":"a.pdf","size":4,"content_id":null,"content_hash":"legacy-hash","is_message":false,"extracted_text":null,"extracted_page_count":null}"#
        ).expect("read legacy attachment metadata");
        assert_eq!(info.storage_hash(), "legacy-hash");
    }

    /// A body that cannot be parsed must surface as `EmailParseError` (not a
    /// generic InternalError): the batch streaming path matches on that code
    /// to skip the single message and keep the batch going (#377). Pins the
    /// classification contract end to end — if a parser upgrade starts
    /// parsing this junk, the input here needs replacing, not the assertion.
    #[tokio::test]
    async fn unparseable_body_raises_email_parse_error() {
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        init_test_env();
        // mail-parser 0.11 is extremely lenient (even binary junk parses as a
        // header-less message); the empty body is the input it still refuses,
        // so it pins the classification. If a parser upgrade starts parsing
        // this, replace the input — the assertion is the contract.
        let junk: &[u8] = b"";
        assert!(
            mail_parser::MessageParser::new().parse(junk).is_none(),
            "test input must actually be unparseable for the classification to apply"
        );
        let err = super::extract_envelope_from_eml(junk, 1, 1)
            .await
            .expect_err("unparseable body must be an error");
        assert_eq!(
            err.code(),
            crate::error::code::ErrorCode::EmailParseError,
            "parse failures must classify as EmailParseError, got {err:?}"
        );
    }

    /// The premise of an integrity check that compares the reattached EML's
    /// content hash against `envelope.content_hash`: splicing every stored blob
    /// back must reproduce the original body *byte for byte*. Covers an inline
    /// CID attachment and a regular one, since the two take different branches
    /// in `strip_attachment_bodies`.
    #[test]
    fn reattaching_every_blob_reproduces_the_original_eml_byte_for_byte() {
        let raw = concat!(
            "Message-ID: <fixture@example.test>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=fixture\r\n",
            "\r\n",
            "--fixture\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "hello\r\n",
            "--fixture\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <logo@example.test>\r\n",
            "Content-Disposition: inline; filename=logo.png\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "aGVsbG8tbG9nbw==\r\n",
            "--fixture\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=fixture.bin\r\n\r\n",
            "fixture bytes\r\n",
            "--fixture--\r\n",
        )
        .as_bytes();
        let message = super::MessageParser::new().parse(raw).expect("fixture parses");
        let detached = super::strip_attachment_bodies(raw, &message, None);
        assert_eq!(detached.infos.len(), 2, "both attachments are detached");

        let stored: std::collections::HashMap<String, bytes::Bytes> =
            detached.blobs.iter().cloned().collect();
        let load = |hash: &str| Ok(stored.get(hash).cloned());

        let restored = super::reattach_parts(
            detached.stripped_eml.clone(),
            detached.infos.clone(),
            load,
        )
        .expect("reattach with every blob present");
        assert_eq!(
            crate::utils::compute_content_hash(&restored),
            crate::utils::compute_content_hash(raw),
            "a fully populated store must reconstruct the archived body exactly"
        );

        // Drop one blob: the placeholder survives into the output, so the hash
        // can no longer match. This is the signal the integrity check relies on.
        for dropped in 0..detached.infos.len() {
            let mut partial = stored.clone();
            partial.remove(detached.infos[dropped].storage_hash());
            let restored = super::reattach_parts(
                detached.stripped_eml.clone(),
                detached.infos.clone(),
                |hash| Ok(partial.get(hash).cloned()),
            )
            .expect("reattach with one blob missing");
            assert_ne!(
                crate::utils::compute_content_hash(&restored),
                crate::utils::compute_content_hash(raw),
                "a missing attachment blob must change the reconstructed hash"
            );
        }

        // The case a presence check gets wrong: metadata that cannot enumerate
        // the placeholders (an unreadable attachment list) leaves every one of
        // them in the output, so the record is still broken even though there is
        // nothing it could have listed as missing. The verdict has to be the
        // hash, not a list.
        let restored = super::reattach_parts(detached.stripped_eml.clone(), Vec::new(), |_| Ok(None))
            .expect("reattach with no attachment metadata");
        assert_ne!(
            crate::utils::compute_content_hash(&restored),
            crate::utils::compute_content_hash(raw),
            "unenumerable attachment metadata must not read as intact"
        );
    }

    #[test]
    fn placeholder_hashes_finds_unique_hashes() {
        let eml = b"a<<BICHON_DETACH_HASH:abc>>b<<BICHON_DETACH_HASH:def>>c<<BICHON_DETACH_HASH:abc>>";
        assert_eq!(super::placeholder_hashes(eml), vec!["abc", "def"]);
        assert!(super::placeholder_hashes(b"no placeholders").is_empty());
        assert!(super::placeholder_hashes(b"<<BICHON_DETACH_HASH:unterminated").is_empty());
    }

    /// A literal marker in the body must never be named as a missing
    /// attachment — the record's own metadata is what gates the list.
    #[test]
    fn known_placeholder_hashes_ignores_literal_body_markers() {
        let attachments = vec![super::AttachmentInfo {
            content_hash: "real".into(),
            ..Default::default()
        }];
        assert_eq!(
            super::known_placeholder_hashes(
                b"<<BICHON_DETACH_HASH:literal>><<BICHON_DETACH_HASH:real>>",
                &attachments,
            ),
            vec!["real"]
        );
    }

    /// A placeholder spliced into the stripped EML carries the *raw* storage
    /// key, which is a different hash domain from `AttachmentInfo::content_hash`
    /// (see `compute_raw_attachment_hash`). Matching on `content_hash` alone
    /// drops every current-format placeholder, leaving the report unable to name
    /// what is missing.
    #[test]
    fn known_placeholder_hashes_recognise_a_real_detached_attachment() {
        let raw = concat!(
            "Message-ID: <fixture@example.test>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=fixture\r\n",
            "\r\n",
            "--fixture\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "hello\r\n",
            "--fixture\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=fixture.bin\r\n\r\n",
            "fixture bytes\r\n",
            "--fixture--\r\n",
        )
        .as_bytes();
        let message = super::MessageParser::new().parse(raw).expect("fixture parses");
        let detached = super::strip_attachment_bodies(raw, &message, None);
        assert_eq!(detached.infos.len(), 1);
        let info = &detached.infos[0];
        // The two domains genuinely differ for a current-format record, which is
        // what makes the placeholder/inventory comparison below non-trivial.
        assert_ne!(info.storage_hash(), info.content_hash);
        assert_eq!(
            super::known_placeholder_hashes(&detached.stripped_eml, &detached.infos),
            vec![info.storage_hash().to_string()]
        );
    }

    fn diagnosis(email_missing: bool, broken: bool, unresolved: &[&str]) -> super::BlobDiagnosis {
        super::BlobDiagnosis {
            email_missing,
            broken,
            unresolved: unresolved.iter().map(|h| h.to_string()).collect(),
        }
    }

    #[test]
    fn post_state_marks_a_repaired_record_and_keeps_the_legacy_alias() {
        let mut report = super::BlobRepairReport {
            email_missing_before: true,
            email_blob_missing: true,
            ..Default::default()
        };
        super::apply_post_state(&mut report, &diagnosis(false, false, &[]));
        assert!(report.email_blob_missing);
        assert!(report.email_missing_before);
        assert!(!report.email_missing_after);
        assert_eq!(report.status, "repaired");
        // The backup-skip flag is reported separately and stays untouched by a
        // successful completion.
        assert!(!report.skipped_due_to_backup);
    }

    /// The regression this integrity check exists for: a record can be broken
    /// while nothing can be named, because the part that cannot be read is the
    /// metadata that would do the naming. `partial` must follow the verdict, not
    /// the (empty) list.
    #[test]
    fn post_state_is_partial_when_broken_even_with_nothing_named() {
        let mut report = super::BlobRepairReport::default();
        super::apply_post_state(&mut report, &diagnosis(false, true, &[]));
        assert!(report.missing_after.is_empty());
        assert_eq!(report.status, "partial");
    }

    #[test]
    fn post_state_reports_the_suspect_hashes_it_can_name() {
        let mut report = super::BlobRepairReport::default();
        super::apply_post_state(&mut report, &diagnosis(false, true, &["suspect-hash"]));
        assert_eq!(report.missing_after, vec!["suspect-hash"]);
        assert_eq!(report.status, "partial");
    }

    /// The count-mismatch guard returns before any repair is attempted, so its
    /// report is the only thing the caller and the repair-history ring get.
    /// It must carry the full identity — a `failed` record with an empty
    /// subject is unusable in the history list.
    #[test]
    fn mismatch_report_carries_the_envelope_identity() {
        let envelope = super::Envelope {
            account_id: 7,
            mailbox_id: 3,
            subject: "Quarterly report".into(),
            message_id: "<abc@example.com>".into(),
            attachment_count: 2,
            ..Default::default()
        };
        let report = super::attachment_count_mismatch_report("env-1", &envelope, 1);
        assert_eq!(report.envelope_id, "env-1");
        assert_eq!(report.status, "failed");
        assert_eq!(report.account_id, 7);
        assert_eq!(report.mailbox_id, 3);
        assert_eq!(report.subject, "Quarterly report");
        assert_eq!(report.message_id, "<abc@example.com>");
        assert!(report.error.unwrap().contains("does not match"));
    }

    /// A paused gate must skip the write outright (and release the admission it
    /// briefly took), while an open gate admits it and releases it again. Uses a
    /// private gate so the process-wide `WRITE_GATE` is never paused.
    ///
    /// The admitted branch queues into the process-global `BLOB_MANAGER`, so
    /// this test needs the shared test environment and lock and runs on
    /// `STORAGE_RUNTIME` — see `init_test_env`.
    #[test]
    fn paused_recovery_skips_persistence_and_releases_admission() {
        STORAGE_RUNTIME.block_on(async {
            let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
            init_test_env();

            let gate = super::WriteGate::new();
            gate.pause();
            let raw = b"From: sender@example.test\r\n\r\nbody";
            let message = super::MessageParser::new().parse(raw).expect("fixture parses");

            assert!(!super::persist_recovered_blob_if_allowed(
                raw,
                &message,
                &crate::utils::compute_content_hash(raw),
                0,
                0,
                &gate,
            )
            .await);
            assert_eq!(gate.in_flight(), 0);

            gate.resume();
            assert!(super::persist_recovered_blob_if_allowed(
                raw,
                &message,
                &crate::utils::compute_content_hash(raw),
                0,
                0,
                &gate,
            )
            .await);
            assert_eq!(gate.in_flight(), 0);
            super::BLOB_MANAGER.drain().await;
        });
    }

    #[test]
    fn recovery_guards_reject_wrong_hash_and_message_id() {
        let body = b"From: sender@example.test\r\nMessage-ID: <original@example.test>\r\n\r\nbody";
        let parsed_id = super::MessageParser::new()
            .parse(body)
            .and_then(|message| message.message_id().map(String::from))
            .expect("message-id parses");
        let mut envelope = super::Envelope {
            message_id: parsed_id.clone(),
            content_hash: super::compute_content_hash(body),
            ..Default::default()
        };
        assert!(super::recovered_body_matches(body, &envelope));
        envelope.message_id = "<other@example.test>".into();
        assert!(!super::recovered_body_matches(body, &envelope));
        envelope.message_id = parsed_id;
        envelope.content_hash = super::compute_content_hash(b"different");
        assert!(!super::recovered_body_matches(body, &envelope));
    }

    #[test]
    fn message_id_candidates_are_bounded_and_exclude_stored_uid() {
        let candidates = super::bounded_message_id_candidates((1..=100).rev(), 50);
        assert_eq!(candidates.len(), 32);
        assert_eq!(candidates.first(), Some(&1));
        assert_eq!(candidates.last(), Some(&32));
        assert!(!candidates.contains(&50));
    }

    #[test]
    fn message_id_search_skips_unsearchable_ids() {
        // A real Message-ID is searched for, trimmed.
        assert_eq!(
            super::searchable_message_id("  <real@example.test> "),
            Some("<real@example.test>")
        );
        assert_eq!(super::searchable_message_id(""), None);
        assert_eq!(super::searchable_message_id("   "), None);
        // The synthetic id minted for a message with no Message-ID header can
        // never match a server-side header. Asserting against the generator
        // itself keeps the marker constant and the format string in step.
        assert_eq!(
            super::searchable_message_id(&super::generate_message_id()),
            None
        );
        // Anything that could break out of the IMAP quoted string.
        assert_eq!(super::searchable_message_id("<a\"b@example.test>"), None);
        assert_eq!(super::searchable_message_id("<a\\b@example.test>"), None);
        assert_eq!(super::searchable_message_id("<a\rb@example.test>"), None);
        assert_eq!(super::searchable_message_id("<a\nb@example.test>"), None);
    }

    // The dedup/idempotence contract of the blob store (a re-detach must never
    // overwrite already-archived bytes) is tested in `crate::store::blob`, where
    // a private `BlobManager` can be built at a temp dir. It used to live here
    // and went through the process-global `BLOB_MANAGER`, which made it depend
    // on which test module initialized that global first and on that module's
    // runtime still being alive.

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
    #[test]
    fn detach_attachments_bounds_check() {
        STORAGE_RUNTIME.block_on(check_attachment_bounds());
    }

    async fn check_attachment_bounds() {
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        init_test_env();
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
            super::detach_and_store_attachments(truncated, &message, "test_content_hash", 0, 0, None)
                .await;

        // The attachment count must still match so the consistency check
        // in reattach_eml_content doesn't fail later.
        assert_eq!(infos.len(), 1);
    }
}

#[cfg(test)]
mod sender_resolution_tests {
    use super::*;

    fn resolve(raw: &str, headers: &[&str]) -> ResolvedSender {
        let message = MessageParser::new().parse(raw.as_bytes()).expect("parse");
        let owned: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
        resolve_sender(&message, Some(&owned))
    }

    #[test]
    fn keeps_display_name() {
        let r = resolve("From: Jane Doe <jane@example.com>\r\n\r\nbody", &[]);
        assert_eq!(r.address.as_deref(), Some("jane@example.com"));
        assert_eq!(r.name.as_deref(), Some("Jane Doe"));
        assert_eq!(r.wire_from, None);
    }

    #[test]
    fn decodes_encoded_word_name() {
        let r = resolve(
            "From: =?UTF-8?B?5byg5LiJ?= <zhangsan@example.com>\r\n\r\nbody",
            &[],
        );
        assert_eq!(r.name.as_deref(), Some("张三"));
    }

    #[test]
    fn addy_bare_address_replaces_from_and_keeps_wire() {
        let r = resolve(
            "From: \"Jane Doe 'jane@example.com'\" <myalias@addy.io>\r\n\
             X-AnonAddy-Original-Sender: jane@example.com\r\n\r\nbody",
            &["X-AnonAddy-Original-Sender"],
        );
        assert_eq!(r.address.as_deref(), Some("jane@example.com"));
        assert_eq!(r.wire_from.as_deref(), Some("myalias@addy.io"));
        // The alias header has no display name, so the real From name is kept.
        assert_eq!(r.name.as_deref(), Some("Jane Doe 'jane@example.com'"));
    }

    #[test]
    fn simplelogin_full_value_supplies_name_and_address() {
        let r = resolve(
            "From: Robert via SimpleLogin <alias@simplelogin.io>\r\n\
             X-SimpleLogin-Original-From: Robert <robert@example.com>\r\n\r\nbody",
            &["X-SimpleLogin-Original-From"],
        );
        assert_eq!(r.address.as_deref(), Some("robert@example.com"));
        assert_eq!(r.name.as_deref(), Some("Robert"));
        assert_eq!(r.wire_from.as_deref(), Some("alias@simplelogin.io"));
    }

    #[test]
    fn invalid_or_empty_header_falls_back() {
        for raw in [
            "From: Jane <jane@example.com>\r\nX-AnonAddy-Original-Sender: not an address\r\n\r\nbody",
            "From: Jane <jane@example.com>\r\nX-AnonAddy-Original-Sender:   \r\n\r\nbody",
        ] {
            let r = resolve(raw, &["X-AnonAddy-Original-Sender"]);
            assert_eq!(r.address.as_deref(), Some("jane@example.com"));
            assert_eq!(r.name.as_deref(), Some("Jane"));
            assert_eq!(r.wire_from, None);
        }
    }

    #[test]
    fn equal_address_does_not_create_a_wire_value() {
        let r = resolve(
            "From: Jane <jane@example.com>\r\nX-AnonAddy-Original-Sender: JANE@example.com\r\n\r\nbody",
            &["X-AnonAddy-Original-Sender"],
        );
        assert_eq!(r.address.as_deref(), Some("jane@example.com"));
        assert_eq!(r.wire_from, None);
    }

    #[test]
    fn first_matching_header_wins() {
        let r = resolve(
            "From: alias@addy.io\r\n\
             X-SimpleLogin-Original-From: b@example.com\r\n\
             X-AnonAddy-Original-Sender: a@example.com\r\n\r\nbody",
            &["X-AnonAddy-Original-Sender", "X-SimpleLogin-Original-From"],
        );
        assert_eq!(r.address.as_deref(), Some("a@example.com"));
    }

    #[test]
    fn only_the_first_from_mailbox_is_used() {
        let r = resolve(
            "From: Alice <a@example.com>, Bob <b@example.com>\r\n\r\nbody",
            &[],
        );
        assert_eq!(r.address.as_deref(), Some("a@example.com"));
        assert_eq!(r.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let r = resolve(
            "From: alias@simplelogin.io\r\nx-simplelogin-original-from: Carol <carol@example.com>\r\n\r\nbody",
            &["X-SimpleLogin-Original-From"],
        );
        assert_eq!(r.address.as_deref(), Some("carol@example.com"));
    }
}
