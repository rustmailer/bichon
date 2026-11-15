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
        migration::{AccountModel, ArchiveRules},
        state::{DownloadState, GapFillFolderStats, GapFillState},
    },
    archive::imap::{
        download::flow::{retry_backoff_secs, should_retry_network, MAX_NETWORK_RETRIES},
        mailbox::MailBox,
        tombstones,
    },
    envelope::extractor::should_archive_from_headers,
    error::BichonResult,
    imap::{
        executor::{
            classify_imap_error, compress_uid_list, poll_with_stall_reports, ImapExecutor,
            StallLogger, DEFAULT_BATCH_SIZE, STALL_REPORT_INTERVAL,
        },
        session::SessionStream,
    },
    store::tantivy::envelope::EnvelopeSnapshot,
};

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

/// Cumulative progress of the header-enumeration phase across batches.
///
/// `fetch_uid_headers` reports how many headers the *current attempt* has
/// received and restarts that count after a retry, so the headers of finished
/// batches are kept as a fixed base. A batch snapshots the base when it
/// starts; repeated stall reports and retries therefore cannot count the same
/// header twice, and a successful batch advances the base exactly once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderScanProgress {
    completed: u64,
    total: u64,
}

impl HeaderScanProgress {
    fn new(total: u64) -> Self {
        Self { completed: 0, total }
    }

    fn total(&self) -> u64 {
        self.total
    }

    /// Cumulative value to show while the current batch has received
    /// `attempt_count` headers.
    fn reported(&self, attempt_count: u64) -> u64 {
        self.completed + attempt_count
    }

    /// Closes a successful batch that produced `headers` headers and returns
    /// the cumulative value to show for it.
    fn complete_batch(&mut self, headers: u64) -> u64 {
        self.completed += headers;
        self.completed
    }
}

/// Reports one header-enumeration progress point for the active gap-fill run.
fn report_header_progress(
    account_id: u64,
    folder_name: &str,
    total: u64,
    scanned: u64,
    stall_secs: Option<f64>,
) -> BichonResult<()> {
    GapFillState::update_folder_progress(
        account_id,
        folder_name.to_string(),
        total,
        scanned,
        crate::imap::executor::slow_server_message(None, stall_secs),
    )
}

/// Which remote uids are missing locally. A remote message is "present" if its
/// message-id exists locally. The (size, internal_date) fingerprint is a
/// fallback for every remote message — not only those without a message-id —
/// because some paths store a different message-id locally than the remote
/// header carries (e.g. the SMTP path generates a random one), and servers
/// like Zoho/163 reuse the same message-id across different messages. A
/// duplicated local message-id still counts as present: re-downloading would
/// be deduplicated away anyway, so it can never repair the duplication.
pub fn compute_missing_uids(remote: &[RemoteHeader], local: &[EnvelopeSnapshot]) -> Vec<u32> {
    let mut local_by_msg_id: HashMap<&str, usize> = HashMap::new();
    let mut local_by_fingerprint: HashSet<(u64, i64)> = HashSet::new();
    for snap in local {
        if !snap.message_id.is_empty() {
            *local_by_msg_id.entry(snap.message_id.as_str()).or_insert(0) += 1;
        }
        local_by_fingerprint.insert((snap.size, snap.internal_date));
    }

    let mut missing = Vec::new();
    for header in remote {
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

/// Decide whether one missing candidate should be downloaded.
///
/// `headers` is the raw header block fetched for the candidate. When headers
/// were not needed (only a size limit is configured) or could not be fetched,
/// `None` lets `should_archive` apply the rules it can evaluate without them;
/// sender/subject rules then stay unenforced and the body path decides after a
/// full parse. A header block that cannot be parsed is treated the same way
/// (`unwrap_or(true)`), so broken mail keeps its previous behaviour.
fn candidate_should_download(
    rules: &ArchiveRules,
    headers: Option<&[u8]>,
    size: u64,
) -> bool {
    let size = size.min(u32::MAX as u64) as u32;
    match headers {
        Some(headers) => should_archive_from_headers(rules, headers, size).unwrap_or(true),
        None => rules.should_archive(None, None, size, false),
    }
}

/// Human-readable summary of a gap-fill folder outcome.
///
/// Archive-rule skips are deliberately not included: they are expected and
/// surfaced as a structured counter, not as an amber warning next to failures.
fn summary_message(parse_skipped: u64, failed: u64) -> Option<String> {
    let mut notes = Vec::new();
    if parse_skipped > 0 {
        notes.push(format!(
            "{parse_skipped} email(s) could not be parsed and were skipped (not archived); \
             they will be re-detected on the next gap-fill run"
        ));
    } else if failed > 0 {
        notes.push(format!(
            "{failed} email(s) could not be compared or downloaded; re-run gap-fill to retry them"
        ));
    }
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("; "))
    }
}

/// Advance the mailbox's `highest_uid` watermark after a scan that covered
/// every candidate (downloaded or filtered by archive rules) without failures.
fn advance_highest_uid(
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    missing_uids: &[u32],
) -> BichonResult<()> {
    if let Some(max_uid) = missing_uids.last().copied() {
        let mut updated = remote_mailbox.clone();
        updated.highest_uid = Some(max_uid.max(local_mailbox.highest_uid.unwrap_or(0)));
        MailBox::batch_upsert(&[updated])?;
    }
    Ok(())
}

/// The IMAP session a gap-fill run shares across folders.
///
/// Gap-fill used to open **two** connections per mailbox (one for the header
/// scan, one for the downloads) and log in again for every folder. On an
/// account with hundreds of folders that is hundreds of logins per run —
/// exactly what makes a server rate-limit, after which it answers `EXAMINE`
/// with silence until the socket times out (#396). The run now keeps one
/// session alive, re-selects the mailbox per folder (skipped when it is
/// already selected) and rebuilds the session only after an error.
pub(crate) struct GapFillConnection {
    account_id: u64,
    session: Option<async_imap::Session<Box<dyn SessionStream>>>,
    /// Mailbox the live session is currently examining.
    selected: Option<String>,
}

impl GapFillConnection {
    pub(crate) fn new(account_id: u64) -> Self {
        Self {
            account_id,
            session: None,
            selected: None,
        }
    }

    /// The live session, reconnecting when the previous one was invalidated.
    async fn session(&mut self) -> BichonResult<&mut async_imap::Session<Box<dyn SessionStream>>> {
        if self.session.is_none() {
            self.session = Some(ImapExecutor::create_connection(self.account_id).await?);
            self.selected = None;
        }
        Ok(self.session.as_mut().expect("session was just created"))
    }

    /// Drop the session and its selection after an error that may have
    /// desynchronized or killed it. The next use reconnects.
    fn invalidate(&mut self) {
        self.session = None;
        self.selected = None;
    }

    /// Whether the live session is already positioned on `mailbox` — in which
    /// case `open_mailbox` may skip the `EXAMINE`. The `session.is_some()`
    /// check is load-bearing: a cleared session must never inherit the old
    /// selection, or a fresh connection would skip the `EXAMINE` it needs.
    fn is_selected(&self, mailbox: &str) -> bool {
        self.session.is_some() && self.selected.as_deref() == Some(mailbox)
    }

    /// Reconnect once and select `mailbox`, without a retry loop of its own.
    /// The batch-level retry paths already own the retry count, so they call
    /// this; the folder-level entry point is [`Self::open_mailbox`].
    async fn reconnect(
        &mut self,
        mailbox: &str,
        token: &tokio_util::sync::CancellationToken,
    ) -> BichonResult<()> {
        self.invalidate();
        let account_id = self.account_id;
        let result = match self.session().await {
            Ok(session) => {
                // Selecting a mailbox is silent on a throttled server until
                // the socket times out; report the wait instead of leaving the
                // UI frozen on the previous folder.
                let selected =
                    poll_with_stall_reports(
                        session.examine(mailbox),
                        STALL_REPORT_INTERVAL,
                        token, |stall_secs| {
                        let _ = GapFillState::update_folder_progress(
                            account_id,
                            mailbox.to_string(),
                            0,
                            0,
                            crate::imap::executor::slow_server_message(
                                None,
                                Some(stall_secs as f64),
                            ),
                        );
                    })
                    .await;
                match selected {
                    Some(res) => res
                        .map(|_| ())
                        .map_err(|e| raise_error!(format!("{:#?}", e), classify_imap_error(&e))),
                    None => Err(raise_error!(
                        "Mailbox selection cancelled".to_string(),
                        ErrorCode::InternalError
                    )),
                }
            }
            Err(e) => Err(e),
        };
        match result {
            Ok(()) => {
                self.selected = Some(mailbox.to_string());
                Ok(())
            }
            Err(e) => {
                // A failed EXAMINE leaves the session unusable.
                self.invalidate();
                Err(e)
            }
        }
    }

    /// Select `mailbox`, reusing the live session when it is already there.
    ///
    /// A transient failure (timeout, throttling, reset) is retried on a fresh
    /// connection with the shared backoff. Without this one timeout failed the
    /// whole folder before a single header was fetched — the failure mode in
    /// the #396 logs, where `EXAMINE` timed out on seven mailboxes in a row.
    pub(crate) async fn open_mailbox(
        &mut self,
        mailbox: &str,
        token: &tokio_util::sync::CancellationToken,
    ) -> BichonResult<()> {
        if self.is_selected(mailbox) {
            return Ok(());
        }
        let mut retries = 0u32;
        loop {
            match self.reconnect(mailbox, token).await {
                Ok(()) => return Ok(()),
                Err(e) if should_retry_network(e.code(), retries, token.is_cancelled()) => {
                    retries += 1;
                    let backoff = retry_backoff_secs(e.code(), retries);
                    warn!(
                        account_id = self.account_id,
                        mailbox,
                        retries,
                        "Gap-fill: cannot select mailbox, reconnecting in {}s ({}/{})",
                        backoff,
                        retries,
                        MAX_NETWORK_RETRIES
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Log the run's single session out once, at the end of the run.
    pub(crate) async fn close(mut self) {
        if let Some(mut session) = self.session.take() {
            session.logout().await.ok();
        }
    }
}

/// Runs the gap-fill phase for one mailbox: enumerates every remote UID,
/// diffs against local envelopes, downloads the missing ones and returns the
/// per-folder outcome. Handles per-batch network errors by counting them as
/// failed (retryable on a later gap-fill run) instead of aborting the phase.
pub(crate) async fn gap_fill_mailbox(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    token: tokio_util::sync::CancellationToken,
    conn: &mut GapFillConnection,
) -> BichonResult<GapFillFolderStats> {
    let account_id = account.id;
    let mut stats = GapFillFolderStats::default();
    let encoded_name = remote_mailbox.encoded_name();

    // Reuse the run's session (one connection for every folder) and select
    // this mailbox. A transient EXAMINE timeout is retried here instead of
    // failing the folder outright.
    conn.open_mailbox(&encoded_name, &token).await?;

    // Phase 1: enumerate every remote UID. A huge mailbox can make the server
    // take a while to answer `UID SEARCH ALL`; keep the UI informed instead of
    // appearing stuck (the socket read timeout is the final backstop).
    // Build the search future ONCE and keep polling it: the stall-report
    // loop used to construct a fresh `uid_search("ALL")` future on every
    // 5 s timeout, dropping the in-flight command (desyncing the session
    // and restarting the server's work each time), so any mailbox whose
    // search takes longer than one stall interval could never finish (#373).
    // The block scope drops the pinned future before the session is reused
    // for the next command — it holds the `&mut Session` borrow.
    let search_outcome = {
        let session = conn.session().await?;
        let mut stall_logger = StallLogger::default();
        let mailbox_name = remote_mailbox.name.clone();
        poll_with_stall_reports(
            session.uid_search("ALL"),
            STALL_REPORT_INTERVAL,
            &token, move |stall_secs| {
            let _ = GapFillState::update_folder_progress(
                account_id,
                mailbox_name.clone(),
                0,
                0,
                crate::imap::executor::slow_server_message(None, Some(stall_secs as f64)),
            );
            if stall_logger.should_log(std::time::Instant::now()) {
                tracing::warn!(
                    account_id,
                    mailbox = %mailbox_name,
                    stall_secs,
                    "gap-fill: UID SEARCH ALL taking long, still waiting"
                );
            } else {
                tracing::debug!(
                    account_id,
                    mailbox = %mailbox_name,
                    stall_secs,
                    "gap-fill: UID SEARCH ALL taking long"
                );
            }
        })
        .await
    };
    let results = match search_outcome {
        Some(Ok(results)) => results,
        Some(Err(e)) => {
            // The session may be desynchronized after a failed search; rebuild
            // it for the next folder instead of reusing it.
            conn.invalidate();
            return Err(raise_error!(format!("{:#?}", e), classify_imap_error(&e)));
        }
        None => {
            // The in-flight search was dropped on cancellation; do not reuse a
            // session that may be desynchronized.
            conn.invalidate();
            return Ok(stats);
        }
    };
    let mut remote_uids: Vec<u32> = results.into_iter().collect();
    remote_uids.sort();
    if remote_uids.is_empty() {
        return Ok(stats);
    }

    // Phase 2: fetch header metadata for all remote uids in batches.
    // A failed batch counts its uids as failed (they cannot be diffed) but
    // does not abort the phase. A cancellation, however, leaves the header
    // list incomplete so the diff would be unreliable — return immediately.
    let mut remote_headers: Vec<RemoteHeader> = Vec::with_capacity(remote_uids.len());
    let batch_size = account.download_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
    let mut cancelled = false;
    let mut header_progress_state = HeaderScanProgress::new(remote_uids.len() as u64);
    for chunk in remote_uids.chunks(batch_size) {
        if token.is_cancelled() {
            cancelled = true;
            break;
        }
        let seq_set = compress_uid_list(chunk.to_vec());
        // FETCH reports a count for this attempt only. Snapshot the base of
        // completed batches so repeated stall reports and retries cannot count
        // the same headers twice.
        let batch_progress = header_progress_state;
        let header_progress = move |count: u64, stall_secs: Option<f64>| {
            report_header_progress(
                account_id,
                &remote_mailbox.name,
                batch_progress.total(),
                batch_progress.reported(count),
                stall_secs,
            )
        };
        // A network error mid-phase used to leave the (now dead) session in
        // place: every later chunk silently returned no rows, so most of the
        // folder was never compared while the run reported a tiny (or zero)
        // candidate count. Reconnect and retry the chunk, the same way the
        // download phase does, before giving up on it.
        let mut header_retries = 0u32;
        let header_result = loop {
            let attempt = match conn.session().await {
                Ok(session) => {
                    ImapExecutor::fetch_uid_headers(
                        session,
                        &seq_set,
                        token.clone(),
                        Some(&header_progress),
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            match attempt {
                Ok(headers) => break Ok(headers),
                Err(e)
                    if should_retry_network(e.code(), header_retries, token.is_cancelled()) =>
                {
                    let code = e.code();
                    header_retries += 1;
                    warn!(
                        account_id,
                        mailbox = remote_mailbox.name,
                        retries = header_retries,
                        "Gap-fill: {} on header batch, reconnecting ({}/{})",
                        if code == ErrorCode::ImapServerThrottled {
                            "server throttling"
                        } else {
                            "network error"
                        },
                        header_retries,
                        MAX_NETWORK_RETRIES
                    );
                    if let Err(e2) = conn.reconnect(&encoded_name, &token).await {
                        let err_msg = format!(
                            "Gap-fill: re-examine failed after reconnect: {:#?}",
                            e2
                        );
                        let _ = DownloadState::append_session_error(account_id, err_msg);
                        break Err(e);
                    }
                    let backoff = retry_backoff_secs(code, header_retries);
                    warn!(
                        account_id,
                        mailbox = remote_mailbox.name,
                        "Gap-fill: backing off {}s before retrying header batch",
                        backoff
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    continue;
                }
                Err(e) => {
                    // Never leave a wedged session behind for the next folder.
                    conn.invalidate();
                    break Err(e);
                }
            }
        };
        match header_result {
            Ok(headers) => {
                // Fast batches may never trigger the stall callback, so report
                // the batch's completion explicitly and advance the base once.
                let scanned = header_progress_state.complete_batch(headers.len() as u64);
                report_header_progress(
                    account_id,
                    &remote_mailbox.name,
                    header_progress_state.total(),
                    scanned,
                    None,
                )?;
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
        GapFillState::update_folder_progress(account_id, remote_mailbox.name.clone(), 0, 0, None)?;
        return Ok(stats);
    }
    remote_headers.sort_by_key(|h| h.uid);

    // Phase 3: local snapshot
    let local_snapshots = crate::store::tantivy::envelope::ENVELOPE_MANAGER
        .get_envelope_snapshots_for_mailbox(account_id, local_mailbox.id)?;

    // Phase 3.5: drop tombstones for UIDs the server no longer has. Only
    // when every header batch succeeded: a partial enumeration would make us
    // forget tombstones that are still needed and re-download those messages.
    if stats.failed == 0 {
        if let Some(uid_validity) = local_mailbox.uid_validity {
            let remote_set: HashSet<u32> = remote_headers.iter().map(|h| h.uid).collect();
            tombstones::retain_remote_uids(
                account_id,
                local_mailbox.id,
                uid_validity,
                &remote_set,
            )
            .await?;
        }
    }

    // Phase 4: diff. Locally deleted messages are recorded as tombstones and
    // must not be re-downloaded; this is what keeps deletion permanent.
    let missing_uids =
        tombstones::filter_uids(account_id, local_mailbox.id, local_mailbox.uid_validity, compute_missing_uids(&remote_headers, &local_snapshots))?;
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

    // Re-select only if the header phase had to reconnect; otherwise the same
    // session the header scan used also carries the body downloads.
    conn.open_mailbox(&encoded_name, &token).await?;

    // Phase 4.5: apply archive rules to the candidates from their headers,
    // before any body is downloaded. A message the filter rejects is never
    // fetched, so gap-fill does not keep re-downloading mail it would discard.
    let remote_sizes: HashMap<u32, u64> =
        remote_headers.iter().map(|h| (h.uid, h.size)).collect();
    let archive_rules = account.archive_rules.as_ref().filter(|rules| rules.enabled);
    let needs_headers = archive_rules
        .map(|rules| {
            !rules.senders.is_empty()
                || !rules.subjects.is_empty()
                || !rules.spam_headers.is_empty()
        })
        .unwrap_or(false);

    let mut filtered = 0u64;
    let mut to_download: Vec<u32> = Vec::with_capacity(missing_uids.len());
    if let Some(rules) = archive_rules {
        if needs_headers {
            GapFillState::update_folder_progress(
                account_id,
                remote_mailbox.name.clone(),
                planned,
                0,
                Some("Checking archive rules before downloading...".into()),
            )?;
            for chunk in missing_uids.chunks(batch_size) {
                if token.is_cancelled() {
                    stats.filtered = filtered;
                    GapFillState::update_folder_progress_and_filtered(
                        account_id,
                        remote_mailbox.name.clone(),
                        0,
                        0,
                        filtered,
                        None,
                    )?;
                    return Ok(stats);
                }
                let seq_set = compress_uid_list(chunk.to_vec());
                let attempt = match conn.session().await {
                    Ok(session) => {
                        ImapExecutor::fetch_uid_header_blocks(session, &seq_set, token.clone())
                            .await
                    }
                    Err(e) => Err(e),
                };
                match attempt {
                    Ok(headers) => {
                        let headers: HashMap<u32, Vec<u8>> = headers.into_iter().collect();
                        for uid in chunk {
                            let size = remote_sizes.get(uid).copied().unwrap_or(0);
                            if candidate_should_download(
                                rules,
                                headers.get(uid).map(|h| h.as_slice()),
                                size,
                            ) {
                                to_download.push(*uid);
                            } else {
                                crate::metrics::count_email_ingested(account_id, "imap", "filtered");
                                filtered += 1;
                            }
                        }
                    }
                    Err(e) => {
                        if token.is_cancelled() {
                            stats.filtered = filtered;
                            GapFillState::update_folder_progress_and_filtered(
                                account_id,
                                remote_mailbox.name.clone(),
                                0,
                                0,
                                filtered,
                                None,
                            )?;
                            return Ok(stats);
                        }
                        // Header pre-filtering is an optimization. If it fails,
                        // download the candidates and let the normal post-parse
                        // path apply the rules.
                        warn!(
                            account_id,
                            mailbox = remote_mailbox.name,
                            "Gap-fill: header pre-filter failed, downloading candidates: {:#?}",
                            e
                        );
                        to_download.extend_from_slice(chunk);
                        // A failed FETCH can leave the session desynchronized;
                        // rebuild it before the body phase.
                        conn.invalidate();
                        conn.open_mailbox(&encoded_name, &token).await?;
                    }
                }
            }
        } else {
            for uid in &missing_uids {
                let size = remote_sizes.get(uid).copied().unwrap_or(0);
                if candidate_should_download(rules, None, size) {
                    to_download.push(*uid);
                } else {
                    crate::metrics::count_email_ingested(account_id, "imap", "filtered");
                    filtered += 1;
                }
            }
        }
        GapFillState::set_folder_filtered(account_id, remote_mailbox.name.clone(), filtered)?;
    } else {
        to_download = missing_uids.clone();
    }

    if to_download.is_empty() {
        info!(
            account_id,
            mailbox = remote_mailbox.name,
            filtered,
            "Gap-fill: all missing emails were skipped by archive rules"
        );
        stats.filtered = filtered;
        // Phase-2 header failures still need their warning even when every
        // candidate that could be diffed was filtered out.
        stats.message = summary_message(0, stats.failed);
        if stats.failed == 0 {
            // Every candidate was skipped by the rules, so nothing is missing
            // locally: advance the watermark exactly as a fully downloaded
            // folder would, otherwise the same candidates stay "missing" and
            // the next incremental sync may fall back to a full re-download.
            advance_highest_uid(local_mailbox, remote_mailbox, &missing_uids)?;
        }
        GapFillState::update_folder_progress_and_filtered(
            account_id,
            remote_mailbox.name.clone(),
            planned,
            0,
            filtered,
            None,
        )?;
        return Ok(stats);
    }

    let batches = crate::imap::executor::generate_uid_sequence_hashset(to_download, batch_size);
    let mut downloaded = 0u64;
    let mut failed = 0u64;
    // Messages skipped because they could not be parsed (not archived): they
    // were candidates that did not land, so they count as failed — that keeps
    // the watermark from advancing and guarantees the next gap-fill run
    // re-detects them (#377).
    let mut parse_skipped = 0u64;
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
        let last_filtered = std::sync::Mutex::new(0u64);
        let batch_result = loop {
            let progress = |cumulative: u64, cumulative_filtered: u64, avg_secs: Option<f64>, stall_secs: Option<f64>| {
                *last_reported.lock().unwrap() = cumulative;
                *last_filtered.lock().unwrap() = cumulative_filtered;
                let _ = GapFillState::update_folder_progress(
                    account_id,
                    remote_mailbox.name.clone(),
                    planned,
                    downloaded + cumulative,
                    crate::imap::executor::slow_server_message(avg_secs, stall_secs),
                );
                Ok(())
            };
            let attempt = match conn.session().await {
                Ok(session) => ImapExecutor::uid_batch_retrieve_emails(
                    session,
                    account_id,
                    remote_mailbox.id,
                    &batch.0,
                    account.max_email_size_bytes,
                    token.clone(),
                    Some(&progress),
                )
                .await,
                Err(e) => Err(e),
            };
            match attempt {
                Ok(outcome) => break Ok(outcome),
                Err(e) if should_retry_network(e.code(), retries, token.is_cancelled()) => {
                    let code = e.code();
                    retries += 1;
                    warn!(
                        account_id,
                        mailbox = remote_mailbox.name,
                        index,
                        retries,
                        "Gap-fill: {} on batch, reconnecting ({}/{})",
                        if code == ErrorCode::ImapServerThrottled {
                            "server throttling"
                        } else {
                            "network error"
                        },
                        retries,
                        MAX_NETWORK_RETRIES
                    );
                    if let Err(e2) = conn.reconnect(&encoded_name, &token).await {
                        let err_msg = format!(
                            "Gap-fill: re-examine failed after reconnect: {:#?}",
                            e2
                        );
                        DownloadState::append_session_error(account_id, err_msg)?;
                        break Err(e);
                    }
                    // A throttled server needs much longer than a transient
                    // network blip before it will accept the next command.
                    let backoff = retry_backoff_secs(code, retries);
                    warn!(
                        account_id,
                        mailbox = remote_mailbox.name,
                        "Gap-fill: backing off {}s before retrying batch",
                        backoff
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    continue;
                }
                Err(e) => {
                    conn.invalidate();
                    break Err(e);
                }
            }
        };
        match batch_result {
            Ok(outcome) => {
                // `stored` is the processed count; a message the filter
                // rejected in the fallback path was not archived.
                downloaded += outcome.stored.saturating_sub(outcome.filtered);
                parse_skipped += outcome.parse_skipped;
                filtered += outcome.filtered;
                GapFillState::update_folder_progress_and_filtered(
                    account_id,
                    remote_mailbox.name.clone(),
                    planned,
                    downloaded,
                    filtered,
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
                let batch_filtered = *last_filtered.lock().unwrap();
                downloaded += processed.saturating_sub(batch_filtered);
                if batch_filtered > 0 {
                    filtered += batch_filtered;
                    GapFillState::set_folder_filtered(
                        account_id,
                        remote_mailbox.name.clone(),
                        filtered,
                    )?;
                }
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
    stats.downloaded = downloaded;
    // Accumulate rather than overwrite: phase 2 (header batch) failures already
    // counted into stats.failed and must survive alongside phase 5 failures.
    // Parse-skipped messages count as failed too — they are never stored, so
    // every later gap-fill run re-detects them (and warns about them again).
    stats.failed += failed + parse_skipped;
    stats.filtered = filtered;
    // Surface the skips and the incomplete header enumeration in the folder's
    // final message so the gap-fill running state carries a visible warning.
    stats.message = summary_message(parse_skipped, stats.failed);
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
    if !cancelled && downloaded + filtered == planned && stats.failed == 0 {
        advance_highest_uid(local_mailbox, remote_mailbox, &missing_uids)?;
    }

    Ok(stats)
}

#[cfg(test)]
mod test {
    use super::*;

    /// A cleared session must never inherit the previous selection: a fresh
    /// connection has not been selected on any mailbox, so reusing the old
    /// name would skip the `EXAMINE` — and then issue `FETCH`es against no
    /// mailbox at all.
    #[test]
    fn invalidating_a_connection_clears_its_selection() {
        let mut conn = GapFillConnection::new(7);
        conn.selected = Some("INBOX".to_string());

        // A selection without a live session is not usable.
        assert!(!conn.is_selected("INBOX"));

        conn.invalidate();
        assert!(
            conn.selected.is_none(),
            "invalidate() must forget the selected mailbox"
        );
        assert!(!conn.is_selected("INBOX"));
        assert!(!conn.is_selected("Sent"));
    }

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
        let missing = compute_missing_uids(&remote, &local);
        assert_eq!(missing, vec![2]);
    }

    #[test]
    fn compute_missing_uids_fingerprint_fallback() {
        let remote = vec![rh(1, None, 10, 1000), rh(2, None, 20, 2000)];
        let local = vec![snap("generated-x", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local);
        assert_eq!(missing, vec![2]);
    }

    #[test]
    fn compute_missing_uids_remote_duplicates_all_present() {
        let remote = vec![rh(1, Some("dup"), 10, 1000), rh(2, Some("dup"), 10, 1000)];
        let local = vec![snap("dup", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local);
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
        let missing = compute_missing_uids(&remote, &local);
        assert!(missing.is_empty());
    }

    #[test]
    fn compute_missing_uids_msgid_mismatch_but_fingerprint_hit() {
        // The remote message-id does not exist locally (e.g. a different
        // message-id was stored by the SMTP path) but the fingerprint matches:
        // the message is already stored and must NOT be re-downloaded.
        let remote = vec![rh(1, Some("remote-id@x.com"), 10, 1000)];
        let local = vec![snap("generated-random-id", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local);
        assert!(missing.is_empty());
    }

    #[test]
    fn compute_missing_uids_msgid_mismatch_and_fingerprint_miss() {
        let remote = vec![
            rh(1, Some("remote-id@x.com"), 10, 1000),
            rh(2, Some("remote-id-2@x.com"), 20, 2000),
        ];
        let local = vec![snap("generated-random-id", 1, 10, 1000)];
        let missing = compute_missing_uids(&remote, &local);
        assert_eq!(missing, vec![2]);
    }

    fn header_block(from: &str, subject: &str, spam: bool) -> Vec<u8> {
        let mut headers = format!("From: {from}\r\nSubject: {subject}\r\n");
        if spam {
            headers.push_str("X-Spam-Flag: YES\r\n");
        }
        headers.push_str("\r\n");
        headers.into_bytes()
    }

    fn rules_with_sender_exclude(pattern: &str) -> ArchiveRules {
        ArchiveRules {
            enabled: true,
            senders: crate::account::migration::FilterRule {
                exclude: vec![pattern.into()],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn candidate_sender_exclude_skips_before_download() {
        let rules = rules_with_sender_exclude(r".*@news\.example\.com$");
        let skipped = header_block("News <weekly@news.example.com>", "Weekly digest", false);
        let kept = header_block("Alice <alice@ok.example>", "Hello", false);
        assert!(!candidate_should_download(&rules, Some(&skipped), 100));
        assert!(candidate_should_download(&rules, Some(&kept), 100));
    }

    #[test]
    fn candidate_decodes_subject_before_matching() {
        // "发票" encoded per RFC 2047.
        let rules = ArchiveRules {
            enabled: true,
            subjects: crate::account::migration::FilterRule {
                exclude: vec!["发票".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let skipped = header_block(
            "billing@shop.example",
            "=?UTF-8?B?5Y+R56Wo?=",
            false,
        );
        let kept = header_block("alice@ok.example", "Meeting", false);
        assert!(!candidate_should_download(&rules, Some(&skipped), 100));
        assert!(candidate_should_download(&rules, Some(&kept), 100));
    }

    #[test]
    fn candidate_spam_header_and_size_are_applied() {
        let spam_rules = ArchiveRules {
            enabled: true,
            spam_headers: vec!["X-Spam-Flag".into()],
            ..Default::default()
        };
        assert!(!candidate_should_download(
            &spam_rules,
            Some(&header_block("a@b.example", "Hi", true)),
            100
        ));
        assert!(candidate_should_download(
            &spam_rules,
            Some(&header_block("a@b.example", "Hi", false)),
            100
        ));

        // Size-only rules do not need headers at all.
        let size_rules = ArchiveRules {
            enabled: true,
            skip_larger_than: Some(100),
            ..Default::default()
        };
        assert!(!candidate_should_download(&size_rules, None, 101));
        assert!(candidate_should_download(&size_rules, None, 100));
    }

    #[test]
    fn candidate_missing_headers_falls_back_to_download() {
        // Sender rules cannot be evaluated without headers; the body path must
        // make the decision after a full parse.
        let rules = rules_with_sender_exclude(r".*@news\.example\.com$");
        assert!(candidate_should_download(&rules, None, 100));
    }

    #[test]
    fn summary_reports_only_anomalies() {
        assert_eq!(summary_message(0, 0), None);
        assert!(summary_message(2, 0).unwrap().contains("could not be parsed"));
        assert!(summary_message(0, 3).unwrap().contains("could not be compared"));
    }

    #[test]
    fn header_scan_progress_accumulates_across_fast_batches() {
        // Fast batches never trigger the stall callback, so the value reported
        // when each batch completes is the only progress the UI sees. It must
        // be cumulative across the whole scan: reporting only the current
        // batch left the bar pinned at `batch_size / total`.
        let mut progress = HeaderScanProgress::new(100);
        assert_eq!(progress.complete_batch(30), 30);
        assert_eq!(progress.complete_batch(30), 60);
        assert_eq!(progress.complete_batch(40), 100);
        assert_eq!(progress.total(), 100);
    }

    #[test]
    fn header_scan_progress_stall_reports_use_the_batch_base() {
        // Within a batch the server re-reports its attempt count every few
        // seconds; each report adds that count to the fixed base of finished
        // batches, so the value keeps advancing without counting anything twice.
        let mut progress = HeaderScanProgress::new(60);
        assert_eq!(progress.complete_batch(30), 30);

        let batch = progress;
        assert_eq!(batch.reported(5), 35);
        assert_eq!(batch.reported(5), 35, "repeated stall reports must not add up");
        assert_eq!(batch.reported(12), 42);
        assert_eq!(batch.reported(0), 30, "a retry restarts the attempt count");
    }

    #[test]
    fn header_scan_progress_retry_is_counted_once() {
        // A retried batch restarts its attempt count at 0 while the base stays
        // put, so the headers of the successful attempt are added exactly once.
        let mut progress = HeaderScanProgress::new(60);
        assert_eq!(progress.complete_batch(30), 30);

        let retried_batch = progress;
        assert_eq!(retried_batch.reported(29), 59, "first attempt died late");
        assert_eq!(retried_batch.reported(0), 30, "retry starts over from zero");
        assert_eq!(progress.complete_batch(30), 60);
    }

    #[test]
    fn header_scan_progress_failed_batch_does_not_advance_the_base() {
        // A failed batch never reaches `complete_batch` (its headers are not
        // added to the remote side), so later reports continue from the same
        // base instead of skipping over headers that were never received.
        let mut progress = HeaderScanProgress::new(90);
        assert_eq!(progress.complete_batch(30), 30);
        // Second batch fails: no `complete_batch` call.
        assert_eq!(progress.reported(0), 30);
        assert_eq!(progress.reported(30), 60);
    }
}
