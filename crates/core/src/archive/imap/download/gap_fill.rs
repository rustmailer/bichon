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

use std::collections::{HashMap, HashSet};

use tracing::{info, warn};

use crate::error::code::ErrorCode;
use crate::raise_error;
use crate::{
    account::{
        migration::AccountModel,
        state::{DownloadState, GapFillFolderStats, GapFillState},
    },
    archive::imap::mailbox::MailBox,
    error::BichonResult,
    imap::executor::{compress_uid_list, ImapExecutor, DEFAULT_BATCH_SIZE},
    store::tantivy::envelope::EnvelopeSnapshot,
};

/// Number of times a batch download is retried with a fresh connection before
/// it is counted as failed (mirrors the incremental sync path).
const MAX_NETWORK_RETRIES: u32 = 3;

/// Lightweight header metadata for one remote message, fetched via
/// `FETCH (UID RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteHeader {
    pub uid: u32,
    pub message_id: Option<String>,
    pub size: u64,
    /// Epoch millis (internal date).
    pub internal_date: i64,
}

/// Which remote uids are missing locally. A remote message is "present" if its
/// message-id exists locally. The (size, internal_date) fingerprint is a
/// fallback for every remote message — not only those without a message-id —
/// because some paths store a different message-id locally than the remote
/// header carries (e.g. the SMTP path generates a random one), and servers
/// like Zoho/163 reuse the same message-id across different messages. A
/// duplicated local message-id still counts as present: re-downloading would
/// be deduplicated away anyway, so it can never repair the duplication.
///
/// With `match_uids` (local and remote UIDVALIDITY are equal) a remote UID
/// that is already stored locally is present regardless of its headers, so a
/// header that fails to parse can never re-download archived mail (#368).
pub fn compute_missing_uids(
    remote: &[RemoteHeader],
    local: &[EnvelopeSnapshot],
    match_uids: bool,
) -> Vec<u32> {
    let mut local_by_msg_id: HashMap<&str, usize> = HashMap::new();
    let mut local_by_fingerprint: HashSet<(u64, i64)> = HashSet::new();
    let local_uids: HashSet<u64> = if match_uids {
        local.iter().map(|s| s.uid).filter(|&u| u > 0).collect()
    } else {
        HashSet::new()
    };
    for snap in local {
        if !snap.message_id.is_empty() {
            *local_by_msg_id.entry(snap.message_id.as_str()).or_insert(0) += 1;
        }
        local_by_fingerprint.insert((snap.size, snap.internal_date));
    }

    let mut missing = Vec::new();
    for header in remote {
        if local_uids.contains(&(header.uid as u64)) {
            continue;
        }
        let present = match &header.message_id {
            Some(msg_id) => local_by_msg_id
                .get(msg_id.as_str())
                .is_some_and(|&c| c > 0),
            None => false,
        };
        if !present {
            // Fingerprint fallback for every remote message, not just those
            // without a message-id: the remote message-id may not exist
            // locally even though the message is already stored (random
            // synthetic ids on the SMTP path).
            let fp_present = local_by_fingerprint.contains(&(header.size, header.internal_date));
            if !fp_present {
                missing.push(header.uid);
            }
        }
    }
    missing
}

/// Runs the gap-fill phase for one mailbox: enumerates every remote UID,
/// diffs against local envelopes, downloads the missing ones and returns the
/// per-folder outcome. Handles per-batch network errors by counting them as
/// failed (retryable on a later gap-fill run) instead of aborting the phase.
pub async fn gap_fill_mailbox(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    token: tokio_util::sync::CancellationToken,
) -> BichonResult<GapFillFolderStats> {
    let account_id = account.id;
    let mut stats = GapFillFolderStats::default();

    let mut session = ImapExecutor::create_connection(account_id).await?;
    let examined = session
        .examine(&remote_mailbox.encoded_name())
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    // Local UIDs are only comparable while UIDVALIDITY is unchanged; a change
    // is handled by the reconcile path in flow.rs.
    let match_uids =
        examined.uid_validity.is_some() && examined.uid_validity == local_mailbox.uid_validity;

    // Phase 1: enumerate every remote UID. A huge mailbox can make the server
    // take a while to answer `UID SEARCH ALL`; keep the UI informed instead of
    // appearing stuck (the socket read timeout is the final backstop).
    let search_started = std::time::Instant::now();
    let results = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), session.uid_search("ALL"))
            .await
        {
            Ok(res) => {
                break res
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            }
            Err(_) => {
                let stall = search_started.elapsed().as_secs_f64();
                if token.is_cancelled() {
                    session.logout().await.ok();
                    return Ok(stats);
                }
                let _ = GapFillState::update_folder_progress(
                    account_id,
                    remote_mailbox.name.clone(),
                    0,
                    0,
                    crate::imap::executor::slow_server_message(None, Some(stall)),
                );
                tracing::warn!(
                    account_id,
                    mailbox = %remote_mailbox.name,
                    stall_secs = format!("{:.0}", stall),
                    "gap-fill: UID SEARCH ALL taking long, still waiting"
                );
            }
        }
    };
    let mut remote_uids: Vec<u32> = results.into_iter().collect();
    remote_uids.sort();
    if remote_uids.is_empty() {
        session.logout().await.ok();
        return Ok(stats);
    }

    // Phase 2: fetch header metadata for all remote uids in batches.
    // A failed batch counts its uids as failed (they cannot be diffed) but
    // does not abort the phase. A cancellation, however, leaves the header
    // list incomplete so the diff would be unreliable — return immediately.
    let mut remote_headers: Vec<RemoteHeader> = Vec::with_capacity(remote_uids.len());
    let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
    let mut cancelled = false;
    // Progress reported so far across all header batches, so the UI can show
    // enumeration progress (and slow-server hints) while a huge mailbox is
    // being scanned.
    let headers_fetched = std::sync::Mutex::new(0u64);
    for chunk in remote_uids.chunks(batch_size) {
        if token.is_cancelled() {
            cancelled = true;
            break;
        }
        let seq_set = compress_uid_list(chunk.to_vec());
        match ImapExecutor::fetch_uid_headers(
            &mut session,
            &seq_set,
            token.clone(),
            Some(&|count, stall_secs| {
                *headers_fetched.lock().unwrap() = count;
                GapFillState::update_folder_progress(
                    account_id,
                    remote_mailbox.name.clone(),
                    remote_uids.len() as u64,
                    count,
                    crate::imap::executor::slow_server_message(None, stall_secs),
                )
            }),
        )
        .await
        {
            Ok(headers) => {
                *headers_fetched.lock().unwrap() += headers.len() as u64;
                remote_headers.extend(headers);
            }
            Err(e) => {
                // Count the whole chunk as failed; the user can re-run
                // gap-fill to retry. Do not abort the phase.
                stats.failed += chunk.len() as u64;
                let err_msg = format!("Gap-fill header batch failed: {:#?}", e);
                warn!(account_id, mailbox = remote_mailbox.name, "{}", err_msg);
                let _ = DownloadState::append_session_error(account_id, err_msg);
            }
        }
    }
    if cancelled {
        session.logout().await.ok();
        GapFillState::update_folder_progress(account_id, remote_mailbox.name.clone(), 0, 0, None)?;
        return Ok(stats);
    }
    remote_headers.sort_by_key(|h| h.uid);
    session.logout().await.ok();

    // Phase 3: local snapshot
    let local_snapshots = crate::store::tantivy::envelope::ENVELOPE_MANAGER
        .get_envelope_snapshots_for_mailbox(account_id, local_mailbox.id)?;

    // Phase 4: diff
    let missing_uids = compute_missing_uids(&remote_headers, &local_snapshots, match_uids);
    stats.candidate_count = missing_uids.len() as u64;
    if missing_uids.is_empty() {
        info!(
            account_id,
            mailbox = remote_mailbox.name,
            "Gap-fill: no missing emails"
        );
        GapFillState::update_folder_progress(account_id, remote_mailbox.name.clone(), 0, 0, None)?;
        return Ok(stats);
    }

    // Phase 5: download missing in batches
    let planned = missing_uids.len() as u64;
    GapFillState::update_folder_progress(
        account_id,
        remote_mailbox.name.clone(),
        planned,
        0,
        None,
    )?;

    let mut session2 = ImapExecutor::create_connection(account_id).await?;
    session2
        .examine(&remote_mailbox.encoded_name())
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    let batches =
        crate::imap::executor::generate_uid_sequence_hashset(missing_uids.clone(), batch_size);
    let mut downloaded = 0u64;
    let mut failed = 0u64;
    let mut cancelled = false;
    for (index, batch) in batches.into_iter().enumerate() {
        if token.is_cancelled() {
            cancelled = true;
            break;
        }
        // A slow server can stall a batch past the socket read timeout, same
        // as in the incremental path. Retry such batches on a fresh connection
        // instead of counting them as failed outright.
        let mut retries = 0u32;
        // Tracks the last cumulative count the progress callback reported. When
        // a batch fails mid-stream the executor reports the already-stored
        // count one final time before returning the error, so this is the
        // number of emails of this batch that actually made it to disk.
        let last_reported = std::sync::Mutex::new(0u64);
        let batch_result = loop {
            match ImapExecutor::uid_batch_retrieve_emails(
                &mut session2,
                account_id,
                remote_mailbox.id,
                &batch.0,
                account.max_email_size_bytes,
                token.clone(),
                Some(&|cumulative, avg_secs, stall_secs| {
                    *last_reported.lock().unwrap() = cumulative;
                    let _ = GapFillState::update_folder_progress(
                        account_id,
                        remote_mailbox.name.clone(),
                        planned,
                        downloaded + cumulative,
                        crate::imap::executor::slow_server_message(avg_secs, stall_secs),
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
                        mailbox = remote_mailbox.name,
                        index,
                        retries,
                        "Gap-fill: network error on batch, reconnecting ({}/{})",
                        retries,
                        MAX_NETWORK_RETRIES
                    );
                    match ImapExecutor::create_connection(account_id).await {
                        Ok(new_session) => {
                            session2 = new_session;
                            if let Err(e2) = session2.examine(&remote_mailbox.encoded_name()).await
                            {
                                let err_msg = format!(
                                    "Gap-fill: re-examine failed after reconnect: {:#?}",
                                    e2
                                );
                                DownloadState::append_session_error(account_id, err_msg)?;
                                break Err(e);
                            }
                            // Longer backoff than the original 1s/2s/4s: a
                            // throttling server needs time to recover.
                            let backoff = [5u64, 15, 30][(retries - 1) as usize];
                            warn!(
                                account_id,
                                mailbox = remote_mailbox.name,
                                "Gap-fill: backing off {}s before retrying batch",
                                backoff
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                            continue;
                        }
                        Err(e2) => {
                            tracing::error!(account_id, "Gap-fill: reconnection failed: {:#?}", e2);
                            break Err(e);
                        }
                    }
                }
                Err(e) => break Err(e),
            }
        };
        match batch_result {
            Ok(processed) => {
                downloaded += processed;
                GapFillState::update_folder_progress(
                    account_id,
                    remote_mailbox.name.clone(),
                    planned,
                    downloaded,
                    None,
                )?;
            }
            Err(e) => {
                // The batch may have partially succeeded: emails already stored
                // before the failure are counted as downloaded, only the rest
                // of the batch is failed. The user can re-run gap-fill to
                // retry the remainder; dedup makes re-downloading the stored
                // ones harmless. Do not abort the phase.
                let processed = *last_reported.lock().unwrap();
                downloaded += processed;
                let remaining = batch.1.saturating_sub(processed);
                failed += remaining;
                let err_msg = format!(
                    "Gap-fill batch {} failed after {} processed: {:#?}",
                    index, processed, e
                );
                warn!(account_id, mailbox = remote_mailbox.name, "{}", err_msg);
                let _ = DownloadState::append_session_error(account_id, err_msg);
            }
        }
    }
    session2.logout().await.ok();

    stats.downloaded = downloaded;
    // Accumulate rather than overwrite: phase 2 (header batch) failures already
    // counted into stats.failed and must survive alongside phase 5 failures.
    stats.failed += failed;
    // Final progress write into the independent gap-fill state (the folder is
    // done; the outcome lands in GapFillRun.folders via add_folder_result).
    GapFillState::update_folder_progress(
        account_id,
        remote_mailbox.name.clone(),
        planned,
        downloaded,
        None,
    )?;

    // Advance the mailbox's highest_uid so subsequent incremental syncs
    // start after the newly downloaded messages. Only do this on a complete,
    // uncancelled run where every planned message was downloaded and nothing
    // failed (phase-2 header batch failures leave uids outside `planned` that
    // must still be picked up by a later gap-fill run).
    if !cancelled && downloaded == planned && stats.failed == 0 {
        if let Some(max_uid) = missing_uids.last().copied() {
            let mut updated = remote_mailbox.clone();
            updated.highest_uid = Some(max_uid.max(local_mailbox.highest_uid.unwrap_or(0)));
            crate::archive::imap::mailbox::MailBox::batch_upsert(&[updated])?;
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod test {
    use super::*;

    fn rh(uid: u32, message_id: Option<&str>, size: u64, internal_date: i64) -> RemoteHeader {
        RemoteHeader {
            uid,
            message_id: message_id.map(|s| s.to_string()),
            size,
            internal_date,
        }
    }

    fn snap(message_id: &str, uid: u64, size: u64, internal_date: i64) -> EnvelopeSnapshot {
        EnvelopeSnapshot {
            message_id: message_id.to_string(),
            uid,
            size,
            internal_date,
            //subject: String::new(),
        }
    }

    #[test]
    fn compute_missing_uids_message_id_diff() {
        let remote = vec![
            rh(1, Some("a"), 10, 1000),
            rh(2, Some("b"), 20, 2000),
            rh(3, Some("c"), 30, 3000),
        ];
        let local = vec![snap("a", 1, 10, 1000), snap("c", 3, 30, 3000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert_eq!(missing, vec![2]);
    }

    #[test]
    fn compute_missing_uids_fingerprint_fallback() {
        let remote = vec![rh(1, None, 10, 1000), rh(2, None, 20, 2000)];
        let local = vec![snap("generated-x", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert_eq!(missing, vec![2]);
    }

    #[test]
    fn compute_missing_uids_remote_duplicates_all_present() {
        let remote = vec![rh(1, Some("dup"), 10, 1000), rh(2, Some("dup"), 10, 1000)];
        let local = vec![snap("dup", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert!(missing.is_empty());
    }

    #[test]
    fn compute_missing_uids_local_duplicate_not_re_downloaded() {
        // A message-id appearing more than once locally is NOT a reason to
        // re-download: servers (Zoho, 163) legitimately reuse message-ids
        // across different messages, and re-downloading would be deduplicated
        // away anyway, so it can never repair the duplication.
        let remote = vec![rh(1, Some("dup"), 10, 1000), rh(2, Some("dup"), 10, 1000)];
        let local = vec![snap("dup", 1, 10, 1000), snap("dup", 2, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert!(missing.is_empty());
    }

    #[test]
    fn compute_missing_uids_msgid_mismatch_but_fingerprint_hit() {
        // The remote message-id does not exist locally (e.g. a different
        // message-id was stored by the SMTP path) but the fingerprint matches:
        // the message is already stored and must NOT be re-downloaded.
        let remote = vec![rh(1, Some("remote-id@x.com"), 10, 1000)];
        let local = vec![snap("generated-random-id", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert!(missing.is_empty());
    }

    #[test]
    fn compute_missing_uids_msgid_mismatch_and_fingerprint_miss() {
        let remote = vec![
            rh(1, Some("remote-id@x.com"), 10, 1000),
            rh(2, Some("remote-id-2@x.com"), 20, 2000),
        ];
        let local = vec![snap("generated-random-id", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local, false);
        assert_eq!(missing, vec![2]);
    }

    #[test]
    fn compute_missing_uids_same_uid_is_present_when_uidvalidity_matches() {
        // #368: the remote Message-ID failed to parse (folded header) and the
        // fingerprint differs, but the UID is already archived.
        let remote = vec![rh(7, None, 11, 1000), rh(8, None, 22, 2000)];
        let local = vec![snap("a@b", 7, 10, 999)];
        assert_eq!(compute_missing_uids(&remote, &local, true), vec![8]);
        assert_eq!(compute_missing_uids(&remote, &local, false), vec![7, 8]);
    }

    #[test]
    fn compute_missing_uids_uid_zero_never_matches() {
        // Imported mail is stored with uid 0 and must not mask remote uid 0.
        let remote = vec![rh(0, None, 1, 1)];
        let local = vec![snap("x", 0, 2, 2)];
        assert_eq!(compute_missing_uids(&remote, &local, true), vec![0]);
    }
}
