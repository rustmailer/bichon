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
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::{error::BichonResult, imap::manager::ImapConnectionManager};
use async_imap::types::Name;
use async_imap::Session;
use futures::TryStreamExt;
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;
use tracing::info;

const BODY_FETCH_COMMAND: &str = "(UID INTERNALDATE RFC822.SIZE BODY.PEEK[])";
const SIZE_ONLY_FETCH: &str = "(UID RFC822.SIZE)";
const MAX_NETWORK_RETRIES: u32 = 3;

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

    /// Enumerate every UID currently present in the mailbox via `UID SEARCH ALL`.
    ///
    /// This is the drift-safe way to page through a full mailbox: UIDs are stable
    /// while the download runs (new arrivals only get larger UIDs), whereas
    /// sequence numbers shift when mail is added or removed mid-download, which
    /// silently skips messages. The caller owns the returned list and decides
    /// how to batch it (see `generate_uid_sequence_hashset`).
    pub async fn uid_search_all_mailbox(
        session: &mut Session<Box<dyn SessionStream>>,
        mailbox_name: &str,
    ) -> BichonResult<Vec<u32>> {
        session
            .examine(mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        let results = session
            .uid_search("ALL")
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;
        let mut uids: Vec<u32> = results.into_iter().collect();
        uids.sort();
        Ok(uids)
    }

    /// Fetches new mail for a mailbox.
    ///
    /// UIDs are enumerated via a ranged `UID FETCH {start}:* (UID RFC822.SIZE
    /// INTERNALDATE)` (RFC 3501 §6.4.4 closed-interval semantics — unlike
    /// `UID SEARCH`, which servers may answer with a subset, a truncated
    /// enumeration cannot silently skip messages), then bodies are downloaded
    /// in batches. When `before` is `Some(date)`, the INTERNALDATE is compared
    /// against the date client-side (equivalent to SEARCH's BEFORE key).
    ///
    /// Returns `Ok(Some(max_uid))` with the highest UID fetched, or `Ok(None)`
    /// if no new mail was found.
    pub async fn fetch_new_mail(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        before: Option<&str>,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        assert!(start_uid > 0, "start_uid must be greater than 0");

        let examined = session
            .examine(&mailbox.encoded_name())
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        match before {
            Some(date) => {
                Self::fetch_new_mail_with_before(
                    session,
                    account,
                    mailbox,
                    start_uid,
                    date,
                    examined,
                    token,
                )
                .await
            }
            None => {
                Self::fetch_new_mail_range(session, account, mailbox, start_uid, examined, token)
                    .await
            }
        }
    }

    /// Date-filtered incremental fetch: enumerate UIDs in `{start}:*` via
    /// ranged `UID FETCH` (RFC 3501 §6.4.4 closed-interval semantics, so a
    /// truncated result cannot silently skip messages), then filter by
    /// INTERNALDATE client-side and batch-download the bodies.
    ///
    /// The `BEFORE {date}` filter is applied locally: RFC 3501's BEFORE key
    /// matches messages whose internal date (ignoring time and timezone) is
    /// earlier than the given date, which is exactly the same comparison done
    /// here on the INTERNALDATE returned by the enumeration fetch.
    async fn fetch_new_mail_with_before(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        date: &str,
        examined: async_imap::types::Mailbox,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        let uid_range = format!("{start_uid}:*");
        let (entries, skipped_oversized) = Self::collect_range_uids(
            session,
            &uid_range,
            account.id,
            mailbox,
            account.max_email_size_bytes,
            token.clone(),
        )
        .await?;
        let mut uid_vec = filter_before_date(&entries, date)?;
        // Same non-compliant-server guard as fetch_new_mail_range: `{start}:*`
        // may be clamped by the server and return uids below start_uid, which
        // are already stored locally (or are drift — gap-fill's job).
        uid_vec.retain(|&uid| (uid as u64) >= start_uid);
        info!(
            account_id = account.id,
            mailbox = %mailbox.name,
            start_uid,
            date,
            found = uid_vec.len(),
            first = uid_vec.first().copied(),
            last = uid_vec.last().copied(),
            skipped_oversized,
            "fetch_new_mail_with_before: UID FETCH result"
        );

        if uid_vec.is_empty() {
            // Same truncated-result guard as fetch_new_mail_range: if the
            // server claims new mail but nothing passed the filters, refuse to
            // advance highest_uid. A legitimate empty result here is: mail
            // existed but was all oversized (skipped_oversized > 0), or all
            // entries fell outside the date window (entries non-empty).
            if skipped_oversized == 0 && entries.is_empty() {
                if let Some(msg) = empty_enumeration_anomaly(
                    mailbox.name.as_str(),
                    &uid_range,
                    start_uid,
                    examined.uid_next,
                ) {
                    return resolve_empty_enumeration(account, mailbox, &examined, start_uid, msg)
                        .await;
                }
            }
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
            let (processed, throttled) = Self::uid_batch_retrieve_emails(
                session,
                account.id,
                mailbox.id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
                Some(&|cumulative, avg_secs, stall_secs| {
                    // Per-message progress: the current batch's cumulative count
                    // keeps the UI moving while a slow server trickles messages.
                    let _ = DownloadState::update_folder_progress(
                        account.id,
                        mailbox.name.clone(),
                        planned,
                        cumulative,
                        FolderStatus::Downloading,
                        slow_server_message(avg_secs, stall_secs),
                    );
                    Ok(())
                }),
            )
            .await?;
            if throttled {
                // Server appears to be rate-limiting; back off before the next
                // batch so we don't hammer the limiter with back-to-back bursts.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
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

    /// Enumerates every UID in `{start}:*` via a lightweight
    /// `UID FETCH {start}:* (UID RFC822.SIZE INTERNALDATE)` — no bodies.
    ///
    /// Unlike `UID SEARCH`, a ranged UID FETCH is required by RFC 3501 §6.4.4
    /// to return the full closed interval, so a truncated response cannot
    /// silently skip messages in the middle of the range.
    ///
    /// UIDs whose RFC822.SIZE exceeds `max_email_size_bytes` are dropped here
    /// (they would be skipped again by the batched body fetch's SIZE pre-check,
    /// so filtering up front saves a round trip per batch). Returns the
    /// accepted `(uid, size, internal_date_epoch_millis)` entries and the
    /// number of oversized messages skipped.
    async fn collect_range_uids(
        session: &mut Session<Box<dyn SessionStream>>,
        uid_range: &str,
        account_id: u64,
        mailbox: &MailBox,
        max_email_size_bytes: Option<u64>,
        token: CancellationToken,
    ) -> BichonResult<(Vec<(u32, u64, i64)>, u64)> {
        let limit = max_email_size_bytes.unwrap_or(DEFAULT_MAX_EMAIL_SIZE);
        let mut uid_stream = session
            .uid_fetch(uid_range, "(UID RFC822.SIZE INTERNALDATE)")
            .await
            .map_err(|e| {
                let err_msg = format!("UID FETCH failed in [{}]: {:#?}", mailbox.name, e);
                let _ = DownloadState::append_session_error(account_id, err_msg);
                raise_error!(format!("{:#?}", e), classify_imap_error(&e))
            })?;
        let mut entries: Vec<(u32, u64, i64)> = Vec::new();
        let mut skipped_oversized = 0u64;
        while let Some(fetch) = uid_stream
            .try_next()
            .await
            .map_err(|e| {
                let err_msg = format!("UID FETCH stream failed in [{}]: {:#?}", mailbox.name, e);
                let _ = DownloadState::append_session_error(account_id, err_msg);
                raise_error!(format!("{:#?}", e), classify_imap_error(&e))
            })?
        {
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account_id,
                    DownloadStatus::Cancelled,
                    Some("User stopped or system shutdown".to_string()),
                )?;
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            let Some(uid) = fetch.uid else {
                continue;
            };
            let size = fetch.size.unwrap_or(0) as u64;
            let internal_date = fetch
                .internal_date()
                .map(|d| d.timestamp_millis())
                .unwrap_or(0);
            if size == 0 || size <= limit {
                entries.push((uid, size, internal_date));
            } else {
                skipped_oversized += 1;
                tracing::warn!(
                    account_id,
                    mailbox_id = mailbox.id,
                    uid,
                    size,
                    limit,
                    "Skipping oversized email during UID enumeration"
                );
            }
        }
        Ok((entries, skipped_oversized))
    }

    /// Fetches all messages with UID >= start_uid via batched UID FETCH.
    ///
    /// A single ranged `UID FETCH {start}:* (BODY[])` can block for minutes on
    /// slow servers pushing hundreds of messages, and hits the socket read
    /// timeout if the server stalls, with zero progress feedback in the
    /// meantime. Instead, enumerate the UIDs first via a lightweight
    /// `UID FETCH {start}:* (UID RFC822.SIZE)` (headers/size only, no bodies),
    /// then download in small batches — each batch is a short round-trip with
    /// a SIZE pre-check (oversized messages are skipped without fetching their
    /// body), progress is reported per batch, and the whole download stays
    /// responsive to cancellation.
    ///
    /// A plain `UID SEARCH {start}:*` is NOT used to enumerate: RFC 3501
    /// grants SEARCH the freedom to return a subset or non-normalized results,
    /// and servers in the wild (e.g. Gmail) occasionally return only the last
    /// matching UID for a huge range. Since the caller advances highest_uid to
    /// the last UID found, a truncated SEARCH permanently skips everything
    /// between start_uid and that last UID. UID FETCH on a range, by contrast,
    /// is REQUIRED by RFC 3501 §6.4.4 to return the full closed interval
    /// [start_uid, max UID].
    async fn fetch_new_mail_range(
        session: &mut Session<Box<dyn SessionStream>>,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        examined: async_imap::types::Mailbox,
        token: CancellationToken,
    ) -> BichonResult<Option<u32>> {
        let uid_range = format!("{start_uid}:*");
        info!(
            "[account {}][mailbox {}] fetch_new_mail: enumerate UIDs via UID FETCH {}",
            account.id, mailbox.name, uid_range
        );

        // Track how many messages were dropped by the size filter so an empty
        // result is not misread as an enumeration failure (anomaly guard).
        let (entries, skipped_oversized) = Self::collect_range_uids(
            session,
            &uid_range,
            account.id,
            mailbox,
            account.max_email_size_bytes,
            token.clone(),
        )
        .await?;
        let mut uid_vec: Vec<u32> = entries.iter().map(|&(uid, _, _)| uid).collect();
        uid_vec.sort();
        // Some non-compliant servers (e.g. Zoho) interpret `{start}:*` as a
        // sequence range and clamp it, returning the last message even when
        // start_uid exceeds the highest UID. Such results are below start_uid
        // and are already stored locally (or are drift — gap-fill's job), so
        // filter them out to avoid re-downloading the same email every sync.
        uid_vec.retain(|&uid| (uid as u64) >= start_uid);
        info!(
            account_id = account.id,
            mailbox = %mailbox.name,
            start_uid,
            found = uid_vec.len(),
            first = uid_vec.first().copied(),
            last = uid_vec.last().copied(),
            skipped_oversized,
            "fetch_new_mail_range: UID FETCH result"
        );

        if uid_vec.is_empty() {
            // Guard against a truncated (or silently dropped) FETCH result:
            // if the server reports a UIDNEXT well above start_uid yet no UIDs
            // came back, do NOT advance highest_uid past start_uid — that would
            // permanently skip everything in between. Report the anomaly and
            // keep the old highest_uid so the next sync retries.
            //
            // Oversized-only mail is legitimate (nothing to download within the
            // size limit), so skip the anomaly check when the size filter (not
            // the server) is what emptied the range.
            if skipped_oversized == 0 {
                if let Some(msg) = empty_enumeration_anomaly(
                    mailbox.name.as_str(),
                    &uid_range,
                    start_uid,
                    examined.uid_next,
                ) {
                    return resolve_empty_enumeration(account, mailbox, &examined, start_uid, msg)
                        .await;
                }
            }
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

        let max_uid = uid_vec.last().copied();
        let planned = uid_vec.len() as u64;
        let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
        let batches = generate_uid_sequence_hashset(uid_vec, batch_size);
        let total_batches = batches.len();

        DownloadState::update_folder_progress(
            account.id,
            mailbox.name.clone(),
            planned,
            0,
            FolderStatus::Downloading,
            None,
        )?;

        let mut count = 0u64;
        for (index, batch) in batches.into_iter().enumerate() {
            if token.is_cancelled() {
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

            // A slow server can stall a batch past the socket read timeout.
            // Retry such batches on a fresh connection instead of failing the
            // whole sync session.
            let mut retries = 0u32;
            let batch_result = loop {
                match Self::uid_batch_retrieve_emails(
                    session,
                    account.id,
                    mailbox.id,
                    &batch.0,
                    account.max_email_size_bytes,
                    token.clone(),
                    Some(&|cumulative, avg_secs, stall_secs| {
                        // Report per-message so the UI moves even while a slow
                        // server trickles out the current batch.
                        DownloadState::update_folder_progress(
                            account.id,
                            mailbox.name.clone(),
                            planned,
                            cumulative,
                            FolderStatus::Downloading,
                            slow_server_message(avg_secs, stall_secs),
                        )
                    }),
                )
                .await
                {
                    Ok(processed) => break Ok(processed),
                    Err(e)
                        if retries < MAX_NETWORK_RETRIES && e.code() == ErrorCode::NetworkError =>
                    {
                        retries += 1;
                        tracing::warn!(
                            account_id = account.id,
                            mailbox = mailbox.name,
                            index,
                            retries,
                            "Network error on batch, reconnecting ({}/{})",
                            retries,
                            MAX_NETWORK_RETRIES
                        );
                        match ImapExecutor::create_connection(account.id).await {
                            Ok(new_session) => {
                                *session = new_session;
                                if let Err(e2) = session.examine(&mailbox.encoded_name()).await {
                                    let err_msg =
                                        format!("Re-examine failed after reconnect: {:#?}", e2);
                                    DownloadState::append_session_error(account.id, err_msg)?;
                                    break Err(e);
                                }
                                // Longer backoff than the original 1s/2s/4s: a
                                // throttling server needs time to recover.
                                let backoff = [5u64, 15, 30][(retries - 1) as usize];
                                tracing::warn!(
                                    account_id = account.id,
                                    mailbox = mailbox.name,
                                    "Backing off {}s before retrying batch",
                                    backoff
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                                continue;
                            }
                            Err(e2) => {
                                tracing::error!(
                                    account_id = account.id,
                                    "Reconnection failed: {:#?}",
                                    e2
                                );
                                break Err(e);
                            }
                        }
                    }
                    Err(e) => break Err(e),
                }
            };
            match batch_result {
                Ok((processed, _throttled)) => {
                    count += processed;
                    DownloadState::update_folder_progress(
                        account.id,
                        mailbox.name.clone(),
                        planned,
                        count,
                        FolderStatus::Downloading,
                        None,
                    )?;
                    tracing::debug!(
                        "[account {}][mailbox {}] fetch_new_mail: batch {}/{} done ({} processed)",
                        account.id,
                        mailbox.name,
                        index + 1,
                        total_batches,
                        processed
                    );
                }
                Err(e) => {
                    DownloadState::append_session_error(account.id, format!("{:#?}", e))?;
                    return Err(e);
                }
            }
            if count == planned {
                break;
            }
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

    /// Downloads the bodies of `uid_set` in one batch.
    ///
    /// `progress` (if given) is invoked after each stored message with the
    /// cumulative count for the whole mailbox, the current average
    /// inter-message interval in seconds (None until at least two messages
    /// arrived), and the current stall duration in seconds when the server is
    /// silent (None when a message just arrived). The UI can then move per
    /// message — and detect a slow server — instead of only updating when a
    /// batch finishes. Slow servers push a batch's messages over many seconds
    /// (even minutes); without per-message updates the UI freezes and looks
    /// stuck.
    pub async fn uid_batch_retrieve_emails(
        session: &mut Session<Box<dyn SessionStream>>,
        account_id: u64,
        mailbox_id: u64,
        uid_set: &str,
        max_email_size_bytes: Option<u64>,
        token: CancellationToken,
        progress: Option<
            &(dyn Fn(u64, Option<f64>, Option<f64>) -> BichonResult<()> + Send + Sync),
        >,
    ) -> BichonResult<(u64, bool)> {
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
            return Ok((0, false));
        }

        // PASS 2: fetch bodies only for acceptable UIDs
        let filtered = compress_uid_list(acceptable_uids);
        let mut body_stream = session
            .uid_fetch(&filtered, BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut count = 0u64;
        // Sliding window of recent per-message receive times, used to estimate
        // how slow the server is: slow servers push messages seconds apart.
        let mut recv_times: std::collections::VecDeque<std::time::Instant> =
            std::collections::VecDeque::with_capacity(11);
        let mut last_recv = std::time::Instant::now();
        // Consecutive stall reports. A high value means the server is
        // throttling us; the caller sleeps before the next batch to avoid
        // hammering the rate limiter back-to-back.
        let mut consecutive_stalls = 0u32;
        // While the server is silent, re-report the current wait every few
        // seconds so the UI shows the wait climbing instead of a stale
        // average (the average only moves when a message actually arrives).
        const STALL_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        loop {
            // On error, report the number of emails already stored through the
            // progress callback so a partial batch is not counted as fully
            // failed by callers (gap_fill uses the last reported count).
            let item = match tokio::time::timeout(STALL_REPORT_INTERVAL, body_stream.try_next()).await
            {
                Ok(Ok(item)) => Ok(item),
                Ok(Err(e)) => Err((count, e)),
                Err(_) => {
                    if token.is_cancelled() {
                        if let Some(progress) = progress {
                            let _ = progress(count, None, None);
                        }
                        return Err(raise_error!(
                            "Stream cancelled".into(),
                            ErrorCode::InternalError
                        ));
                    }
                    let stall_secs = last_recv.elapsed().as_secs_f64();
                    consecutive_stalls += 1;

                    if let Some(progress) = progress {
                        progress(count, None, Some(stall_secs))?;
                    }
                    continue;
                }
            };
            let item = match item {
                Ok(item) => item,
                Err((processed, e)) => {
                    if let Some(progress) = progress {
                        let _ = progress(processed, None, None);
                    }
                    return Err(raise_error!(
                        format!("{:#?}", e),
                        classify_imap_error(&e)
                    ));
                }
            };
            let Some(fetch) = item else { break };

            if token.is_cancelled() {
                tracing::info!("Account {}: UID fetch stream interrupted.", account_id);
                if let Some(progress) = progress {
                    let _ = progress(count, None, None);
                }
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            let now = std::time::Instant::now();
            consecutive_stalls = 0;
            last_recv = now;
            recv_times.push_back(now);
            if recv_times.len() > 10 {
                recv_times.pop_front();
            }
            let avg_secs = if recv_times.len() >= 2 {
                let span = recv_times
                    .back()
                    .unwrap()
                    .duration_since(*recv_times.front().unwrap());
                Some(span.as_secs_f64() / (recv_times.len() as f64 - 1.0))
            } else {
                None
            };
            if let Err(e) = extract_envelope_and_store_it(fetch, account_id, mailbox_id).await {
                if let Some(progress) = progress {
                    let _ = progress(count, None, None);
                }
                return Err(e);
            }
            count += 1;
            if let Some(progress) = progress {
                progress(count, avg_secs, None)?;
            }
        }
        Ok((count, consecutive_stalls >= 2))
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

    /// Fetch lightweight header metadata (UID, size, internal date, message-id)
    /// for a UID sequence-set, without downloading bodies. Used by gap-fill to
    /// build the remote side of the diff. The fetch deliberately asks only for
    /// the Message-ID header (no SUBJECT): parsing out a SUBJECT forces the
    /// server to decode the full header for every message, which some servers
    /// (e.g. Zoho) answer very slowly or in bursts. Message-ID alone is the
    /// primary match key; the fingerprint fallback for messages without one
    /// uses (size, internal date).
    ///
    /// `progress` (if given) is invoked with the number of headers received so
    /// far and the current stall duration in seconds (None while the server is
    /// feeding) every few seconds while the server is slow or silent, so
    /// callers can surface "server is slow / rate limiting" feedback instead
    /// of appearing stuck (mirrors `uid_batch_retrieve_emails`).
    pub async fn fetch_uid_headers(
        session: &mut Session<Box<dyn SessionStream>>,
        uid_set: &str,
        token: CancellationToken,
        progress: Option<
            &(dyn Fn(u64, Option<f64>) -> BichonResult<()> + Send + Sync),
        >,
    ) -> BichonResult<Vec<crate::cache::imap::download::gap_fill::RemoteHeader>> {
        let mut stream = session
            .uid_fetch(uid_set, "(UID RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])")
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e)))?;

        let mut result = Vec::new();
        let mut last_recv = std::time::Instant::now();
        // While the server is silent, re-report the current count every few
        // seconds so the UI shows the wait climbing instead of a stale state.
        const STALL_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        loop {
            let item =
                match tokio::time::timeout(STALL_REPORT_INTERVAL, stream.try_next()).await {
                    Ok(Ok(item)) => Ok(item),
                    Ok(Err(e)) => Err(raise_error!(
                        format!("{:#?}", e),
                        classify_imap_error(&e)
                    )),
                    Err(_) => {
                        if token.is_cancelled() {
                            return Err(raise_error!(
                                "Stream cancelled".into(),
                                ErrorCode::InternalError
                            ));
                        }
                        let stall_secs = last_recv.elapsed().as_secs_f64();
                        if let Some(progress) = progress {
                            progress(result.len() as u64, Some(stall_secs))?;
                        }
                        tracing::warn!(
                            stall_secs = format!("{:.0}", stall_secs),
                            fetched = result.len(),
                            "fetch_uid_headers: server silent, waiting"
                        );
                        continue;
                    }
                };
            let item = match item {
                Ok(item) => item,
                Err(e) => return Err(e),
            };
            let Some(fetch) = item else { break };
            last_recv = std::time::Instant::now();
            if token.is_cancelled() {
                return Err(raise_error!(
                    "Stream cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            let uid = fetch.uid.unwrap_or(0);
            let size = fetch.size.unwrap_or(0) as u64;
            let internal_date = fetch
                .internal_date()
                .map(|d| d.timestamp_millis())
                .unwrap_or(0);
            let header_bytes = match fetch.header() {
                Some(h) => h,
                None => &[],
            };
            let message_id = parse_message_id_header(header_bytes);
            result.push(crate::cache::imap::download::gap_fill::RemoteHeader {
                uid,
                message_id,
                size,
                internal_date,
            });
        }
        Ok(result)
    }
}

pub const DEFAULT_BATCH_SIZE: u32 = 30;
pub const DEFAULT_MAX_EMAIL_SIZE: u64 = 100 * 1024 * 1024;

/// Average seconds between consecutive messages above which the IMAP server is
/// considered slow (a healthy server responds in milliseconds).
const SLOW_SERVER_THRESHOLD_SECS: f64 = 3.0;
/// Seconds of silence before the UI is told the server is stalling on the
/// current message (lower than SLOW_SERVER_THRESHOLD so the message flips to
/// "waiting" promptly once the server stops feeding).
const STALL_REPORT_THRESHOLD_SECS: f64 = 3.0;
/// Silence beyond this is treated as likely rate limiting (throttling), not
/// just slowness — the UI then explains what Bichon is doing about it.
const RATE_LIMIT_THRESHOLD_SECS: f64 = 10.0;

/// Returns a user-facing hint when the IMAP server is feeding messages slowly
/// or has gone silent, so the UI can explain "this is the server, not Bichon".
/// `None` when the server is responding normally or not enough messages
/// arrived to tell. `stall_secs` (server silent on the current message) takes
/// precedence over the running average.
pub fn slow_server_message(avg_secs: Option<f64>, stall_secs: Option<f64>) -> Option<String> {
    if let Some(stall) = stall_secs {
        if stall >= RATE_LIMIT_THRESHOLD_SECS {
            return Some(format!(
                "Possible IMAP rate limiting: server has been silent for {:.0}s. Bichon is pacing the download (pausing between batches) and will retry with backoff if the connection stalls.",
                stall
            ));
        }
        if stall >= STALL_REPORT_THRESHOLD_SECS {
            return Some(format!(
                "IMAP server is slow: no response for {:.0}s while fetching the next message; download is still in progress.",
                stall
            ));
        }
    }
    match avg_secs {
        Some(secs) if secs >= SLOW_SERVER_THRESHOLD_SECS => Some(format!(
            "IMAP server is slow (avg {:.1}s between messages); download is still in progress.",
            secs
        )),
        _ => None,
    }
}

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

/// Filters `(uid, size, internal_date_epoch_millis)` entries to those whose
/// internal date (ignoring time and timezone, matching RFC 3501's BEFORE key)
/// is strictly earlier than `date` (`%d-%b-%Y`, e.g. "26-May-2025").
/// Returns the matching UIDs sorted ascending. Errors if the date cannot be
/// parsed — a silently-broken date filter would download mail the user asked
/// to exclude.
fn filter_before_date(
    entries: &[(u32, u64, i64)],
    date: &str,
) -> BichonResult<Vec<u32>> {
    let cutoff = chrono::NaiveDate::parse_from_str(date, "%d-%b-%Y").map_err(|e| {
        raise_error!(
            format!("Invalid BEFORE date '{date}': {e}"),
            ErrorCode::InvalidParameter
        )
    })?;
    let mut uid_vec: Vec<u32> = entries
        .iter()
        .filter(|&&(_, _, internal_date)| {
            let d = chrono::DateTime::from_timestamp_millis(internal_date)
                .map(|dt| dt.date_naive())
                .unwrap_or_default();
            d < cutoff
        })
        .map(|&(uid, _, _)| uid)
        .collect();
    uid_vec.sort();
    Ok(uid_vec)
}

/// When a range enumeration comes back empty, decide whether that is an
/// anomaly (server claims messages exist in the range but none were returned)
/// or a genuine "no new mail" result. Returns a warning message for the
/// anomaly, `None` when the empty result is legitimate (and highest_uid may be
/// left unchanged safely).
fn empty_enumeration_anomaly(
    mailbox_name: &str,
    uid_range: &str,
    start_uid: u64,
    server_uid_next: Option<u32>,
) -> Option<String> {
    let uid_next = server_uid_next?;
    if (uid_next as u64) > start_uid {
        Some(format!(
            "Mailbox '{}': UID FETCH {} returned no UIDs but server UIDNEXT={} ({} messages in range). Refusing to advance highest_uid to avoid skipping them; the next sync will retry.",
            mailbox_name, uid_range, uid_next, uid_next.saturating_sub(start_uid as u32)
        ))
    } else {
        None
    }
}

/// Resolve an empty incremental enumeration once `empty_enumeration_anomaly`
/// has flagged it as suspicious.
///
/// The pure guard only knows `UIDNEXT > start_uid`, which cannot distinguish a
/// truncated/throttled empty (real mail we must NOT skip) from a retired-UID
/// tail (Gmail relabel/archive/move leaves `UIDNEXT` above the last surviving
/// message, so `{start}:*` is legitimately empty). Confirm with an
/// authoritative message count: `EXISTS` from EXAMINE and the local stored
/// count are both immune to the SEARCH/FETCH truncation that fools the guard.
///
/// Returns the value the caller should return from its fetch fn:
/// - `Ok(Some(uid))` — retired tail confirmed; advance `highest_uid` to `uid`.
/// - `Ok(None)` — genuine gap; refuse to advance and retry next sync.
async fn resolve_empty_enumeration(
    account: &AccountModel,
    mailbox: &MailBox,
    examined: &async_imap::types::Mailbox,
    start_uid: u64,
    anomaly_msg: String,
) -> BichonResult<Option<u32>> {
    let server_count = examined.exists as u64;
    let local_count = ENVELOPE_MANAGER.count_for_mailbox(account.id, mailbox.id)? as u64;

    if server_count <= local_count {
        // Retired-UID tail: the folder holds no more mail than we already
        // store, so the empty range is genuinely empty. Advance past it so the
        // guard stops re-firing every sync. Safe: we cannot skip mail that is
        // not there. (Conservative on the oversized-skipped edge: if the folder
        // has messages skipped for size, server_count may exceed local_count
        // and we simply fall through to the retry path -- never skipping mail.)
        let new_highest = examined
            .uid_next
            .unwrap_or(0)
            .saturating_sub(1)
            .max(start_uid as u32);
        info!(
            account_id = account.id,
            mailbox = %mailbox.name,
            server_count,
            local_count,
            new_highest,
            "Empty range confirmed as retired-UID tail (server count <= local); advancing highest_uid past it."
        );
        DownloadState::update_folder_progress(
            account.id,
            mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            Some("No new emails (retired-UID tail).".into()),
        )?;
        return Ok(Some(new_highest));
    }

    // Server genuinely holds more mail than we do -> real gap -> refuse & retry.
    tracing::warn!(
        account_id = account.id,
        mailbox = %mailbox.name,
        start_uid,
        uid_next = examined.uid_next,
        server_count,
        local_count,
        "{}",
        anomaly_msg
    );
    DownloadState::append_session_error(account.id, anomaly_msg)?;
    DownloadState::update_folder_progress(
        account.id,
        mailbox.name.clone(),
        0,
        0,
        FolderStatus::Failed,
        Some("UID enumeration came back empty despite new mail on server. Retrying on next sync.".into()),
    )?;
    Ok(None)
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
    use crate::imap::session::SessionStream;
    use tokio_io_timeout::TimeoutStream;

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

    // ── filter_before_date ─────────────────────────────────────────

    fn ms(y: i32, m: u32, d: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn filter_before_date_keeps_only_earlier_dates() {
        let entries = vec![
            (1, 100, ms(2025, 5, 20)),
            (2, 200, ms(2025, 5, 26)), // exactly on the cutoff day — excluded (BEFORE is strict)
            (3, 300, ms(2025, 5, 27)),
        ];
        let uids = filter_before_date(&entries, "27-May-2025").unwrap();
        assert_eq!(uids, vec![1, 2]);
    }

    #[test]
    fn filter_before_date_timezone_ignored() {
        // Internal date late in the day in +1400 still counts as that day:
        // 2025-05-26 23:59:00 +1400 == 2025-05-26 09:59 UTC.
        let entries = vec![
            (1, 100, ms(2025, 5, 25)),
            (2, 200, ms(2025, 5, 26)),
        ];
        let uids = filter_before_date(&entries, "26-May-2025").unwrap();
        assert_eq!(uids, vec![1]);
    }

    #[test]
    fn filter_before_date_invalid_date_errors() {
        let entries = vec![(1, 100, ms(2025, 5, 20))];
        assert!(filter_before_date(&entries, "not-a-date").is_err());
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

    // ── empty_enumeration_anomaly ──────────────────────────────────

    #[test]
    fn empty_enumeration_no_anomaly_when_uidnext_below_start() {
        // No new mail: server UIDNEXT <= start_uid → legitimate empty result.
        assert_eq!(
            empty_enumeration_anomaly("INBOX", "816098:*", 816098, Some(816098)),
            None
        );
        assert_eq!(
            empty_enumeration_anomaly("INBOX", "816098:*", 816098, Some(816097)),
            None
        );
    }

    #[test]
    fn empty_enumeration_no_anomaly_when_uidnext_unknown() {
        // Server did not report UIDNEXT; cannot prove mail exists in range.
        assert_eq!(
            empty_enumeration_anomaly("INBOX", "816098:*", 816098, None),
            None
        );
    }

    #[test]
    fn empty_enumeration_anomaly_when_uidnext_above_start() {
        // Server claims messages exist but none came back → anomaly message.
        let msg = empty_enumeration_anomaly("portal_issues", "816098:*", 816098, Some(816118));
        let msg = msg.expect("should be Some for anomalous empty enumeration");
        assert!(msg.contains("portal_issues"));
        assert!(msg.contains("UIDNEXT=816118"));
    }

    // ── collect_range_uids via mock server ─────────────────────────

    use crate::imap::mock_server::{
        examine_response, uid_fetch_size_response, MockImapServer, MockImapServerHandle,
    };

    /// Build an `async_imap::Session` connected to the mock server,
    /// authenticated and with the given mailbox examined.
    async fn mock_session(
        handle: &MockImapServerHandle,
    ) -> async_imap::Session<Box<dyn SessionStream>> {
        let tcp = tokio::net::TcpStream::connect((handle.host(), handle.port()))
            .await
            .unwrap();
        let timeout_stream = TimeoutStream::new(tcp);
        let pinned: std::pin::Pin<Box<TimeoutStream<tokio::net::TcpStream>>> =
            Box::pin(timeout_stream);
        let stream: Box<dyn SessionStream> = Box::new(pinned);
        let mut client = async_imap::Client::new(stream);

        // Read greeting
        client.read_response().await.unwrap();

        // Login
        let mut session = client
            .login("user", "pass")
            .await
            .map_err(|(e, _)| panic!("Login failed: {e:?}"))
            .unwrap();

        // Examine
        session.examine("INBOX").await.unwrap();

        session
    }

    #[tokio::test]
    async fn collect_range_uids_via_mock_server() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 3, 42, 4))
            .respond(
                "UID FETCH",
                uid_fetch_size_response(&[(1, 100), (2, 200), (3, 300)]),
            )
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let mut mailbox = MailBox::default();
        mailbox.name = "INBOX".into();

        let (entries, skipped) = ImapExecutor::collect_range_uids(
            &mut session,
            "1:*",
            1,
            &mailbox,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            entries.iter().map(|&(uid, _, _)| uid).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(skipped, 0);
        session.logout().await.ok();
    }

    #[tokio::test]
    async fn collect_range_uids_empty_mailbox() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 0, 42, 1))
            .respond("UID FETCH", b"{TAG} OK FETCH completed\r\n".to_vec())
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let mut mailbox = MailBox::default();
        mailbox.name = "INBOX".into();

        let (entries, skipped) = ImapExecutor::collect_range_uids(
            &mut session,
            "1:*",
            1,
            &mailbox,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(entries.is_empty());
        assert_eq!(skipped, 0);
        session.logout().await.ok();
    }

    #[tokio::test]
    async fn collect_range_uids_filters_oversized() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 4, 42, 5))
            .respond(
                "UID FETCH",
                uid_fetch_size_response(&[(1, 100), (2, 500), (3, 1000), (4, 2000)]),
            )
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let mut mailbox = MailBox::default();
        mailbox.name = "INBOX".into();

        // Limit 1000: UIDs 1..3 accepted, UID 4 (2000) skipped.
        let (entries, skipped) = ImapExecutor::collect_range_uids(
            &mut session,
            "1:*",
            1,
            &mailbox,
            Some(1000),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(
            entries.iter().map(|&(uid, _, _)| uid).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(skipped, 1);
        session.logout().await.ok();
    }
}
