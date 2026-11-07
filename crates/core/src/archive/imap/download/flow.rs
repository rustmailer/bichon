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

use crate::{
    raise_error,
    {
        account::{
            migration::AccountModel,
            state::{DownloadState, DownloadStatus, FolderStatus},
        },
        archive::{
            imap::{
                download::rebuild::{rebuild_mailbox_cache, rebuild_mailbox_cache_by_date},
                find_intersecting_mailboxes, find_missing_mailboxes,
                mailbox::MailBox,
            },
            SEMAPHORE,
        },
        error::{code::ErrorCode, BichonResult},
        imap::executor::{
            compress_uid_list, generate_uid_sequence_hashset, slow_server_message, ImapExecutor,
            DEFAULT_BATCH_SIZE,
        },
        store::tantivy::envelope::ENVELOPE_MANAGER,
    },
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

const MAX_NETWORK_RETRIES: u32 = 3;

/// Whether an error is worth retrying on a fresh connection. Throttling is
/// retryable too — just much more slowly (see [`retry_backoff_secs`]).
pub(crate) fn is_retryable_error(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::NetworkError | ErrorCode::ImapServerThrottled)
}

/// Seconds to wait before retry number `retry` (1-based).
///
/// A throttled server needs far longer than a transient network blip before
/// it will accept the next command; retrying a throttle after 5 s is what
/// turned one refusal into a 72-login storm in the #396 logs.
pub(crate) fn retry_backoff_secs(code: ErrorCode, retry: u32) -> u64 {
    let table: [u64; 3] = if code == ErrorCode::ImapServerThrottled {
        [30, 60, 120]
    } else {
        [5, 15, 30]
    };
    table[(retry.saturating_sub(1) as usize).min(table.len() - 1)]
}

/// Whether the server is refusing this account's commands outright. Further
/// folders in the same round would only burn more of its quota, so the round
/// stops and the account backs off instead (#396).
pub(crate) fn is_throttle_error(e: &crate::error::BichonError) -> bool {
    e.code() == ErrorCode::ImapServerThrottled
}

/// When `UIDNEXT` has not passed the local watermark, no message can be new,
/// so the folder can be skipped without opening a connection at all.
///
/// `None` means "cannot tell, do the normal sync" (no stored watermark yet,
/// or the server does not report `UIDNEXT` in `STATUS`).
fn incremental_skip_target(
    remote_uid_next: Option<u32>,
    local_highest: Option<u32>,
) -> Option<u32> {
    let next = remote_uid_next?;
    let highest = local_highest?;
    ((next as u64) <= highest as u64 + 1).then_some(highest)
}

/// Per-round result of [`reconcile_mailboxes`].
#[derive(Clone, Debug, Default)]
pub struct ReconcileSummary {
    /// Folders that completed this round.
    pub synced_folders: u32,
    /// Folders that failed this round (they are retried next round).
    pub failed_folders: u32,
    /// The round was cut short because the server throttled the account;
    /// folders after that point were not attempted at all.
    pub throttled: bool,
}

/// Maps a round summary to the session status the UI shows. Never `Success`
/// when any folder failed or was deferred — that is the whole point of
/// `Partial` (#396).
pub(crate) fn session_status_for(
    summary: &ReconcileSummary,
) -> (crate::account::state::DownloadStatus, Option<String>) {
    use crate::account::state::DownloadStatus;
    let problem = summary.throttled || summary.failed_folders > 0;
    if !problem {
        return (DownloadStatus::Success, None);
    }

    let message = if summary.throttled {
        format!(
            "IMAP server throttled this account: {} folder(s) synced, the remaining folders \
             were deferred to the next sync. Bichon backs off instead of retrying immediately.",
            summary.synced_folders
        )
    } else {
        format!(
            "{} folder(s) synced, {} failed. Failed folders are retried on the next sync.",
            summary.synced_folders, summary.failed_folders
        )
    };

    // Nothing completed: this round is a failure, not a partial success.
    let status = if summary.synced_folders == 0 {
        DownloadStatus::Failed
    } else {
        DownloadStatus::Partial
    };
    (status, Some(message))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchDirection {
    Since,
    Before,
}

pub async fn fetch_and_save_by_date(
    account: &AccountModel,
    date: &str,
    mailbox: &MailBox,
    direction: FetchDirection,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    let account_id = account.id;
    let mut session = match ImapExecutor::create_connection(account_id).await {
        Ok(session) => session,
        Err(e) => {
            let err_msg = format!("Connection failed for this folder: {:#?}", e);
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Failed,
                Some(err_msg.clone()),
            )?;
            DownloadState::append_session_error(account_id, err_msg)?;
            return Err(e);
        }
    };

    let search_criteria = match direction {
        FetchDirection::Since => format!("SINCE {date}"),
        FetchDirection::Before => format!("BEFORE {date}"),
    };

    let uid_list =
        match ImapExecutor::uid_search(&mut session, &mailbox.encoded_name(), &search_criteria)
            .await
        {
            Ok(uid_list) => uid_list,
            Err(e) => {
                let err_msg = format!("UID search failed in [{}]: {:#?}", mailbox.name, e);
                DownloadState::update_folder_progress(
                    account_id,
                    mailbox.name.clone(),
                    0,
                    0,
                    FolderStatus::Failed,
                    Some(err_msg.clone()),
                )?;
                DownloadState::append_session_error(account_id, err_msg)?;
                return Err(e);
            }
        };

    let len = uid_list.len();
    if len == 0 {
        DownloadState::update_folder_progress(
            account_id,
            mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
        return Ok(None);
    }

    // sort small -> bigger
    let mut uid_vec: Vec<u32> = uid_list.into_iter().collect();
    uid_vec.sort();

    let max_uid = uid_vec.last().copied();
    let planned = uid_vec.len() as u64;
    let uid_batches = generate_uid_sequence_hashset(
        uid_vec,
        account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize,
    );
    DownloadState::update_folder_progress(
        account_id,
        mailbox.name.clone(),
        planned,
        0,
        FolderStatus::Pending,
        None,
    )?;

    let mut current_processed = 0u64;
    // Messages skipped because they could not be parsed (not archived). They
    // do not fail the folder — the watermark advances past them so scheduled
    // syncs do not re-fetch a permanently broken message — but the total is
    // reported so the running state carries a warning (they stay "missing"
    // locally, so gap-fill keeps detecting them).
    let mut parse_skipped = 0u64;
    let mut filtered = 0u64;
    let mut has_error_or_cancel = false;
    for (index, batch) in uid_batches.into_iter().enumerate() {
        if token.is_cancelled() {
            DownloadState::update_session_status(
                account_id,
                DownloadStatus::Cancelled,
                Some("User stopped or system shutdown".to_string()),
            )?;
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                planned,
                current_processed,
                FolderStatus::Cancelled,
                None,
            )?;
            has_error_or_cancel = true;
            break;
        }
        // Fetch metadata for the current batch of UIDs
        let mut retries = 0u32;
        let batch_result = loop {
            match ImapExecutor::uid_batch_retrieve_emails(
                &mut session,
                account_id,
                mailbox.id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
                Some(&|cumulative, _cumulative_filtered, avg_secs, stall_secs| {
                    // Per-message progress: the current batch's cumulative count
                    // keeps the UI moving while a slow server trickles messages.
                    let _ = DownloadState::update_folder_progress(
                        account_id,
                        mailbox.name.clone(),
                        planned,
                        cumulative,
                        FolderStatus::Downloading,
                        slow_server_message(avg_secs, stall_secs),
                    );
                    Ok(())
                }),
            )
            .await
            {
                Ok(outcome) => break Ok(outcome),
                Err(e) if retries < MAX_NETWORK_RETRIES && is_retryable_error(e.code()) => {
                    retries += 1;
                    let code = e.code();
                    warn!(
                        account_id,
                        mailbox = mailbox.name,
                        index,
                        retries,
                        "{} on batch, reconnecting ({}/{})",
                        if code == ErrorCode::ImapServerThrottled {
                            "Server throttling"
                        } else {
                            "Network error"
                        },
                        retries,
                        MAX_NETWORK_RETRIES
                    );
                    match ImapExecutor::create_connection(account_id).await {
                        Ok(new_session) => {
                            session = new_session;
                            if let Err(e2) = session.examine(&mailbox.encoded_name()).await {
                                let err_msg =
                                    format!("Re-examine failed after reconnect: {:#?}", e2);
                                DownloadState::append_session_error(account_id, err_msg)?;
                                break Err(e);
                            }
                            // A throttled server needs much longer than a
                            // transient network blip before it will accept
                            // the next command.
                            let backoff = retry_backoff_secs(code, retries);
                            tokio::time::sleep(Duration::from_secs(backoff)).await;
                            continue;
                        }
                        Err(e2) => {
                            error!(account_id, "Reconnection failed: {:#?}", e2);
                            break Err(e);
                        }
                    }
                }
                Err(e) => break Err(e),
            }
        };
        match batch_result {
            Ok(outcome) => {
                current_processed += outcome.stored;
                parse_skipped += outcome.parse_skipped;
                filtered += outcome.filtered;
                DownloadState::update_folder_progress_and_filtered(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
                    filtered,
                    FolderStatus::Downloading,
                    None,
                )?;
            }
            Err(e) => {
                let err_msg = format!("Batch {} failed: {:#?}", index, e);
                DownloadState::append_session_error(account_id, err_msg.clone())?;
                DownloadState::update_folder_progress(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
                    FolderStatus::Failed,
                    Some(err_msg),
                )?;
                has_error_or_cancel = true;
                break;
            }
        }
    }
    if !has_error_or_cancel {
        if parse_skipped > 0 {
            // Folder-level summary of the per-message warnings the batch
            // fetch already recorded, so the running state's error list
            // carries the total as well.
            let _ = DownloadState::append_session_error(
                account_id,
                format!(
                    "Mailbox '{}': {} email(s) could not be parsed and were skipped \
                     (not archived); gap-fill will keep detecting them",
                    mailbox.name, parse_skipped
                ),
            );
        }
        DownloadState::update_folder_progress(
            account_id,
            mailbox.name.clone(),
            planned,
            current_processed,
            FolderStatus::Success,
            None,
        )?;
    }
    session.logout().await.ok();
    // Only hand back a watermark when the whole folder completed: the caller
    // persists it as the incremental-sync cursor, and a mid-download failure
    // must not move it past UIDs that were never fetched (#377).
    Ok(if has_error_or_cancel { None } else { max_uid })
}

/// Fetches all messages from a mailbox.
/// Returns `Ok(Some(max_uid))` with the highest UID stored, or `Ok(None)` if empty.
///
/// The mailbox is enumerated via `UID SEARCH ALL` first, then downloaded in UID
/// batches. Unlike sequence-number paging, UIDs stay stable while the download
/// runs (new arrivals only get larger UIDs), so no message is silently skipped
/// when the server changes mid-download.
pub async fn fetch_and_save_full_mailbox(
    account: &AccountModel,
    mailbox: &MailBox,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    let mailbox_id = mailbox.id;
    let account_id = account.id;

    let mut session = match ImapExecutor::create_connection(account_id).await {
        Ok(session) => session,
        Err(e) => {
            let err_msg = format!("Connection failed for this folder: {:#?}", e);
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Failed,
                Some(err_msg.clone()),
            )?;
            DownloadState::append_session_error(account_id, err_msg)?;
            return Err(e);
        }
    };

    // When retention is active, full downloads are limited to the retention
    // window: everything older was already purged (or is rejected at ingest),
    // so refetching it would just waste bandwidth and get swept again.
    let full_search = match crate::retention::retention_floor_date(account) {
        Some(floor) => format!("SINCE {floor}"),
        None => "ALL".to_string(),
    };
    let uid_list = match ImapExecutor::uid_search(&mut session, &mailbox.encoded_name(), &full_search)
        .await
    {
        Ok(list) => list,
        Err(e) => {
            let err_msg = format!("UID SEARCH failed in [{}]: {:#?}", mailbox.name, e);
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Failed,
                Some(err_msg.clone()),
            )?;
            DownloadState::append_session_error(account_id, err_msg)?;
            session.logout().await.ok();
            return Err(e);
        }
    };

    // `uid_search` returns a set; the download loop below pages over UIDs in
    // ascending order (stable while new mail only grows UIDs).
    let mut uid_list: Vec<u32> = uid_list.into_iter().collect();
    uid_list.sort();

    let planned = uid_list.len() as u64;
    if planned == 0 {
        info!(
            "Mailbox '{}' is empty, no emails to download.",
            mailbox.name
        );
        DownloadState::update_folder_progress(
            account_id,
            mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
        session.logout().await.ok();
        return Ok(None);
    }

    let max_uid = *uid_list.last().unwrap();
    let page_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
    let uid_batches = generate_uid_sequence_hashset(uid_list, page_size);
    let total_batches = uid_batches.len();

    info!(
        "Starting full mailbox download for '{}', uids={}, batches={}",
        mailbox.name, planned, total_batches
    );

    let mut current_processed = 0u64;
    // Messages skipped because they could not be parsed (not archived). They
    // do not fail the folder — the watermark advances past them so scheduled
    // syncs do not re-fetch a permanently broken message — but the total is
    // reported so the running state carries a warning (they stay "missing"
    // locally, so gap-fill keeps detecting them).
    let mut parse_skipped = 0u64;
    let mut filtered = 0u64;
    let mut has_error_or_cancel = false;

    for (index, batch) in uid_batches.into_iter().enumerate() {
        if token.is_cancelled() {
            DownloadState::update_session_status(
                account_id,
                DownloadStatus::Cancelled,
                Some("User stopped or system shutdown".to_string()),
            )?;
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                planned,
                current_processed,
                FolderStatus::Cancelled,
                None,
            )?;
            has_error_or_cancel = true;
            break;
        }

        // Heartbeat: keeps `current_folder` fresh so the web UI can show
        // "waiting for server" while a slow IMAP server is mid-batch.
        DownloadState::set_current_folder(account_id, mailbox.name.clone())?;

        // Heartbeat: keeps `current_folder` fresh so the web UI can show
        // "waiting for server" while a slow IMAP server is mid-batch.
        DownloadState::set_current_folder(account_id, mailbox.name.clone())?;

        let mut retries = 0u32;
        let batch_result = loop {
            match ImapExecutor::uid_batch_retrieve_emails(
                &mut session,
                account_id,
                mailbox_id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
                Some(&|cumulative, _cumulative_filtered, avg_secs, stall_secs| {
                    // Per-message progress: the current batch's cumulative count
                    // keeps the UI moving while a slow server trickles messages.
                    let _ = DownloadState::update_folder_progress(
                        account_id,
                        mailbox.name.clone(),
                        planned,
                        cumulative,
                        FolderStatus::Downloading,
                        slow_server_message(avg_secs, stall_secs),
                    );
                    Ok(())
                }),
            )
            .await
            {
                Ok(outcome) => break Ok(outcome),
                Err(e) if retries < MAX_NETWORK_RETRIES && is_retryable_error(e.code()) => {
                    retries += 1;
                    let code = e.code();
                    warn!(
                        account_id,
                        mailbox = mailbox.name,
                        index,
                        retries,
                        "{} on batch, reconnecting ({}/{})",
                        if code == ErrorCode::ImapServerThrottled {
                            "Server throttling"
                        } else {
                            "Network error"
                        },
                        retries,
                        MAX_NETWORK_RETRIES
                    );
                    // Refresh the heartbeat after a reconnect too.
                    let _ = DownloadState::set_current_folder(account_id, mailbox.name.clone());
                    match ImapExecutor::create_connection(account_id).await {
                        Ok(new_session) => {
                            session = new_session;
                            if let Err(e2) = session.examine(&mailbox.encoded_name()).await {
                                let err_msg =
                                    format!("Re-examine failed after reconnect: {:#?}", e2);
                                DownloadState::append_session_error(account_id, err_msg)?;
                                break Err(e);
                            }
                            // A throttled server needs much longer than a
                            // transient network blip before it will accept
                            // the next command.
                            let backoff = retry_backoff_secs(code, retries);
                            tokio::time::sleep(Duration::from_secs(backoff)).await;
                            continue;
                        }
                        Err(e2) => {
                            error!(account_id, "Reconnection failed: {:#?}", e2);
                            break Err(e);
                        }
                    }
                }
                Err(e) => break Err(e),
            }
        };
        match batch_result {
            Ok(outcome) => {
                current_processed += outcome.stored;
                parse_skipped += outcome.parse_skipped;
                filtered += outcome.filtered;
                DownloadState::update_folder_progress_and_filtered(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
                    filtered,
                    FolderStatus::Downloading,
                    None,
                )?;
            }
            Err(e) => {
                let err_msg = format!("Batch {} failed: {:#?}", index, e);
                DownloadState::append_session_error(account_id, err_msg.clone())?;
                DownloadState::update_folder_progress(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
                    FolderStatus::Failed,
                    Some(err_msg),
                )?;
                has_error_or_cancel = true;
                break;
            }
        }
    }

    if !has_error_or_cancel {
        if parse_skipped > 0 {
            // Folder-level summary of the per-message warnings the batch
            // fetch already recorded, so the running state's error list
            // carries the total as well.
            let _ = DownloadState::append_session_error(
                account_id,
                format!(
                    "Mailbox '{}': {} email(s) could not be parsed and were skipped \
                     (not archived); gap-fill will keep detecting them",
                    mailbox.name, parse_skipped
                ),
            );
        }
        DownloadState::update_folder_progress(
            account_id,
            mailbox.name.clone(),
            planned,
            current_processed,
            FolderStatus::Success,
            None,
        )?;
    }
    session.logout().await.ok();
    // Only hand back a watermark when the whole folder completed: the caller
    // persists it as the incremental-sync cursor, and a mid-download failure
    // must not move it past UIDs that were never fetched (#377).
    if has_error_or_cancel {
        Ok(None)
    } else {
        Ok(Some(max_uid))
    }
}

/// Generates a synthetic UIDVALIDITY for IMAP servers that don't provide it.
/// Uses a stable hash of the mailbox name to ensure consistent IDs across sessions.
fn generate_synthetic_uidvalidity(mailbox_name: &str) -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    mailbox_name.hash(&mut hasher);
    (hasher.finish() as u32).wrapping_add(1) // Avoid 0, which might be reserved
}

/// Retry fetching UIDVALIDITY via STATUS command when the initial listing returned None.
/// A None UIDVALIDITY while other STATUS fields (MESSAGES, UNSEEN, UIDNEXT) are present
/// can be caused by transient network issues corrupting just the UIDVALIDITY portion of
/// the response. Retrying avoids unnecessarily triggering a full reconcile.
async fn fetch_uid_validity_with_retry(
    account_id: u64,
    mailbox_name: &str,
    max_retries: u32,
) -> BichonResult<Option<u32>> {
    let mailbox_name = mailbox_name.to_string();
    fetch_uid_validity_with_retry_inner(max_retries, move || {
        let account_id = account_id;
        let mailbox_name = mailbox_name.clone();
        async move {
            let mut session = ImapExecutor::create_connection(account_id).await?;
            let result = session
                .status(&mailbox_name, "(UIDVALIDITY)")
                .await
                .map(|r| r.uid_validity)
                .map_err(|e| {
                    let msg = format!("STATUS failed during UIDVALIDITY retry: {:#?}", e);
                    raise_error!(msg, ErrorCode::InternalError)
                });
            session.logout().await.ok();
            result
        }
    })
    .await
}

/// Generic retry loop: calls `fetch_fn` up to `max_retries` times with
/// backoff (500ms × attempt). Returns the first `Some(uid)`, or `Ok(None)`
/// if all attempts return `None` or error.
async fn fetch_uid_validity_with_retry_inner<F, Fut>(
    max_retries: u32,
    mut fetch_fn: F,
) -> BichonResult<Option<u32>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = BichonResult<Option<u32>>>,
{
    for attempt in 0..max_retries {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
        }

        match fetch_fn().await {
            Ok(Some(uid)) => return Ok(Some(uid)),
            Ok(None) => {
                warn!(
                    attempt = attempt + 1,
                    max_retries, "STATUS returned no UIDVALIDITY"
                );
            }
            Err(e) => {
                warn!(
                    attempt = attempt + 1,
                    max_retries, "UIDVALIDITY fetch attempt failed: {:#?}", e
                );
            }
        }
    }

    Ok(None)
}

/// Handle uid_validity change without deleting local data.
/// Compares remote Message-IDs with local Tantivy index, downloads only
/// truly missing emails. DedupCache catches any remaining duplicates.
async fn reconcile_uid_validity_change(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    let account_id = account.id;

    // Phase 1: connect + examine
    let mut session = ImapExecutor::create_connection(account_id).await?;
    session
        .examine(&remote_mailbox.encoded_name())
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    // Phase 2: collect remote UIDs, respecting date constraints. When the
    // account has a retention window, a `SINCE` floor is composed in so the
    // full UID compare does not re-download mail the retention sweep already
    // purged. The floor is conservative (INTERNALDATE based): anything the
    // server still reports as recent is fetched, then re-checked against the
    // exact window at ingest time.
    let retention_floor = crate::retention::retention_floor_date(account);
    let remote_uid_list: Vec<u32> = {
        let mut criteria: Vec<String> = Vec::new();
        if let Some(date_since) = &account.date_since {
            criteria.push(format!("SINCE {}", date_since.since_date()?));
        }
        if let Some(date_before) = &account.date_before {
            criteria.push(format!("BEFORE {}", date_before.calculate_date()?));
        }
        if let Some(floor) = retention_floor {
            criteria.push(format!("SINCE {floor}"));
        }
        let query = if criteria.is_empty() {
            "ALL".to_string()
        } else {
            criteria.join(" ")
        };
        let results = session
            .uid_search(&query)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut v: Vec<u32> = results.into_iter().collect();
        v.sort();
        v
    };

    if remote_uid_list.is_empty() {
        DownloadState::update_folder_progress(
            account_id,
            remote_mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            Some("UIDVALIDITY changed but remote mailbox is empty.".into()),
        )?;
        session.logout().await.ok();
        return Ok(None);
    }

    let max_uid = remote_uid_list.last().copied();

    // Phase 3: fetch remote Message-IDs (headers only, no bodies)
    let uid_set = compress_uid_list(remote_uid_list.clone());
    let remote_msg_ids =
        ImapExecutor::fetch_uid_metadata(&mut session, &uid_set, token.clone()).await?;
    session.logout().await.ok();

    // Phase 4: query local Message-IDs from Tantivy.
    // For a large mailbox this can allocate 50-100 MB of HashSet.
    let local_msg_ids =
        ENVELOPE_MANAGER.get_message_ids_for_mailbox(account_id, local_mailbox.id)?;

    // Phase 5: compute missing UIDs
    let mut missing_uids: Vec<u32> = Vec::new();
    for uid in &remote_uid_list {
        if token.is_cancelled() {
            return Err(raise_error!("Cancelled".into(), ErrorCode::InternalError));
        }
        match remote_msg_ids.get(uid) {
            Some(Some(msg_id)) if local_msg_ids.contains(msg_id) => {
                // already have this email locally
            }
            _ => missing_uids.push(*uid),
        }
    }

    // Phase 6: download missing
    if missing_uids.is_empty() {
        info!(
            account_id,
            mailbox = remote_mailbox.name,
            "UIDVALIDITY changed but all {} emails already exist locally",
            remote_uid_list.len()
        );
        DownloadState::update_folder_progress(
            account_id,
            remote_mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
    } else {
        let planned = missing_uids.len() as u64;
        info!(
            account_id,
            mailbox = remote_mailbox.name,
            total = remote_uid_list.len(),
            missing = planned,
            "UIDVALIDITY changed, downloading missing emails"
        );
        DownloadState::update_folder_progress(
            account_id,
            remote_mailbox.name.clone(),
            planned,
            0,
            FolderStatus::Downloading,
            Some("UIDVALIDITY changed, downloading missing emails...".into()),
        )?;

        let mut session2 = ImapExecutor::create_connection(account_id).await?;
        session2
            .examine(&remote_mailbox.encoded_name())
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
        let batches = generate_uid_sequence_hashset(missing_uids, batch_size);

        let mut downloaded = 0u64;
        // Messages skipped because they could not be parsed (not archived);
        // reported as a folder-level warning once the batch loop ends.
        let mut parse_skipped = 0u64;
        let mut filtered = 0u64;
        for (index, batch) in batches.into_iter().enumerate() {
            if token.is_cancelled() {
                DownloadState::update_folder_progress(
                    account_id,
                    remote_mailbox.name.clone(),
                    planned,
                    downloaded,
                    FolderStatus::Cancelled,
                    None,
                )?;
                session2.logout().await.ok();
                return Err(raise_error!("Cancelled".into(), ErrorCode::InternalError));
            }

            match ImapExecutor::uid_batch_retrieve_emails(
                &mut session2,
                account_id,
                remote_mailbox.id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
                Some(&|cumulative, _cumulative_filtered, avg_secs, stall_secs| {
                    // Per-message progress: the current batch's cumulative count
                    // keeps the UI moving while a slow server trickles messages.
                    let _ = DownloadState::update_folder_progress(
                        account_id,
                        remote_mailbox.name.clone(),
                        planned,
                        cumulative,
                        FolderStatus::Downloading,
                        slow_server_message(avg_secs, stall_secs),
                    );
                    Ok(())
                }),
            )
            .await
            {
                Ok(outcome) => {
                    downloaded += outcome.stored;
                    parse_skipped += outcome.parse_skipped;
                    filtered += outcome.filtered;
                    DownloadState::update_folder_progress_and_filtered(
                        account_id,
                        remote_mailbox.name.clone(),
                        planned,
                        downloaded,
                        filtered,
                        FolderStatus::Downloading,
                        None,
                    )?;
                }
                Err(e) => {
                    let err_msg = format!("Batch {} failed: {:#?}", index, e);
                    DownloadState::append_session_error(account_id, err_msg.clone())?;
                    DownloadState::update_folder_progress(
                        account_id,
                        remote_mailbox.name.clone(),
                        planned,
                        downloaded,
                        FolderStatus::Failed,
                        Some(err_msg),
                    )?;
                    session2.logout().await.ok();
                    return Err(e);
                }
            }
        }

        if parse_skipped > 0 {
            let _ = DownloadState::append_session_error(
                account_id,
                format!(
                    "Mailbox '{}': {} email(s) could not be parsed and were skipped \
                     (not archived); gap-fill will keep detecting them",
                    remote_mailbox.name, parse_skipped
                ),
            );
        }
        DownloadState::update_folder_progress(
            account_id,
            remote_mailbox.name.clone(),
            planned,
            downloaded,
            FolderStatus::Success,
            None,
        )?;
        session2.logout().await.ok();
    }

    Ok(max_uid)
}

pub async fn reconcile_mailboxes(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    local_mailboxes: &[MailBox],
    token: CancellationToken,
) -> BichonResult<ReconcileSummary> {
    let start_time = Instant::now();
    let existing_mailboxes = find_intersecting_mailboxes(local_mailboxes, remote_mailboxes);
    let account_id = account.id;
    // A failed folder must not stop the other folders from being attempted
    // (#377), but it must keep the session from reporting Success — the
    // caller maps `failed_folders > 0` to `Partial` (#396). A *throttled*
    // folder is different: the server has started refusing this account, so
    // continuing only burns more of its quota.
    let mut summary = ReconcileSummary::default();
    if !existing_mailboxes.is_empty() {
        let mut mailboxes_to_update = Vec::with_capacity(existing_mailboxes.len());

        DownloadState::init_folder_details(
            account.id,
            remote_mailboxes.iter().map(|m| m.name.clone()).collect(),
        )?;

        for (local_mailbox, remote_mailbox) in &existing_mailboxes {
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("Received termination signal (User stop or System shutdown)".to_string()),
                )?;
                break;
            }

            // Handle missing UIDVALIDITY from non-compliant IMAP servers
            // (e.g., Tencent Enterprise Mail, etc.)
            let remote_uid_validity = match remote_mailbox.uid_validity {
                Some(uid) => uid,
                None => {
                    if local_mailbox.uid_validity.is_some() {
                        // We had a real UIDVALIDITY before; a None now is likely
                        // transient (network jitter). Retry before falling back.
                        warn!(
                            "Account {}: Mailbox '{}' - STATUS returned no UIDVALIDITY, retrying to rule out network jitter...",
                            account_id, remote_mailbox.name
                        );
                        match fetch_uid_validity_with_retry(
                            account_id,
                            &remote_mailbox.encoded_name(),
                            3,
                        )
                        .await?
                        {
                            Some(uid) => {
                                info!(
                                    "Account {}: Mailbox '{}' - UIDVALIDITY recovered after retry: {}",
                                    account_id, remote_mailbox.name, uid
                                );
                                uid
                            }
                            None => {
                                // All retries exhausted; keep the local value.
                                // Safer than synthetic because we know the server had one.
                                let fallback = local_mailbox.uid_validity.unwrap();
                                warn!(
                                    "Account {}: Mailbox '{}' - all retries failed, keeping local UIDVALIDITY {}",
                                    account_id, remote_mailbox.name, fallback
                                );
                                fallback
                            }
                        }
                    } else {
                        // First sync and server genuinely doesn't provide UIDVALIDITY
                        let synthetic_uid = generate_synthetic_uidvalidity(&remote_mailbox.name);
                        warn!(
                            "Account {}: Mailbox '{}' - Server did not provide UIDVALIDITY. \
                            Using synthetic UIDVALIDITY {} based on mailbox name. \
                            This mailbox will be synced but may require periodic rebuilds if the server's mailbox structure changes.",
                            account_id, remote_mailbox.name, synthetic_uid
                        );
                        synthetic_uid
                    }
                }
            };

            let new_highest_uid = if local_mailbox.uid_validity != Some(remote_uid_validity) {
                info!(
                    "Account {}: Mailbox '{}' detected with changed uid_validity (local: {:#?}, remote: {:#?}). \
                    Comparing by Message-ID to find missing emails.",
                    account_id, local_mailbox.name, &local_mailbox.uid_validity, &remote_uid_validity
                );

                reconcile_uid_validity_change(account, local_mailbox, remote_mailbox, token.clone())
                    .await
            } else {
                perform_incremental_sync(account, local_mailbox, remote_mailbox, token.clone())
                    .await
            };
            // A folder failure must not abort the remaining folders: record
            // it, keep the session honest (the caller maps failed folders to
            // `Partial`), and move on. Throttling is the exception: every
            // further folder would open a new connection that the server is
            // already refusing, so the round stops here.
            let new_highest_uid = match new_highest_uid {
                Ok(v) => v,
                Err(e) => {
                    let err_msg = format!("Folder '{}' sync failed: {:#?}", remote_mailbox.name, e);
                    warn!(account_id, "{}", err_msg);
                    DownloadState::append_session_error(account.id, err_msg)?;
                    if is_throttle_error(&e) {
                        summary.throttled = true;
                        warn!(
                            account_id,
                            "Server throttled this account; deferring the remaining folders"
                        );
                        break;
                    }
                    summary.failed_folders += 1;
                    continue;
                }
            };
            summary.synced_folders += 1;
            info!(
                account_id,
                mailbox = %remote_mailbox.name,
                local_highest = local_mailbox.highest_uid,
                new_highest = new_highest_uid,
                "reconcile: computed highest_uid for mailbox"
            );
            let mut updated = remote_mailbox.clone();
            // Preserve the stored watermark when this round produced none
            // (folder failed / empty): None must never clear it, or the
            // un-downloaded range would be skipped forever. Note
            // `remote_mailbox.highest_uid` comes from a fresh remote listing
            // and is not the local value.
            updated.highest_uid = new_highest_uid.or(local_mailbox.highest_uid);
            // Update uid_validity with the resolved value (either from server or synthetic)
            if updated.uid_validity.is_none() {
                updated.uid_validity = Some(remote_uid_validity);
            }
            info!(
                account_id,
                mailbox = %updated.name,
                highest_uid = updated.highest_uid,
                "reconcile: persisting mailbox state"
            );
            mailboxes_to_update.push(updated);
        }
        //The metadata of this mailbox must only be updated after a successful synchronization;
        //otherwise, it may cause synchronization errors and result in missing emails in the local sync results.
        MailBox::batch_upsert(&mailboxes_to_update)?;
    }

    debug!(
        "Checked mailbox folders for account ID: {}. Compared local and server folders to identify changes. Elapsed time: {} seconds",
        account.id,
        start_time.elapsed().as_secs()
    );

    let missing_mailboxes = find_missing_mailboxes(local_mailboxes, remote_mailboxes);
    //Mail folders that are not locally need to be downloaded.
    // A throttled round does not attempt them either: each one opens another
    // connection that the server is already refusing.
    if !missing_mailboxes.is_empty() {
        MailBox::batch_insert(&missing_mailboxes)?;

        for mailbox in &missing_mailboxes {
            if summary.throttled {
                break;
            }
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("Received termination signal (User stop or System shutdown)".to_string()),
                )?;
                break;
            }
            if mailbox.exists > 0 {
                let account = account.clone();
                let mailbox = mailbox.clone();

                let _global_permit = match SEMAPHORE.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        error!(
                            "Failed to acquire global semaphore permit for account {} mailbox '{}': {:#?}",
                            account.id, &mailbox.name, err
                        );
                        summary.failed_folders += 1;
                        continue;
                    }
                };

                let result = match &account.date_since {
                    Some(date_since) => {
                        rebuild_mailbox_cache_by_date(
                            &account,
                            mailbox.id,
                            &date_since.since_date()?,
                            &mailbox,
                            FetchDirection::Since,
                            token.clone(),
                        )
                        .await
                    }
                    None => match &account.date_before {
                        Some(r) => {
                            rebuild_mailbox_cache_by_date(
                                &account,
                                mailbox.id,
                                &r.calculate_date()?,
                                &mailbox,
                                FetchDirection::Before,
                                token.clone(),
                            )
                            .await
                        }
                        None => {
                            rebuild_mailbox_cache(&account, &mailbox, &mailbox, token.clone()).await
                        }
                    },
                };

                match result {
                    Ok(new_highest_uid) => {
                        let mut updated = mailbox.clone();
                        updated.highest_uid = new_highest_uid;
                        MailBox::batch_upsert(&[updated])?;
                        summary.synced_folders += 1;
                    }
                    Err(err) => {
                        tracing::error!(mailbox = %mailbox.name, "Folder sync task failed: {:#?}", err);
                        if is_throttle_error(&err) {
                            summary.throttled = true;
                            warn!(
                                account.id,
                                "Server throttled this account; deferring the remaining folders"
                            );
                            break;
                        }
                        summary.failed_folders += 1;
                    }
                }
            } else {
                DownloadState::update_folder_progress(
                    account.id,
                    mailbox.name.clone(),
                    0,
                    0,
                    FolderStatus::Success,
                    None,
                )?;
                // Count it, like the incremental path counts an existing
                // empty folder (#398): a round where one folder failed and one
                // empty folder completed is Partial, not a total failure.
                summary.synced_folders += 1;
            }
        }
    }
    // The caller maps failures to `Partial` (never `Success`): a Success
    // session here would tell the user everything was archived when part of a
    // folder was skipped (#377), and all `Failed` hides that 393 of 394
    // folders did work (#396).
    Ok(summary)
}

//only check new emails and sync
/// Incrementally syncs a mailbox.
/// Returns the new highest UID after sync, or `None` if nothing changed.
async fn perform_incremental_sync(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    if remote_mailbox.exists > 0 {
        // Fast path: `UIDNEXT` comes from the STATUS the round already ran on
        // the listing connection. When it has not passed the local watermark,
        // no message can be new, and the normal path below would connect,
        // EXAMINE and get an empty `UID FETCH start:*` back — one full
        // TLS+LOGIN per unchanged folder. With hundreds of folders that
        // churn is what makes Exchange/Gmail throttle the account (#396).
        if let Some(highest) =
            incremental_skip_target(remote_mailbox.uid_next, local_mailbox.highest_uid)
        {
            debug!(
                account_id = account.id,
                mailbox = %local_mailbox.name,
                highest_uid = highest,
                uid_next = remote_mailbox.uid_next,
                "incremental: UIDNEXT has not passed the watermark, skipping connection"
            );
            DownloadState::update_folder_progress(
                account.id,
                remote_mailbox.name.clone(),
                0,
                0,
                FolderStatus::Success,
                Some("No new emails found.".into()),
            )?;
            return Ok(Some(highest));
        }

        // Use stored highest_uid if available; otherwise fall back to Tantivy
        // query once (backward compatibility with pre-existing databases).
        let start_uid = match local_mailbox.highest_uid {
            Some(uid) => {
                info!(
                    account_id = account.id,
                    mailbox = %local_mailbox.name,
                    highest_uid = uid,
                    "incremental: stored highest_uid"
                );
                uid as u64 + 1
            }
            None => {
                warn!(
                    account_id = account.id,
                    mailbox = %local_mailbox.name,
                    "incremental: no stored highest_uid, falling back to Tantivy get_max_uid"
                );
                let local_max_uid = ENVELOPE_MANAGER.get_max_uid(account.id, local_mailbox.id)?;
                match local_max_uid {
                    Some(uid) => uid + 1,
                    None => {
                        info!(
                            "No maximum UID found in index for mailbox, assuming local storage is missing."
                        );
                        let result = match &account.date_since {
                            Some(date_since) => {
                                fetch_and_save_by_date(
                                    account,
                                    date_since.since_date()?.as_str(),
                                    remote_mailbox,
                                    FetchDirection::Since,
                                    token,
                                )
                                .await?
                            }
                            None => match &account.date_before {
                                Some(r) => {
                                    fetch_and_save_by_date(
                                        account,
                                        &r.calculate_date()?,
                                        remote_mailbox,
                                        FetchDirection::Before,
                                        token,
                                    )
                                    .await?
                                }
                                None => {
                                    fetch_and_save_full_mailbox(account, remote_mailbox, token)
                                        .await?
                                }
                            },
                        };
                        return Ok(result);
                    }
                }
            }
        };

        let mut session = ImapExecutor::create_connection(account.id).await?;
        let before_date = account
            .date_before
            .as_ref()
            .map(|r| r.calculate_date())
            .transpose()?;
        if start_uid == 1 {
            // No stored highest_uid and no indexed messages: fall back to a
            // full mailbox download. Tell the UI up-front so it doesn't show
            // a stale "Pending" while the (possibly large) mailbox streams.
            let _ = DownloadState::update_folder_progress(
                account.id,
                remote_mailbox.name.clone(),
                0,
                0,
                FolderStatus::Downloading,
                Some("Full mailbox download".into()),
            )?;
        }

        let new_max_uid = ImapExecutor::fetch_new_mail(
            &mut session,
            account,
            local_mailbox,
            start_uid,
            before_date.as_deref(),
            token,
        )
        .await?;
        session.logout().await.ok();

        // Keep existing highest_uid if no new mail was fetched.
        Ok(new_max_uid.or(local_mailbox.highest_uid))
    } else {
        DownloadState::update_folder_progress(
            account.id,
            remote_mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
        Ok(local_mailbox.highest_uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::session::SessionStream;
    use std::sync::Arc;
    use tokio_io_timeout::TimeoutStream;

    // ============================================================
    // Pure unit tests (no network)
    // ============================================================

    // ── Retry policy (#396) ────────────────────────────────────────

    #[test]
    fn network_retries_back_off_5_15_30() {
        assert_eq!(retry_backoff_secs(ErrorCode::NetworkError, 1), 5);
        assert_eq!(retry_backoff_secs(ErrorCode::NetworkError, 2), 15);
        assert_eq!(retry_backoff_secs(ErrorCode::NetworkError, 3), 30);
        // Defensive: never index out of bounds, never wait 0.
        assert_eq!(retry_backoff_secs(ErrorCode::NetworkError, 0), 5);
        assert_eq!(retry_backoff_secs(ErrorCode::NetworkError, 99), 30);
    }

    #[test]
    fn throttled_retries_back_off_much_much_longer() {
        assert_eq!(retry_backoff_secs(ErrorCode::ImapServerThrottled, 1), 30);
        assert_eq!(retry_backoff_secs(ErrorCode::ImapServerThrottled, 2), 60);
        assert_eq!(retry_backoff_secs(ErrorCode::ImapServerThrottled, 3), 120);
        assert_eq!(retry_backoff_secs(ErrorCode::ImapServerThrottled, 99), 120);
        // Retrying a throttle is pointless before the server's window resets,
        // so it must always wait longer than a network blip.
        for attempt in 1..=3 {
            assert!(
                retry_backoff_secs(ErrorCode::ImapServerThrottled, attempt)
                    > retry_backoff_secs(ErrorCode::NetworkError, attempt)
            );
        }
    }

    #[test]
    fn only_network_and_throttle_errors_are_retried() {
        assert!(is_retryable_error(ErrorCode::NetworkError));
        assert!(is_retryable_error(ErrorCode::ImapServerThrottled));
        assert!(!is_retryable_error(ErrorCode::ImapAuthenticationFailed));
        assert!(!is_retryable_error(ErrorCode::ImapCommandFailed));
        assert!(!is_retryable_error(ErrorCode::InternalError));
    }

    #[test]
    fn throttle_error_detection_uses_the_error_code() {
        let throttled = raise_error!("x".into(), ErrorCode::ImapServerThrottled);
        let auth = raise_error!("x".into(), ErrorCode::ImapAuthenticationFailed);
        assert!(is_throttle_error(&throttled));
        assert!(!is_throttle_error(&auth));
    }

    // ── UIDNEXT short-circuit (#396) ───────────────────────────────

    #[test]
    fn unchanged_uidnext_skips_the_connection() {
        // UIDNEXT == highest+1: the next assigned UID would be highest+1, so
        // nothing exists above the watermark.
        assert_eq!(incremental_skip_target(Some(151), Some(150)), Some(150));
        // UIDNEXT below the watermark (server reuse/manual fix-up) is also a skip.
        assert_eq!(incremental_skip_target(Some(120), Some(150)), Some(150));
    }

    #[test]
    fn advanced_uidnext_does_not_skip() {
        assert_eq!(incremental_skip_target(Some(152), Some(150)), None);
        assert_eq!(incremental_skip_target(Some(153), Some(150)), None);
    }

    #[test]
    fn missing_inputs_never_skip() {
        // No local watermark yet: the Tantivy fallback must run.
        assert_eq!(incremental_skip_target(Some(151), None), None);
        // Server did not report UIDNEXT: sync normally.
        assert_eq!(incremental_skip_target(None, Some(150)), None);
        assert_eq!(incremental_skip_target(None, None), None);
    }

    // ── Session status mapping (#396) ──────────────────────────────

    #[test]
    fn clean_round_is_success() {
        let (status, message) = session_status_for(&ReconcileSummary {
            synced_folders: 394,
            ..Default::default()
        });
        assert_eq!(status, DownloadStatus::Success);
        assert_eq!(message, None);
    }

    #[test]
    fn one_failed_folder_out_of_many_is_partial_not_failed() {
        let (status, message) = session_status_for(&ReconcileSummary {
            synced_folders: 393,
            failed_folders: 1,
            ..Default::default()
        });
        assert_eq!(
            status,
            DownloadStatus::Partial,
            "393/394 folders is not 'all syncs failed'"
        );
        let message = message.expect("a partial round must explain itself");
        assert!(message.contains("393"), "{message}");
        assert!(message.contains('1'), "{message}");
    }

    #[test]
    fn a_round_where_nothing_worked_is_failed() {
        let (status, _) = session_status_for(&ReconcileSummary {
            synced_folders: 0,
            failed_folders: 3,
            ..Default::default()
        });
        assert_eq!(status, DownloadStatus::Failed);
    }

    #[test]
    fn throttled_round_is_partial_when_something_synced() {
        let (status, message) = session_status_for(&ReconcileSummary {
            synced_folders: 120,
            failed_folders: 0,
            throttled: true,
        });
        assert_eq!(status, DownloadStatus::Partial);
        let message = message.expect("a throttled round must explain itself");
        assert!(message.to_lowercase().contains("throttl"), "{message}");
    }

    #[test]
    fn throttled_round_is_failed_when_nothing_synced() {
        let (status, _) = session_status_for(&ReconcileSummary {
            synced_folders: 0,
            throttled: true,
            ..Default::default()
        });
        assert_eq!(status, DownloadStatus::Failed);
    }

    #[test]
    fn problem_rounds_never_report_success() {
        for &(synced, failed, throttled) in &[
            (0u32, 1u32, false),
            (10, 1, false),
            (0, 0, true),
            (10, 0, true),
            (10, 5, true),
        ] {
            let (status, _) = session_status_for(&ReconcileSummary {
                synced_folders: synced,
                failed_folders: failed,
                throttled,
            });
            assert_ne!(
                status,
                DownloadStatus::Success,
                "synced={synced} failed={failed} throttled={throttled}"
            );
        }
    }

    #[test]
    fn test_generate_synthetic_uidvalidity_deterministic() {
        let a = generate_synthetic_uidvalidity("INBOX");
        let b = generate_synthetic_uidvalidity("INBOX");
        assert_eq!(a, b, "same mailbox name must produce same uid_validity");
    }

    #[test]
    fn test_generate_synthetic_uidvalidity_different_mailboxes() {
        let inbox = generate_synthetic_uidvalidity("INBOX");
        let sent = generate_synthetic_uidvalidity("Sent");
        assert_ne!(
            inbox, sent,
            "different mailboxes should have different uid_validity"
        );
    }

    #[test]
    fn test_generate_synthetic_uidvalidity_non_zero() {
        let uid = generate_synthetic_uidvalidity("INBOX");
        assert_ne!(uid, 0, "UIDVALIDITY should not be 0 (reserved)");
    }

    #[tokio::test]
    async fn test_empty_folder_reconciliation_completes_only_observed_work() {
        // Reuse upstream's per-process test root; account IDs isolate these cases.
        let root = crate::test_support::unique_test_root("bichon-core-test");
        std::env::set_var("BICHON_ROOT_DIR", &root);
        std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
        let _ = &*crate::settings::cli::SETTINGS;

        for (case, existing, cancelled, exists) in [
            (0, false, false, 0),
            (1, true, false, 0),
            (2, false, true, 1),
            (3, true, true, 1),
            (4, true, false, 1),
            (5, false, true, 0),
            (6, true, true, 0),
        ] {
            let account = AccountModel {
                id: 9_002_011 + case,
                ..Default::default()
            };
            let remote = MailBox {
                id: account.id,
                account_id: account.id,
                name: "Observed".into(),
                exists,
                uid_validity: Some(7),
                ..Default::default()
            };
            let local = if existing {
                vec![MailBox {
                    highest_uid: Some(42),
                    ..remote.clone()
                }]
            } else {
                vec![]
            };
            DownloadState::init(account.id).await.unwrap();
            DownloadState::init_folder_details(
                account.id,
                vec![remote.name.clone(), "Unvisited".into()],
            )
            .unwrap();
            for (name, status) in [
                ("Failed", FolderStatus::Failed),
                ("Cancelled", FolderStatus::Cancelled),
            ] {
                DownloadState::update_folder_progress(account.id, name.into(), 0, 0, status, None)
                    .unwrap();
            }
            let token = CancellationToken::new();
            if cancelled {
                token.cancel();
            }
            // The nonempty failure case has no stored account, so connection
            // setup fails locally before any network access.
            let summary = reconcile_mailboxes(&account, &[remote.clone()], &local, token)
                .await
                .unwrap();
            // Since #396 a folder failure is recorded in the summary (which the
            // caller maps to a Partial/Failed session), not returned as an Err,
            // so every case returns Ok.
            assert_eq!(
                summary.failed_folders,
                u32::from(exists > 0 && !cancelled),
                "case {case}"
            );
            assert_eq!(
                summary.synced_folders,
                u32::from(exists == 0 && !cancelled),
                "case {case}"
            );
            let state = DownloadState::get(account.id).unwrap().unwrap();
            let session = if cancelled {
                assert!(state.active_session.is_none());
                let session = state.history.last().unwrap();
                assert_eq!(session.status, DownloadStatus::Cancelled);
                session
            } else {
                state.active_session.as_ref().unwrap()
            };
            let progress = &session.folder_details[&remote.name];
            assert_eq!(
                progress.status,
                if exists == 0 && !cancelled {
                    FolderStatus::Success
                } else {
                    FolderStatus::Pending
                },
                "case {case}, existing={existing}, cancelled={cancelled}"
            );
            assert_eq!(
                (progress.planned, progress.current, &progress.message),
                (0, 0, &None)
            );
            assert_eq!(
                session.folder_details["Unvisited"].status,
                FolderStatus::Pending
            );
            assert_eq!(
                session.folder_details["Failed"].status,
                FolderStatus::Failed
            );
            assert_eq!(
                session.folder_details["Cancelled"].status,
                FolderStatus::Cancelled
            );
            if exists == 0 && !cancelled {
                assert_eq!(
                    MailBox::get(remote.id).unwrap().highest_uid,
                    if existing { Some(42) } else { None }
                );
            } else if !cancelled {
                assert!(!session.errors.is_empty());
            }
        }
    }

    // ============================================================
    // Integration tests (real IMAP server required)
    // ============================================================
    // Fill in your IMAP server details below to run these tests.
    // cargo test -p bichon-core -- --ignored

    const TEST_IMAP_HOST: &str = "imap.zoho.com";
    const TEST_IMAP_PORT: u16 = 993;
    const TEST_IMAP_USERNAME: &str = "";
    const TEST_IMAP_PASSWORD: &str = "";
    const TEST_MAILBOX: &str = "INBOX";

    /// Build a direct IMAP session bypassing the database-dependent
    /// ImapConnectionManager, for local testing with real credentials.
    async fn direct_connect(
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> Result<async_imap::Session<Box<dyn SessionStream>>, String> {
        use rustls::ClientConfig;
        use rustls_pki_types::ServerName;
        use tokio::net::TcpStream;
        use tokio_rustls::TlsConnector;

        // Ensure a rustls crypto provider is installed (ring).
        // May already be installed by production code; ignore duplicate.
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider())
            .ok();

        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|e| format!("TCP connect error: {e}"))?;

        // Wrap in Pin<Box<TimeoutStream<TcpStream>>> to satisfy SessionStream,
        // matching the production path in establish_tcp_connection_with_timeout.
        let timeout_stream = TimeoutStream::new(tcp);
        let pinned = Box::pin(timeout_stream);

        let server_name =
            ServerName::try_from(host.to_owned()).map_err(|e| format!("Invalid hostname: {e}"))?;

        let config = ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.into(),
            })
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(config));
        let tls_stream = connector
            .connect(server_name, pinned)
            .await
            .map_err(|e| format!("TLS error: {e}"))?;

        let client = async_imap::Client::new(Box::new(tls_stream) as Box<dyn SessionStream>);
        let session = client
            .login(username, password)
            .await
            .map_err(|(e, _)| format!("Login error: {e}"))?;

        Ok(session)
    }

    /// Test STATUS UIDVALIDITY directly — verifies your server returns
    /// a valid UIDVALIDITY for the given mailbox.
    #[tokio::test]
    #[ignore = "requires real IMAP credentials"]
    async fn test_status_uid_validity_present() {
        if TEST_IMAP_HOST.is_empty() || TEST_IMAP_USERNAME.is_empty() {
            eprintln!("SKIP: fill in TEST_IMAP_* constants to run this test");
            return;
        }

        let mut session = direct_connect(
            TEST_IMAP_HOST,
            TEST_IMAP_PORT,
            TEST_IMAP_USERNAME,
            TEST_IMAP_PASSWORD,
        )
        .await
        .expect("should connect");

        let status = session
            .status(TEST_MAILBOX, "(UIDVALIDITY)")
            .await
            .expect("STATUS command should succeed");

        session.logout().await.ok();

        match status.uid_validity {
            Some(uid) => {
                println!("[OK] Server returned UIDVALIDITY: {uid}");
                assert_ne!(uid, 0);
            }
            None => {
                println!("[INFO] Server returned no UIDVALIDITY in STATUS response");
                println!("       This server may need the synthetic fallback or the retry logic.");
            }
        }
    }

    /// Test STATUS with full attributes (MESSAGES UNSEEN UIDNEXT UIDVALIDITY),
    /// mimicking the actual listing flow in mailbox::list::fetch_remote_with_progress.
    #[tokio::test]
    #[ignore = "requires real IMAP credentials"]
    async fn test_status_full_attributes() {
        if TEST_IMAP_HOST.is_empty() || TEST_IMAP_USERNAME.is_empty() {
            eprintln!("SKIP: fill in TEST_IMAP_* constants to run this test");
            return;
        }

        let mut session = direct_connect(
            TEST_IMAP_HOST,
            TEST_IMAP_PORT,
            TEST_IMAP_USERNAME,
            TEST_IMAP_PASSWORD,
        )
        .await
        .expect("should connect");

        let status = session
            .status(TEST_MAILBOX, "(MESSAGES UNSEEN UIDNEXT UIDVALIDITY)")
            .await
            .expect("STATUS with full attributes should succeed");

        session.logout().await.ok();

        println!("MESSAGES:    {:?}", status.exists);
        println!("UNSEEN:      {:?}", status.unseen);
        println!("UIDNEXT:     {:?}", status.uid_next);
        println!("UIDVALIDITY: {:?}", status.uid_validity);
    }

    /// Simulate the retry flow: call STATUS multiple times and verify
    /// UIDVALIDITY is consistently returned (or consistently absent).
    #[tokio::test]
    #[ignore = "requires real IMAP credentials"]
    async fn test_uid_validity_consistency_over_multiple_status_calls() {
        if TEST_IMAP_HOST.is_empty() || TEST_IMAP_USERNAME.is_empty() {
            eprintln!("SKIP: fill in TEST_IMAP_* constants to run this test");
            return;
        }

        let results: Vec<Option<u32>> = Vec::with_capacity(5);
        let results = std::cell::RefCell::new(results);

        for i in 0..5 {
            let mut session = direct_connect(
                TEST_IMAP_HOST,
                TEST_IMAP_PORT,
                TEST_IMAP_USERNAME,
                TEST_IMAP_PASSWORD,
            )
            .await
            .expect("should connect");

            let status = session
                .status(TEST_MAILBOX, "(UIDVALIDITY)")
                .await
                .expect("STATUS should succeed");

            session.logout().await.ok();

            println!("Call {}: UIDVALIDITY = {:?}", i + 1, status.uid_validity);
            results.borrow_mut().push(status.uid_validity);
        }

        let results = results.into_inner();
        let first = results[0];
        let all_same = results.iter().all(|r| *r == first);
        if all_same {
            println!("[OK] All 5 STATUS calls returned consistent UIDVALIDITY: {first:?}");
        } else {
            println!("[WARN] Inconsistent UIDVALIDITY across calls: {results:?}");
            println!("       Network jitter or server-side changes detected.");
        }
    }

    /// Test `fetch_uid_metadata` against a real IMAP server.
    /// Fetches a few UIDs from the configured mailbox and verifies
    /// that every returned UID has a valid, non-empty Message-ID.
    #[tokio::test]
    #[ignore = "requires real IMAP credentials"]
    async fn test_fetch_uid_metadata_real_server() {
        if TEST_IMAP_HOST.is_empty() || TEST_IMAP_USERNAME.is_empty() {
            eprintln!("SKIP: fill in TEST_IMAP_* constants to run this test");
            return;
        }

        let mut session = direct_connect(
            TEST_IMAP_HOST,
            TEST_IMAP_PORT,
            TEST_IMAP_USERNAME,
            TEST_IMAP_PASSWORD,
        )
        .await
        .expect("should connect");

        // Examine to enter the mailbox
        session
            .examine(TEST_MAILBOX)
            .await
            .expect("EXAMINE should succeed");

        // Find actual UIDs via SEARCH (don't assume contiguous 1..N)
        let all_uids: Vec<u32> = {
            let mut v: Vec<u32> = session
                .uid_search("ALL")
                .await
                .expect("UID SEARCH ALL should succeed")
                .into_iter()
                .collect();
            v.sort();
            v
        };

        if all_uids.is_empty() {
            eprintln!("SKIP: mailbox is empty, nothing to fetch");
            session.logout().await.ok();
            return;
        }

        // Take up to 5 UIDs for a quick test
        let sample: Vec<u32> = all_uids.into_iter().take(5).collect();
        let uid_set = compress_uid_list(sample.clone());

        let result =
            ImapExecutor::fetch_uid_metadata(&mut session, &uid_set, CancellationToken::new())
                .await
                .expect("fetch_uid_metadata should succeed");

        session.logout().await.ok();

        println!(
            "[OK] Fetched {} UIDs from mailbox '{}':",
            result.len(),
            TEST_MAILBOX
        );
        for uid in sample.iter() {
            let mid = result
                .get(uid)
                .map(|o| o.as_deref().unwrap_or("<none>"))
                .unwrap_or("<missing>");
            println!("  UID {uid}: {mid}");
        }

        // Every UID we requested must be present with a valid Message-ID
        for uid in &sample {
            let msg_id = result
                .get(uid)
                .unwrap_or_else(|| panic!("UID {uid} missing from result"));
            let mid = msg_id
                .as_deref()
                .unwrap_or_else(|| panic!("UID {uid} returned None Message-ID"));
            assert!(!mid.is_empty(), "UID {uid} returned empty Message-ID");
            assert!(
                mid.contains('@'),
                "UID {uid}: Message-ID '{mid}' does not look like a valid Message-ID"
            );
        }
    }

    // ============================================================
    // Unit tests for fetch_uid_validity_with_retry_inner
    // (pure logic, no network needed)
    // ============================================================

    /// Mock helper: returns the given results in sequence, then always None.
    fn mock_results(
        results: Vec<BichonResult<Option<u32>>>,
    ) -> impl FnMut() -> std::future::Ready<BichonResult<Option<u32>>> {
        let mut iter = results.into_iter();
        move || std::future::ready(iter.next().unwrap_or(Ok(None)))
    }

    #[tokio::test]
    async fn test_retry_first_attempt_succeeds() {
        let result = fetch_uid_validity_with_retry_inner(3, mock_results(vec![Ok(Some(42))])).await;
        assert_eq!(result.unwrap(), Some(42));
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_two_nones() {
        // First two attempts return None, third returns Some
        let result = fetch_uid_validity_with_retry_inner(
            3,
            mock_results(vec![Ok(None), Ok(None), Ok(Some(99))]),
        )
        .await;
        assert_eq!(result.unwrap(), Some(99));
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_error_then_none() {
        // Error, then None, then success
        let result = fetch_uid_validity_with_retry_inner(
            3,
            mock_results(vec![
                Err(raise_error!("boom".into(), ErrorCode::NetworkError)),
                Ok(None),
                Ok(Some(7)),
            ]),
        )
        .await;
        assert_eq!(result.unwrap(), Some(7));
    }

    #[tokio::test]
    async fn test_retry_returns_none_after_all_retries_exhausted() {
        // All attempts return None
        let result = fetch_uid_validity_with_retry_inner(
            3,
            mock_results(vec![Ok(None), Ok(None), Ok(None)]),
        )
        .await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn test_retry_returns_none_when_all_attempts_error() {
        // All attempts return Err — still returns Ok(None), not propagating the error
        let result = fetch_uid_validity_with_retry_inner(
            3,
            mock_results(vec![
                Err(raise_error!("e1".into(), ErrorCode::NetworkError)),
                Err(raise_error!("e2".into(), ErrorCode::NetworkError)),
                Err(raise_error!("e3".into(), ErrorCode::NetworkError)),
            ]),
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn test_retry_respects_max_retries() {
        // max_retries=5, success on 5th attempt
        let result = fetch_uid_validity_with_retry_inner(
            5,
            mock_results(vec![Ok(None), Ok(None), Ok(None), Ok(None), Ok(Some(5))]),
        )
        .await;
        assert_eq!(result.unwrap(), Some(5));
    }

    #[tokio::test]
    async fn test_retry_stops_at_max_retries() {
        // max_retries=2, third value is Some but should never be reached
        let result = fetch_uid_validity_with_retry_inner(
            2,
            mock_results(vec![Ok(None), Ok(None), Ok(Some(42))]),
        )
        .await;
        assert_eq!(result.unwrap(), None, "should stop after max_retries (2)");
    }

    #[tokio::test]
    async fn test_retry_max_retries_zero() {
        // max_retries=0 means no attempts at all
        let result = fetch_uid_validity_with_retry_inner(0, mock_results(vec![Ok(Some(42))])).await;
        assert_eq!(result.unwrap(), None);
    }

    // ============================================================
    // Mock IMAP server integration tests
    // ============================================================

    use crate::imap::mock_server::{
        examine_response, minimal_eml, uid_fetch_metadata_response, uid_fetch_rfc822_response,
        MockImapServer, MockImapServerHandle,
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
    async fn fetch_uid_metadata_with_mock_server() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 3, 42, 4))
            .respond(
                "UID FETCH",
                uid_fetch_metadata_response(&[
                    (1, "<msg-a@test.com>"),
                    (2, "<msg-b@test.com>"),
                    (3, "<msg-c@test.com>"),
                ]),
            )
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let result =
            ImapExecutor::fetch_uid_metadata(&mut session, "1:3", CancellationToken::new())
                .await
                .unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result.get(&1).unwrap().as_deref(), Some("msg-a@test.com"));
        assert_eq!(result.get(&2).unwrap().as_deref(), Some("msg-b@test.com"));
        assert_eq!(result.get(&3).unwrap().as_deref(), Some("msg-c@test.com"));

        session.logout().await.ok();
    }

    /// UIDVALIDITY reconciliation fetches full headers: folded and
    /// lower/upper-case Message-ID headers must still yield an id, otherwise
    /// the whole mailbox is treated as missing and re-downloaded (#368).
    #[tokio::test]
    async fn fetch_uid_metadata_folded_and_case_variant_message_ids() {
        let folded = b"message-id:\r\n <folded-a@test.example>\r\n\r\n";
        let upper = b"MESSAGE-ID: <upper-b@test.example>\r\n\r\n";
        let mut response = Vec::new();
        for (uid, header) in [(1u32, &folded[..]), (2u32, &upper[..])] {
            response.extend_from_slice(
                format!(
                    "* {uid} FETCH (UID {uid} BODY[HEADER] {{{}}}\r\n",
                    header.len()
                )
                .as_bytes(),
            );
            response.extend_from_slice(header);
            response.extend_from_slice(b")\r\n");
        }
        response.extend_from_slice(b"{TAG} OK FETCH completed\r\n");

        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 2, 42, 3))
            .respond("UID FETCH", response)
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let result =
            ImapExecutor::fetch_uid_metadata(&mut session, "1:2", CancellationToken::new())
                .await
                .unwrap();

        assert_eq!(result.get(&1).unwrap().as_deref(), Some("folded-a@test.example"));
        assert_eq!(result.get(&2).unwrap().as_deref(), Some("upper-b@test.example"));

        session.logout().await.ok();
    }

    /// Verify basic UID FETCH (FLAGS only, no body) works with the mock server.
    #[tokio::test]
    async fn fetch_uid_flags_with_mock_server() {
        // Response with just UID and FLAGS — no body literal parsing needed.
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 2, 42, 3))
            .respond(
                "UID FETCH",
                b"* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n\
* 2 FETCH (UID 2 FLAGS (\\Flagged))\r\n\
{TAG} OK FETCH completed\r\n"
                    .to_vec(),
            )
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let uids: Vec<u32> = {
            let mut stream = session.uid_fetch("1:2", "(UID FLAGS)").await.unwrap();

            use futures::TryStreamExt;
            let mut uids = Vec::new();
            while let Some(fetch) = stream.try_next().await.unwrap() {
                uids.push(fetch.uid.unwrap_or(0));
            }
            uids
        };

        assert_eq!(uids, vec![1, 2]);

        session.logout().await.ok();
    }

    /// Verify UID FETCH with BODY[] (full RFC822) works with the mock server.
    #[tokio::test]
    async fn fetch_uid_rfc822_with_mock_server() {
        let eml = minimal_eml("Test Subject", "test@example.com");
        let eml_len = eml.len();

        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 1, 42, 2))
            .respond("UID FETCH", uid_fetch_rfc822_response(1, &eml))
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let bodies: Vec<(u32, Vec<u8>)> = {
            let mut stream = session.uid_fetch("1:1", "(UID BODY[])").await.unwrap();

            use futures::TryStreamExt;
            let mut bodies = Vec::new();
            while let Some(fetch) = stream.try_next().await.unwrap() {
                let uid = fetch.uid.unwrap_or(0);
                let body = fetch.body().map(|b| b.to_vec()).unwrap_or_default();
                bodies.push((uid, body));
            }
            bodies
        };

        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].0, 1);
        assert_eq!(bodies[0].1.len(), eml_len);
        assert!(String::from_utf8_lossy(&bodies[0].1).contains("Test Subject"));

        session.logout().await.ok();
    }

    #[tokio::test]
    async fn fetch_uid_metadata_empty_mailbox() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 0, 42, 1))
            .respond("UID FETCH", b"{TAG} OK FETCH completed\r\n".to_vec())
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let result =
            ImapExecutor::fetch_uid_metadata(&mut session, "1:*", CancellationToken::new())
                .await
                .unwrap();

        assert!(result.is_empty(), "empty mailbox should return empty map");

        session.logout().await.ok();
    }

    #[tokio::test]
    async fn fetch_uid_metadata_missing_message_id() {
        // One entry has a Message-ID, the other has no header at all.
        let header_with_msgid = "From: sender@example.com\r\n\
             Date: Thu, 01 Jan 2025 00:00:00 +0000\r\n\
             Message-ID: <ok@test.com>\r\n\r\n";
        let header_without_msgid = "\r\n";
        let len1 = header_with_msgid.len();
        let len2 = header_without_msgid.len();

        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN done\r\n")
            .respond("EXAMINE", examine_response("INBOX", 2, 42, 3))
            .respond(
                "UID FETCH",
                format!(
                    "* 1 FETCH (UID 1 BODY[HEADER] {{{len1}}}\r\n\
{header_with_msgid})\r\n\
* 2 FETCH (UID 2 BODY[HEADER] {{{len2}}}\r\n\
{header_without_msgid})\r\n\
{{TAG}} OK FETCH completed\r\n"
                )
                .into_bytes(),
            )
            .start()
            .await;

        let mut session = mock_session(&handle).await;

        let result =
            ImapExecutor::fetch_uid_metadata(&mut session, "1:2", CancellationToken::new())
                .await
                .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(&1).unwrap().as_deref(), Some("ok@test.com"));
        // UID 2 has no Message-ID header → None
        assert_eq!(result.get(&2).unwrap().as_deref(), None);

        session.logout().await.ok();
    }
}
