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
use crate::account::state::{DownloadState, DownloadStatus, FolderStatus};
use crate::cache::imap::mailbox::MailBox;
use crate::envelope::extractor::extract_envelope_and_store_it;
use crate::error::code::ErrorCode;
use crate::imap::session::SessionStream;
use crate::raise_error;
use crate::{error::BichonResult, imap::manager::ImapConnectionManager};
use async_imap::types::Name;
use async_imap::Session;
use futures::TryStreamExt;
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;
use tracing::info;

const BODY_FETCH_COMMAND: &str = "(UID INTERNALDATE RFC822.SIZE BODY.PEEK[])";
const SIZE_ONLY_FETCH: &str = "(UID RFC822.SIZE)";

fn classify_imap_error(e: &async_imap::error::Error) -> ErrorCode {
    match e {
        async_imap::error::Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        )
        .then_some(ErrorCode::NetworkError)
        .unwrap_or(ErrorCode::ImapCommandFailed),
        async_imap::error::Error::ConnectionLost => ErrorCode::NetworkError,
        _ => ErrorCode::ImapCommandFailed,
    }
}

pub struct ImapExecutor;

impl ImapExecutor {
    pub async fn list_all_mailboxes(
        session: &mut Session<Box<dyn SessionStream>>,
    ) -> BichonResult<Vec<Name>> {
        let list = session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        let result = list
            .try_collect::<Vec<Name>>()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        Ok(result)
    }

    pub async fn uid_search(
        session: &mut Session<Box<dyn SessionStream>>,
        mailbox_name: &str,
        query: &str,
    ) -> BichonResult<HashSet<u32>> {
        session
            .examine(mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        let result = session
            .uid_search(query)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        Ok(result)
    }

    pub async fn append(
        session: &mut Session<Box<dyn SessionStream>>,
        mailbox_name: impl AsRef<str>,
        flags: Option<&str>,
        internaldate: Option<&str>,
        content: impl AsRef<[u8]>,
    ) -> BichonResult<()> {
        session
            .append(mailbox_name, flags, internaldate, content)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))
    }

    /// Fetches new mail for a mailbox.
    ///
    /// When `before` is `Some(date)`, a two-step approach is used:
    /// `UID SEARCH` to find matching UIDs (standard IMAP), then batch `UID FETCH`
    /// for the specific UIDs. When `before` is `None`, bounded ranges are batched
    /// when the server supplied `UIDNEXT`; otherwise a direct ranged
    /// `UID FETCH {start}:*` is issued and results are streamed.
    ///
    /// Returns `Ok(Some(max_uid))` with the highest UID fetched, or `Ok(None)`
    /// if no new mail was found.
    pub async fn fetch_new_mail(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        end_uid: Option<u32>,
        before: Option<&str>,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        assert!(start_uid > 0, "start_uid must be greater than 0");

        session
            .examine(&mailbox.encoded_name())
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        match before {
            Some(date) => {
                Self::fetch_new_mail_with_before(session, account, mailbox, start_uid, date, token)
                    .await
            }
            None => {
                Self::fetch_new_mail_range(session, account, mailbox, start_uid, end_uid, token)
                    .await
            }
        }
    }

    /// Two-step approach for date-filtered incremental fetch: UID SEARCH first,
    /// then batch UID FETCH for matching UIDs. Uses standard IMAP syntax that
    /// works across all compliant servers.
    async fn fetch_new_mail_with_before(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        date: &str,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        let query = format!("UID {start_uid}:* BEFORE {date}");
        info!(
            "[account {}][mailbox {}] fetch_new_mail: UID SEARCH {}",
            account.id, mailbox.name, query
        );
        let results = session.uid_search(&query).await.map_err(|e| {
            let err_msg = format!("UID SEARCH failed in [{}]: {:#?}", mailbox.name, e);
            let _ = DownloadState::append_session_error(account.id, err_msg);
            raise_error!(format!("{:#?}", e), classify_imap_error(&e))
        })?;

        if results.is_empty() {
            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Success,
                Some("No new emails found.".into()),
            )?;
            return Ok(None);
        }

        let mut uid_vec: Vec<u32> = results.into_iter().collect();
        uid_vec.sort();
        let max_uid = uid_vec.last().copied();
        let planned = uid_vec.len() as u64;
        let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
        let uid_batches = generate_uid_sequence_hashset(uid_vec, batch_size);

        DownloadState::update_folder_progress(
            account.id,
            mailbox.name.clone(),
            planned,
            0,
            FolderStatus::Pending,
            None,
        )?;

        let mut count = 0u64;
        for batch in uid_batches {
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("User stopped or system shutdown".to_string()),
                )?;
                DownloadState::update_folder_progress(
                    account.id,
                    mailbox.name.clone(),
                    planned,
                    count,
                    FolderStatus::Cancelled,
                    None,
                )?;
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            let processed = Self::uid_batch_retrieve_emails(
                session,
                account.id,
                mailbox.id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
            )
            .await?;
            count += processed;
            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                planned,
                count,
                FolderStatus::Downloading,
                None,
            )?;
        }

        DownloadState::update_folder_progress(
            account.id,
            mailbox.name.clone(),
            count,
            count,
            FolderStatus::Success,
            None,
        )?;

        Ok(max_uid)
    }

    /// Ranged UID FETCH without date filtering. If `end_uid` is known, split the
    /// range into bounded batches so large backlogs cannot occupy one long-lived
    /// IMAP stream. If `end_uid` is unavailable, preserve the legacy unbounded
    /// streaming behavior.
    async fn fetch_new_mail_range(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        end_uid: Option<u32>,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        if let Some(end_uid) = end_uid {
            if start_uid > end_uid as u64 {
                DownloadState::update_folder_progress(
                    account.id,
                    mailbox.name.clone(),
                    0,
                    0,
                    FolderStatus::Success,
                    Some("No new emails found.".into()),
                )?;
                return Ok(None);
            }

            let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
            let uid_batches = generate_uid_range_batches(start_uid, end_uid, batch_size);
            let planned = uid_batches.iter().map(|(_, count, _)| *count).sum();

            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                planned,
                0,
                FolderStatus::Pending,
                None,
            )?;

            let mut count = 0u64;
            let mut max_uid: Option<u32> = None;
            for (uid_range, _, batch_end) in uid_batches {
                if token.is_cancelled() {
                    tracing::info!("Account {}: fetch_new_mail batch interrupted.", account.id);
                    DownloadState::update_session_status(
                        account.id,
                        DownloadStatus::Cancelled,
                        Some("User stopped or system shutdown".to_string()),
                    )?;
                    DownloadState::update_folder_progress(
                        account.id,
                        mailbox.name.clone(),
                        planned,
                        count,
                        FolderStatus::Cancelled,
                        None,
                    )?;
                    return Err(raise_error!(
                        "Stream cancelled".into(),
                        ErrorCode::InternalError
                    ));
                }

                info!(
                    "[account {}][mailbox {}] fetch_new_mail: batched UID FETCH {}",
                    account.id, mailbox.name, uid_range
                );
                let processed = Self::uid_batch_retrieve_emails(
                    session,
                    account.id,
                    mailbox.id,
                    &uid_range,
                    account.max_email_size_bytes,
                    token.clone(),
                )
                .await?;
                count += processed;
                if processed > 0 {
                    max_uid = Some(batch_end);
                }
                DownloadState::update_folder_progress(
                    account.id,
                    mailbox.name.clone(),
                    planned,
                    count,
                    FolderStatus::Downloading,
                    None,
                )?;
            }

            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                count,
                count,
                FolderStatus::Success,
                None,
            )?;

            return Ok(max_uid);
        }

        let uid_range = format!("{start_uid}:*");
        info!(
            "[account {}][mailbox {}] fetch_new_mail: direct UID FETCH {}",
            account.id, mailbox.name, uid_range
        );

        let mut stream = session
            .uid_fetch(&uid_range, BODY_FETCH_COMMAND)
            .await
            .map_err(|e| {
                let err_msg = format!("UID FETCH failed in [{}]: {:#?}", mailbox.name, e);
                let _ = DownloadState::append_session_error(account.id, err_msg);
                raise_error!(format!("{:#?}", e), classify_imap_error(&e))
            })?;

        let mut count = 0u64;
        let mut skipped = 0u64;
        let mut max_uid: Option<u32> = None;
        let size_limit = account
            .max_email_size_bytes
            .unwrap_or(DEFAULT_MAX_EMAIL_SIZE);
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
        {
            if token.is_cancelled() {
                tracing::info!("Account {}: fetch_new_mail stream interrupted.", account.id);
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("User stopped or system shutdown".to_string()),
                )?;
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }

            let msg_size = fetch.size.unwrap_or(0) as u64;
            if msg_size > 0 && msg_size > size_limit {
                tracing::warn!(
                    account_id = account.id,
                    mailbox_id = mailbox.id,
                    uid = fetch.uid,
                    size = msg_size,
                    limit = size_limit,
                    "Skipping oversized email (streaming mode)"
                );
                skipped += 1;
                continue;
            }

            if let Some(uid) = fetch.uid {
                max_uid = Some(max_uid.unwrap_or(0).max(uid));
            }
            extract_envelope_and_store_it(fetch, account.id, mailbox.id).await?;
            count += 1;
        }

        let total = count + skipped;
        if total == 0 {
            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Success,
                Some("No new emails found.".into()),
            )?;
        } else {
            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                total,
                count,
                FolderStatus::Success,
                if skipped > 0 {
                    Some(format!("{skipped} email(s) skipped due to size limit"))
                } else {
                    None
                },
            )?;
        }

        Ok(max_uid)
    }

    pub async fn batch_retrieve_emails(
        session: &mut Session<Box<dyn SessionStream>>,
        account_id: u64,
        mailbox_id: u64,
        total: u64,
        page: u64,
        page_size: u64,
        encoded_mailbox_name: &str,
        max_email_size_bytes: Option<u64>,
        token: CancellationToken,
        max_uid: &mut Option<u32>,
    ) -> BichonResult<usize> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");

        // Fetch messages starting from the oldest (ascending order).
        let start = (page - 1) * page_size + 1;
        if start > total {
            return Ok(0);
        }
        let end = (start + page_size - 1).min(total);

        let sequence_set = format!("{}:{}", start, end);
        info!(
            "Fetching mailbox '{}' messages: sequence {} (page {}, page_size {})",
            encoded_mailbox_name, sequence_set, page, page_size
        );

        let limit = max_email_size_bytes.unwrap_or(DEFAULT_MAX_EMAIL_SIZE);

        // PASS 1: fetch only SIZE to identify oversized messages
        let acceptable_uids = {
            let mut size_stream = session
                .fetch(sequence_set.as_str(), SIZE_ONLY_FETCH)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

            let mut uids: Vec<u32> = Vec::new();
            while let Some(fetch) = size_stream
                .try_next()
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
            {
                let uid = fetch.uid.unwrap_or(0);
                let msg_size = fetch.size.unwrap_or(0) as u64;
                if msg_size == 0 || msg_size <= limit {
                    uids.push(uid);
                } else {
                    tracing::warn!(
                        account_id,
                        mailbox_id,
                        uid,
                        size = msg_size,
                        limit,
                        "Skipping oversized email"
                    );
                }
            }
            uids
        };

        if acceptable_uids.is_empty() {
            return Ok(0);
        }

        // PASS 2: fetch bodies only for acceptable UIDs
        let filtered = compress_uid_list(acceptable_uids);
        let mut body_stream = session
            .uid_fetch(&filtered, BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut count = 0;
        while let Some(fetch) = body_stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
        {
            if token.is_cancelled() {
                tracing::info!("Account {}: UID fetch stream interrupted.", account_id);
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            if let Some(uid) = fetch.uid {
                *max_uid = Some((*max_uid).unwrap_or(0).max(uid));
            }
            extract_envelope_and_store_it(fetch, account_id, mailbox_id).await?;
            count += 1;
        }
        Ok(count)
    }

    pub async fn uid_batch_retrieve_emails(
        session: &mut Session<Box<dyn SessionStream>>,
        account_id: u64,
        mailbox_id: u64,
        uid_set: &str,
        max_email_size_bytes: Option<u64>,
        token: CancellationToken,
    ) -> BichonResult<u64> {
        let limit = max_email_size_bytes.unwrap_or(DEFAULT_MAX_EMAIL_SIZE);

        // PASS 1: fetch only SIZE to identify oversized messages
        let acceptable_uids = {
            let mut size_stream = session
                .uid_fetch(uid_set, SIZE_ONLY_FETCH)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

            let mut uids: Vec<u32> = Vec::new();
            while let Some(fetch) = size_stream
                .try_next()
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
            {
                let uid = fetch.uid.unwrap_or(0);
                let msg_size = fetch.size.unwrap_or(0) as u64;
                if msg_size == 0 || msg_size <= limit {
                    uids.push(uid);
                } else {
                    tracing::warn!(
                        account_id,
                        mailbox_id,
                        uid,
                        size = msg_size,
                        limit,
                        "Skipping oversized email"
                    );
                }
            }
            uids
        };

        if acceptable_uids.is_empty() {
            return Ok(0);
        }

        // PASS 2: fetch bodies only for acceptable UIDs
        let filtered = compress_uid_list(acceptable_uids);
        let mut body_stream = session
            .uid_fetch(&filtered, BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut count = 0u64;
        while let Some(fetch) = body_stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
        {
            if token.is_cancelled() {
                tracing::info!("Account {}: UID fetch stream interrupted.", account_id);
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            extract_envelope_and_store_it(fetch, account_id, mailbox_id).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Fetches the raw RFC822 body of a single message by UID.
    ///
    /// Selects (read-only) the given mailbox and issues `UID FETCH <uid> (BODY.PEEK[])`.
    /// Used for on-demand self-healing when an indexed message's content blob is missing.
    /// Returns the raw bytes, or an error if the message cannot be retrieved.
    pub async fn fetch_single_message_body(
        session: &mut Session<Box<dyn SessionStream>>,
        encoded_mailbox_name: &str,
        uid: u32,
    ) -> BichonResult<Vec<u8>> {
        session
            .examine(encoded_mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut stream = session
            .uid_fetch(uid.to_string(), BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let fetch = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
            .ok_or_else(|| {
                raise_error!(
                    format!("UID {uid} not found on IMAP server"),
                    ErrorCode::ResourceNotFound
                )
            })?;

        let body = fetch
            .body()
            .ok_or_else(|| {
                raise_error!(
                    format!("No body returned for UID {uid}"),
                    ErrorCode::ImapUnexpectedResult
                )
            })?
            .to_vec();

        // // Drain any remaining items so the stream is fully consumed before reuse.
        // while stream
        //     .try_next()
        //     .await
        //     .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
        //     .is_some()
        // {}

        Ok(body)
    }

    pub async fn create_connection(
        account_id: u64,
    ) -> BichonResult<Session<Box<dyn SessionStream>>> {
        ImapConnectionManager::build(account_id).await
    }

    /// Fetch UID → Message-ID mapping without downloading bodies.
    /// `uid_set` is an IMAP sequence-set string (e.g. "1:100" or "1,3,5").
    pub async fn fetch_uid_metadata(
        session: &mut Session<Box<dyn SessionStream>>,
        uid_set: &str,
        token: CancellationToken,
    ) -> BichonResult<HashMap<u32, Option<String>>> {
        let mut stream = session
            .uid_fetch(uid_set, "(UID BODY.PEEK[HEADER])")
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut result = HashMap::new();
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?
        {
            if token.is_cancelled() {
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            let uid = fetch.uid.unwrap_or(0);
            let msg_id = fetch.header().and_then(parse_message_id_header);
            result.insert(uid, msg_id);
        }
        Ok(result)
    }
}

pub const DEFAULT_BATCH_SIZE: u32 = 30;
pub const DEFAULT_MAX_EMAIL_SIZE: u64 = 100 * 1024 * 1024;

/// Compresses a sorted list of UIDs into an IMAP sequence-set string.
/// Consecutive UIDs become ranges (e.g. `1:5`), non-consecutive are
/// comma-separated (e.g. `1:5,10,12:15`).
pub fn compress_uid_list(nums: Vec<u32>) -> String {
    if nums.is_empty() {
        return String::new();
    }

    let mut sorted_nums = nums;
    sorted_nums.sort();

    let mut result = Vec::new();
    let mut current_range_start = sorted_nums[0];
    let mut current_range_end = sorted_nums[0];

    for &n in sorted_nums.iter().skip(1) {
        if n == current_range_end + 1 {
            current_range_end = n;
        } else {
            if current_range_start == current_range_end {
                result.push(current_range_start.to_string());
            } else {
                result.push(format!("{}:{}", current_range_start, current_range_end));
            }
            current_range_start = n;
            current_range_end = n;
        }
    }

    if current_range_start == current_range_end {
        result.push(current_range_start.to_string());
    } else {
        result.push(format!("{}:{}", current_range_start, current_range_end));
    }

    result.join(",")
}

/// Splits a sorted list of unique UIDs into compressed sequence-set batches.
/// Returns `Vec<(sequence_set_string, batch_count)>`.
pub fn generate_uid_sequence_hashset(
    unique_nums: Vec<u32>,
    chunk_size: usize,
) -> Vec<(String, u64)> {
    assert!(!unique_nums.is_empty());

    let mut result = Vec::new();
    let nums = unique_nums;

    for chunk in nums.chunks(chunk_size) {
        let size = chunk.len() as u64;
        let compressed = compress_uid_list(chunk.to_vec());
        result.push((compressed, size));
    }

    result
}

/// Splits a bounded inclusive UID range into IMAP sequence-set batches.
/// Returns `Vec<(sequence_set_string, batch_count, batch_end_uid)>`.
fn generate_uid_range_batches(
    start_uid: u64,
    end_uid: u32,
    chunk_size: usize,
) -> Vec<(String, u64, u32)> {
    assert!(chunk_size > 0);

    if start_uid == 0 || start_uid > end_uid as u64 || start_uid > u32::MAX as u64 {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut start = start_uid as u32;

    while start <= end_uid {
        let end = start
            .saturating_add(chunk_size as u32)
            .saturating_sub(1)
            .min(end_uid);
        let count = end as u64 - start as u64 + 1;
        let sequence_set = if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        };
        result.push((sequence_set, count, end));

        if end == u32::MAX {
            break;
        }
        start = end + 1;
    }

    result
}

fn parse_message_id_header(header_bytes: &[u8]) -> Option<String> {
    let header = std::str::from_utf8(header_bytes).ok()?;
    for line in header.lines() {
        if let Some(value) = line
            .strip_prefix("Message-ID:")
            .or_else(|| line.strip_prefix("Message-Id:"))
            .or_else(|| line.strip_prefix("Message-id:"))
        {
            // mail_parser strips angle brackets, so we must do the same
            // to ensure comparisons against the Tantivy index match.
            let trimmed = value.trim();
            let stripped = trimmed.strip_prefix('<').unwrap_or(trimmed);
            let stripped = stripped.strip_suffix('>').unwrap_or(stripped);
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod test {
    use super::*;

    // ── compress_uid_list ──────────────────────────────────────────

    #[test]
    fn compress_empty() {
        assert_eq!(compress_uid_list(vec![]), "");
    }

    #[test]
    fn compress_single_uid() {
        assert_eq!(compress_uid_list(vec![42]), "42");
    }

    #[test]
    fn compress_consecutive_range() {
        assert_eq!(compress_uid_list(vec![1, 2, 3, 4, 5]), "1:5");
    }

    #[test]
    fn compress_mixed_ranges() {
        assert_eq!(
            compress_uid_list(vec![1, 2, 3, 5, 7, 8, 9, 10]),
            "1:3,5,7:10"
        );
    }

    #[test]
    fn compress_gap_at_boundary() {
        assert_eq!(compress_uid_list(vec![1, 2, 4, 5]), "1:2,4:5");
    }

    // ── generate_uid_sequence_hashset ──────────────────────────────

    #[test]
    fn batch_single_chunk() {
        let batches = generate_uid_sequence_hashset(vec![1, 2, 3], 10);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].0, "1:3");
        assert_eq!(batches[0].1, 3);
    }

    #[test]
    fn batch_multiple_chunks() {
        let batches = generate_uid_sequence_hashset(vec![1, 2, 3, 4, 5], 2);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].0, "1:2");
        assert_eq!(batches[0].1, 2);
        assert_eq!(batches[1].0, "3:4");
        assert_eq!(batches[1].1, 2);
        assert_eq!(batches[2].0, "5");
        assert_eq!(batches[2].1, 1);
    }

    #[test]
    fn uid_range_batches_do_not_use_unbounded_fetch() {
        let batches = generate_uid_range_batches(195_225, 195_426, 100);

        assert_eq!(
            batches,
            vec![
                ("195225:195324".to_string(), 100, 195_324),
                ("195325:195424".to_string(), 100, 195_424),
                ("195425:195426".to_string(), 2, 195_426),
            ]
        );
        assert!(batches
            .iter()
            .all(|(sequence_set, _, _)| !sequence_set.contains('*')));
    }

    // ── parse_message_id_header ─────────────────────────────────────

    #[test]
    fn parse_standard_message_id() {
        let header = b"Message-ID: <abc123@example.com>\r\n";
        assert_eq!(
            parse_message_id_header(header),
            Some("abc123@example.com".into())
        );
    }

    #[test]
    fn parse_message_id_lowercase() {
        let header = b"Message-Id: <foo@bar.com>\r\n";
        assert_eq!(parse_message_id_header(header), Some("foo@bar.com".into()));
    }

    #[test]
    fn parse_message_id_extra_whitespace() {
        let header = b"Message-ID:   <spaces@test.com>  \r\n";
        assert_eq!(
            parse_message_id_header(header),
            Some("spaces@test.com".into())
        );
    }

    #[test]
    fn parse_empty_message_id_returns_none() {
        let header = b"Message-ID: <>\r\n";
        assert_eq!(parse_message_id_header(header), None);
    }

    #[test]
    fn parse_missing_header_returns_none() {
        let header = b"X-Custom: something\r\n";
        assert_eq!(parse_message_id_header(header), None);
    }

    #[test]
    fn parse_empty_body_returns_none() {
        assert_eq!(parse_message_id_header(b""), None);
    }

    #[test]
    fn parse_message_id_in_full_header() {
        // The Message-ID line is in the middle, not at the start.
        let header = b"From: sender@example.com\r\n\
Date: Thu, 01 Jan 2025 00:00:00 +0000\r\n\
Subject: test\r\n\
Message-ID: <mid@example.com>\r\n\
To: recipient@example.com\r\n\r\n";
        assert_eq!(
            parse_message_id_header(header),
            Some("mid@example.com".into())
        );
    }

    #[test]
    fn parse_message_id_only_in_full_header() {
        // Only a few headers, Message-ID is among them.
        let header = b"From: a@b.com\r\nMessage-ID: <x@y.com>\r\n\r\n";
        assert_eq!(parse_message_id_header(header), Some("x@y.com".into()));
    }

    #[test]
    fn parse_message_id_no_brackets_still_works() {
        let header = b"Message-ID: plain@example.com\r\n";
        assert_eq!(
            parse_message_id_header(header),
            Some("plain@example.com".into())
        );
    }
}
