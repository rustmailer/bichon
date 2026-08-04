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
        cache::{
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
        imap::uidonly_acquisition::{
            account_requires_uidonly, connect_and_acquire_or_legacy,
            mailbox_has_uidonly_proof, AcquisitionRoute,
        },
        store::tantivy::envelope::ENVELOPE_MANAGER,
    },
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

const MAX_NETWORK_RETRIES: u32 = 3;

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
    if account_requires_uidonly(account) || mailbox.uidonly_source_scope.is_some() {
        return Err(raise_error!(
            "Date-scoped acquisition is unsafe on a UIDONLY-limited server".into(),
            ErrorCode::Incompatible
        ));
    }
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
                Some(&|cumulative, avg_secs, stall_secs| {
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
                Ok((processed, _throttled)) => break Ok(processed),
                Err(e) if retries < MAX_NETWORK_RETRIES && e.code() == ErrorCode::NetworkError => {
                    retries += 1;
                    warn!(
                        account_id,
                        mailbox = mailbox.name,
                        index,
                        retries,
                        "Network error on batch, reconnecting ({}/{})",
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
                            // Longer backoff than the original 1s/2s/4s: a
                            // throttling server needs time to recover.
                            let backoff = [5u64, 15, 30][(retries - 1) as usize];
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
            Ok(processed) => {
                current_processed += processed;
                DownloadState::update_folder_progress(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
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
    Ok(max_uid)
}

/// Fetches all messages from a mailbox.
/// Returns a checkpoint only after the complete mailbox run succeeds.
/// The legacy route enumerates `UID SEARCH ALL` first, so new arrivals cannot
/// shift messages between batches while the download is running.
pub async fn fetch_and_save_full_mailbox(
    account: &AccountModel,
    mailbox: &MailBox,
    force_uidonly: bool,
    token: CancellationToken,
) -> BichonResult<MailBox> {
    let mailbox_id = mailbox.id;
    let account_id = account.id;

    // Classify the actual acquisition connection. Cached capabilities route
    // known-limited accounts early, but can never authorize a legacy fetch.
    let route = connect_and_acquire_or_legacy(
        account,
        mailbox,
        force_uidonly,
        token.clone(),
        |progress| {
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                progress.planned,
                progress.resolved,
                FolderStatus::Downloading,
                None,
            )
        },
    )
    .await;
    let mut session = match route {
        Ok(AcquisitionRoute::Acquired {
            report,
            source_scope,
        }) => {
            DownloadState::update_folder_progress(
                account_id,
                mailbox.name.clone(),
                report.inventoried,
                report.archived,
                FolderStatus::Success,
                None,
            )?;
            let mut updated = mailbox.clone();
            updated.highest_uid = report.checkpoint;
            updated.uid_validity = Some(report.uid_validity);
            updated.uid_next = Some(report.uid_next);
            updated.exists = report.exists;
            updated.uidonly_source_scope = Some(source_scope);
            return Ok(updated);
        }
        Ok(AcquisitionRoute::Legacy(session)) => session,
        Err(e) => {
            let err_msg = format!("Full mailbox acquisition failed: {:#?}", e);
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

    let uid_list =
        match ImapExecutor::uid_search_all_mailbox(&mut session, &mailbox.encoded_name()).await {
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
        let mut updated = mailbox.clone();
        updated.highest_uid = None;
        updated.uidonly_source_scope = None;
        return Ok(updated);
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
            session.logout().await.ok();
            return Err(raise_error!(
                "Full mailbox download cancelled".into(),
                ErrorCode::InternalError
            ));
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
                Some(&|cumulative, avg_secs, stall_secs| {
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
                Ok((count, _throttled)) => break Ok(count),
                Err(e) if retries < MAX_NETWORK_RETRIES && e.code() == ErrorCode::NetworkError => {
                    retries += 1;
                    warn!(
                        account_id,
                        mailbox = mailbox.name,
                        index,
                        retries,
                        "Network error on batch, reconnecting ({}/{})",
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
                            // Longer backoff than the original 1s/2s/4s: a
                            // throttling server needs time to recover.
                            let backoff = [5u64, 15, 30][(retries - 1) as usize];
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
            Ok(count) => {
                current_processed += count as u64;
                DownloadState::update_folder_progress(
                    account_id,
                    mailbox.name.clone(),
                    planned,
                    current_processed,
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
                session.logout().await.ok();
                return Err(e);
            }
        }
    }

    DownloadState::update_folder_progress(
        account_id,
        mailbox.name.clone(),
        planned,
        current_processed,
        FolderStatus::Success,
        None,
    )?;
    session.logout().await.ok();
    let mut updated = mailbox.clone();
    updated.highest_uid = Some(max_uid);
    updated.uidonly_source_scope = None;
    Ok(updated)
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

    // Phase 2: collect remote UIDs, respecting date constraints
    let remote_uid_list: Vec<u32> = if let Some(date_since) = &account.date_since {
        let date = date_since.since_date()?;
        let results = session
            .uid_search(&format!("SINCE {date}"))
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut v: Vec<u32> = results.into_iter().collect();
        v.sort();
        v
    } else if let Some(date_before) = &account.date_before {
        let date = date_before.calculate_date()?;
        let results = session
            .uid_search(&format!("BEFORE {date}"))
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut v: Vec<u32> = results.into_iter().collect();
        v.sort();
        v
    } else {
        let results = session
            .uid_search("ALL")
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
                Some(&|cumulative, avg_secs, stall_secs| {
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
                Ok((processed, _throttled)) => {
                    downloaded += processed;
                    DownloadState::update_folder_progress(
                        account_id,
                        remote_mailbox.name.clone(),
                        planned,
                        downloaded,
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

fn mailbox_source_changed(account: &AccountModel, mailbox: &MailBox) -> bool {
    mailbox.uidonly_source_scope.is_some() && !mailbox_has_uidonly_proof(account, mailbox)
}

fn intersecting_source_changed(account: &AccountModel, mailboxes: &[(MailBox, MailBox)]) -> bool {
    mailboxes
        .iter()
        .any(|(local, _)| mailbox_source_changed(account, local))
}

pub async fn reconcile_mailboxes(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    local_mailboxes: &[MailBox],
    token: CancellationToken,
) -> BichonResult<()> {
    let existing_mailboxes = find_intersecting_mailboxes(local_mailboxes, remote_mailboxes);
    let source_changed = intersecting_source_changed(account, &existing_mailboxes);
    let known_limited = local_mailboxes
        .iter()
        .any(|mailbox| mailbox_has_uidonly_proof(account, mailbox))
        || account_requires_uidonly(account);
    if (known_limited || source_changed)
        && (account.date_since.is_some() || account.date_before.is_some())
    {
        return Err(raise_error!(
            "Date-scoped acquisition is unsafe on a UIDONLY-limited server".into(),
            ErrorCode::Incompatible
        ));
    }
    let start_time = Instant::now();
    let account_id = account.id;
    if !existing_mailboxes.is_empty() {
        let mut mailboxes_to_update = Vec::with_capacity(existing_mailboxes.len());

        DownloadState::init_folder_details(
            account.id,
            remote_mailboxes.iter().map(|m| m.name.clone()).collect(),
        )?;

        for (local_mailbox, remote_mailbox) in &existing_mailboxes {
            let source_changed = mailbox_source_changed(account, local_mailbox);
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("Received termination signal (User stop or System shutdown)".to_string()),
                )?;
                break;
            }

            if account.date_since.is_none()
                && account.date_before.is_none()
                && (known_limited || source_changed)
            {
                let can_resume = local_mailbox.uid_validity == remote_mailbox.uid_validity
                    && !source_changed
                    && mailbox_has_uidonly_proof(account, local_mailbox);
                let mut acquisition_mailbox = (*remote_mailbox).clone();
                if can_resume {
                    acquisition_mailbox.highest_uid = local_mailbox.highest_uid;
                    acquisition_mailbox.uidonly_source_scope =
                        local_mailbox.uidonly_source_scope.clone();
                }
                let result = fetch_and_save_full_mailbox(
                    account,
                    &acquisition_mailbox,
                    known_limited,
                    token.clone(),
                )
                .await?;
                mailboxes_to_update.push(result);
                continue;
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
                    .await?
            } else {
                perform_incremental_sync(account, local_mailbox, remote_mailbox, token.clone())
                    .await?
            };
            let mut updated = remote_mailbox.clone();
            updated.highest_uid = new_highest_uid;
            // Update uid_validity with the resolved value (either from server or synthetic)
            if updated.uid_validity.is_none() {
                updated.uid_validity = Some(remote_uid_validity);
            }
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
    if !missing_mailboxes.is_empty() {
        MailBox::batch_insert(&missing_mailboxes)?;

        let mut has_error = false;
        let mut last_err = None;
        for mailbox in &missing_mailboxes {
            if token.is_cancelled() {
                DownloadState::update_session_status(
                    account.id,
                    DownloadStatus::Cancelled,
                    Some("Received termination signal (User stop or System shutdown)".to_string()),
                )?;
                break;
            }
            if mailbox.exists > 0
                || (account.date_since.is_none()
                    && account.date_before.is_none()
                    && (known_limited || account_requires_uidonly(account)))
            {
                let account = account.clone();
                let mailbox = mailbox.clone();

                let _global_permit = match SEMAPHORE.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        error!(
                            "Failed to acquire global semaphore permit for account {} mailbox '{}': {:#?}",
                            account.id, &mailbox.name, err
                        );
                        continue;
                    }
                };

                let result = match &account.date_since {
                    Some(date_since) => rebuild_mailbox_cache_by_date(
                        &account,
                        mailbox.id,
                        &date_since.since_date()?,
                        &mailbox,
                        FetchDirection::Since,
                        token.clone(),
                    )
                    .await
                    .map(|highest_uid| {
                        let mut updated = mailbox.clone();
                        updated.highest_uid = highest_uid;
                        updated
                    }),
                    None => match &account.date_before {
                        Some(r) => rebuild_mailbox_cache_by_date(
                            &account,
                            mailbox.id,
                            &r.calculate_date()?,
                            &mailbox,
                            FetchDirection::Before,
                            token.clone(),
                        )
                        .await
                        .map(|highest_uid| {
                            let mut updated = mailbox.clone();
                            updated.highest_uid = highest_uid;
                            updated
                        }),
                        None => {
                            rebuild_mailbox_cache(
                                &account,
                                &mailbox,
                                &mailbox,
                                known_limited,
                                token.clone(),
                            )
                            .await
                        }
                    },
                };

                match result {
                    Ok(updated) => MailBox::batch_upsert(&[updated])?,
                    Err(err) => {
                        has_error = true;
                        tracing::error!("Folder sync task failed: {:#?}", err);
                        last_err = Some(err);
                    }
                }
            }
        }

        if has_error {
            if let Some(e) = last_err {
                return Err(e);
            }
            return Err(raise_error!(
                "Some tasks failed".into(),
                ErrorCode::InternalError
            ));
        }
    }
    Ok(())
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
        // Use stored highest_uid if available; otherwise fall back to Tantivy
        // query once (backward compatibility with pre-existing databases).
        let start_uid = match local_mailbox.highest_uid {
            Some(uid) => uid as u64 + 1,
            None => {
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
                                    fetch_and_save_full_mailbox(
                                        account,
                                        remote_mailbox,
                                        false,
                                        token,
                                    )
                                    .await?
                                    .highest_uid
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

    #[test]
    fn stale_nonremote_uidonly_scope_does_not_force_current_mailbox_rescan() {
        let local = [
            MailBox {
                name: "INBOX".into(),
                ..Default::default()
            },
            MailBox {
                name: "Unsubscribed".into(),
                uidonly_source_scope: Some("old-source".into()),
                ..Default::default()
            },
        ];
        let remote = [MailBox {
            name: "INBOX".into(),
            ..Default::default()
        }];
        let current = find_intersecting_mailboxes(&local, &remote);
        assert!(mailbox_source_changed(&AccountModel::default(), &local[1]));
        assert!(!intersecting_source_changed(
            &AccountModel::default(),
            &current
        ));
    }

    #[tokio::test]
    async fn known_limited_server_rejects_date_scoped_fallback_before_connecting() {
        let account = AccountModel {
            capabilities: Some(vec!["UIDONLY".into(), "MESSAGELIMIT=10000".into()]),
            ..Default::default()
        };
        assert!(fetch_and_save_by_date(
            &account,
            "2026-01-01",
            &MailBox::default(),
            FetchDirection::Since,
            CancellationToken::new(),
        )
        .await
        .is_err());
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
