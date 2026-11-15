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
    account::{
        migration::{AccountModel, AccountType},
        state::{
            DownloadState, DownloadStatus, GapFillFolderStats, GapFillState, GapFillStatus,
            TriggerType,
        },
    },
    archive::imap::{download::flow::FetchDirection, mailbox::MailBox},
    backup::gate::WRITE_GATE,
    error::{code::ErrorCode, BichonResult},
    imap::executor::ImapExecutor,
};
use download_folders::get_download_folders;
use download_type::{decide_next_download_task, DownloadTask};
use flow::reconcile_mailboxes;
use rebuild::{rebuild_cache, rebuild_cache_by_date};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub mod download_folders;
pub mod download_type;
pub mod gap_fill;
pub mod flow;
pub mod rebuild;

pub async fn process_imap_download(
    account: &AccountModel,
    token: CancellationToken,
    trigger_type: TriggerType,
    run_gap_fill: bool,
) -> BichonResult<()> {
    assert_eq!(account.account_type, AccountType::IMAP);
    let start_time = Instant::now();
    let account_id = account.id;

    // Skip a round entirely while a backup window is open: downloads would
    // repeatedly hit the paused write gate and error on every envelope.
    if WRITE_GATE.is_paused() {
        debug!(account_id = account_id, "IMAP sync skipped: backup in progress");
        return Ok(());
    }
    let download_task = decide_next_download_task(account, trigger_type).await?;
    if matches!(download_task, DownloadTask::Idle) {
        return Ok(());
    }
    // Connect and list the mailboxes, retrying transient failures. A single
    // LOGIN or LIST timeout used to mark the whole round Failed (#396), even
    // though a fresh connection usually succeeds right away — on a throttled
    // server that is exactly what the #396 logs showed.
    let remote_mailboxes = match flow::retry_network_op(
        || async {
            let mut session = ImapExecutor::create_connection(account_id).await?;
            let listed = get_download_folders(account, &mut session, &token).await;
            session.logout().await.ok();
            listed
        },
        &token,
    )
    .await
    {
        Ok(mailboxes) => mailboxes,
        Err(err) => {
            let err_msg =
                format!("Failed to connect to or list the IMAP server's mailboxes: {:#?}", err);
            warn!(account_id = account.id, error = %err, "{}", err_msg);
            DownloadState::append_session_error(account_id, err_msg.clone())?;
            DownloadState::update_session_status(
                account_id,
                DownloadStatus::Failed,
                Some(err_msg),
            )?;
            return Ok(());
        }
    };
    if matches!(download_task, DownloadTask::FullFetch) {
        let result = match &account.date_since {
            Some(date_since) => {
                rebuild_cache_by_date(
                    account,
                    &remote_mailboxes,
                    &date_since.since_date()?,
                    FetchDirection::Since,
                    token,
                )
                .await
            }
            None => match &account.date_before {
                Some(r) => {
                    rebuild_cache_by_date(
                        account,
                        &remote_mailboxes,
                        &r.calculate_date()?,
                        FetchDirection::Before,
                        token,
                    )
                    .await
                }
                None => rebuild_cache(account, &remote_mailboxes, token).await,
            },
        };
        match result {
            Ok(_) => {
                // A full rebuild finished: no reason to keep a throttle
                // backoff from an earlier incremental round.
                DownloadState::clear_throttle(account_id)?;
                DownloadState::update_session_status(account_id, DownloadStatus::Success, None)?;
            }
            Err(e) => {
                let err_msg = format!("Email Download interrupted: {:#?}", e);
                DownloadState::append_session_error(account_id, err_msg.clone())?;
                DownloadState::update_session_status(
                    account_id,
                    DownloadStatus::Failed,
                    Some(err_msg),
                )?;
            }
        }
        return Ok(());
    }

    let local_mailboxes = MailBox::list_all(account_id)?;
    let reconcile_result =
        reconcile_mailboxes(account, &remote_mailboxes, &local_mailboxes, token.clone()).await;

    // Gap-fill phase: only on explicit user request (manual download with
    // "run gap-fill" checked). Enumerate every UID in the download folders and
    // download anything missing locally. Not run on scheduled syncs. Gap-fill
    // runs are tracked in their own state (independent of the download session)
    // because they are repeatable until `failed == 0`.
    //
    // Skipped when the round was throttled: gap-fill opens a connection per
    // folder, which is exactly what the server is refusing right now.
    let throttled = matches!(&reconcile_result, Ok(summary) if summary.throttled);
    if run_gap_fill && !throttled {
        GapFillState::start_run(account_id)?;
        // Run inside a helper so a failure anywhere still finalizes the run:
        // an abandoned active run would otherwise show as Running forever.
        let run_outcome = gap_fill_phase(
            account,
            &local_mailboxes,
            &remote_mailboxes,
            token,
            account_id,
        )
        .await;
        let pass = match run_outcome {
            Ok(v) => v,
            Err(e) => {
                warn!(account_id = account_id, "Gap-fill phase error: {:#?}", e);
                GapFillPass {
                    failed: 1,
                    ..Default::default()
                }
            }
        };
        let status = if pass.cancelled {
            GapFillStatus::Cancelled
        } else if pass.failed > 0 {
            GapFillStatus::Failed
        } else {
            GapFillStatus::Success
        };
        GapFillState::finish_run(
            account_id,
            status,
            pass.downloaded,
            pass.failed,
            pass.filtered,
        )?;
        let mut notes = String::new();
        if pass.filtered > 0 {
            notes.push_str(&format!(", {} skipped by archive rules", pass.filtered));
        }
        if pass.deferred > 0 {
            notes.push_str(&format!(
                ", {} folder(s) deferred (the server stopped answering) — re-run gap-fill later",
                pass.deferred
            ));
        }
        if pass.retried > 0 {
            notes.push_str(&format!(
                ", retried {} failed folder(s) after backoff: {} recovered",
                pass.retried, pass.recovered
            ));
        }
        let summary = if pass.cancelled {
            format!(
                "Gap-fill cancelled: {} downloaded, {} failed{}",
                pass.downloaded, pass.failed, notes
            )
        } else {
            format!(
                "Gap-fill finished: {} downloaded, {} failed{}",
                pass.downloaded, pass.failed, notes
            )
        };
        DownloadState::update_session_message(account_id, summary.clone())?;
        info!(account_id = account_id, "{}", summary);
    }

    // Finalize session status AFTER all phases so stats/progress written
    // during gap-fill are not dropped (update_session_status closes the active
    // session, moving it into history).
    match reconcile_result {
        Ok(summary) => {
            let (status, message) = flow::session_status_for(&summary);
            if summary.throttled {
                // Back off instead of retrying at the normal interval; a
                // short interval against a throttling server is what produced
                // the 72-logins-per-minute storm in #396.
                let delay_secs = DownloadState::note_throttle(account_id)?;
                let minutes = (delay_secs + 59) / 60;
                let msg = format!(
                    "{} Next scheduled attempt in about {} minute(s).",
                    message.unwrap_or_default(),
                    minutes
                );
                DownloadState::update_session_status(account_id, status, Some(msg))?;
            } else {
                // Any non-throttled round clears a previous backoff.
                DownloadState::clear_throttle(account_id)?;
                DownloadState::update_session_status(account_id, status, message)?;
            }
        }
        Err(e) => {
            let err_msg = format!("Email Download interrupted: {:#?}", e);
            DownloadState::append_session_error(account_id, err_msg.clone())?;
            DownloadState::update_session_status(
                account_id,
                DownloadStatus::Failed,
                Some(err_msg),
            )?;
        }
    }

    let elapsed_time = start_time.elapsed().as_secs();
    debug!(
        "Account{{{}}} Incremental sync completed: {} seconds elapsed.",
        account.email, elapsed_time
    );
    Ok(())
}

/// A gap-fill scan for one folder, boxed so the phase loop can be driven by a
/// fake in tests without an IMAP session.
type GapFillScanFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = BichonResult<GapFillFolderStats>> + Send + 'a>,
>;

/// One folder's scan inside a gap-fill run.
///
/// A trait rather than the closure this loop used to take: the production
/// scanner owns the run's single IMAP session, and a `FnMut` closure cannot
/// return a future that borrows its captured state.
pub(crate) trait GapFillScanner: Send {
    fn scan<'a>(
        &'a mut self,
        local: &'a MailBox,
        remote: &'a MailBox,
        token: CancellationToken,
    ) -> GapFillScanFuture<'a>;
}

/// The production scanner: one IMAP session for every folder of the run.
/// Gap-fill used to log in per folder (and again for the downloads), which is
/// exactly what a rate-limiting server punishes with `EXAMINE` timeouts
/// (#396).
struct LiveGapFillScanner<'a> {
    account: &'a AccountModel,
    conn: gap_fill::GapFillConnection,
}

impl GapFillScanner for LiveGapFillScanner<'_> {
    fn scan<'a>(
        &'a mut self,
        local: &'a MailBox,
        remote: &'a MailBox,
        token: CancellationToken,
    ) -> GapFillScanFuture<'a> {
        Box::pin(gap_fill::gap_fill_mailbox(
            self.account,
            local,
            remote,
            token,
            &mut self.conn,
        ))
    }
}

/// Outcome of one gap-fill run (first pass plus the retry pass).
#[derive(Debug, Default)]
struct GapFillPass {
    cancelled: bool,
    downloaded: u64,
    failed: u64,
    filtered: u64,
    /// Folders skipped because the server stopped answering / throttled.
    /// They are deferred, not failed.
    deferred: usize,
    /// Folders that failed with a transient error in the first pass.
    retryable_failures: Vec<String>,
    /// How many of them the retry pass attempted.
    retried: usize,
    /// How many retries succeeded.
    recovered: usize,
}

/// Backoff before retrying the folders whose first scan failed on a network
/// error. Matches the longest throttling backoff: retrying a throttled server
/// sooner only repeats the failure.
#[cfg(not(test))]
const GAP_FILL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(120);

/// Test builds retry immediately — the wait itself is not what the tests
/// assert, and a 2-minute pause would make them unusable.
fn gap_fill_retry_delay() -> std::time::Duration {
    #[cfg(test)]
    {
        std::time::Duration::ZERO
    }
    #[cfg(not(test))]
    {
        GAP_FILL_RETRY_DELAY
    }
}

/// Runs the gap-fill phase across all download-folder mailboxes. Errors inside
/// are converted into a failed-run outcome instead of propagating, so the
/// caller can always finalize the active run.
async fn gap_fill_phase(
    account: &AccountModel,
    local_mailboxes: &[MailBox],
    remote_mailboxes: &[MailBox],
    token: CancellationToken,
    account_id: u64,
) -> BichonResult<GapFillPass> {
    let mut scanner = LiveGapFillScanner {
        account,
        conn: gap_fill::GapFillConnection::new(account_id),
    };
    let pass = run_gap_fill_folders(
        local_mailboxes,
        remote_mailboxes,
        token,
        account_id,
        &mut scanner,
    )
    .await;
    // Log the shared session out once, on every exit path.
    scanner.conn.close().await;
    pass
}

/// The folder loop, split out from [`gap_fill_phase`] so tests can inject a
/// fake scan and exercise the cancellation rules without an IMAP server.
async fn run_gap_fill_folders(
    local_mailboxes: &[MailBox],
    remote_mailboxes: &[MailBox],
    token: CancellationToken,
    account_id: u64,
    scanner: &mut dyn GapFillScanner,
) -> BichonResult<GapFillPass> {
    let mut pass = GapFillPass::default();
    // Consecutive folders that failed with a network error. A throttling
    // server often stops answering rather than sending an IMAP throttle
    // response (#396); after this many in a row, defer the rest instead of
    // opening another session per folder and prolonging the lockout.
    let mut consecutive_network_failures = 0u32;
    let remaining_folders = |from: usize| {
        local_mailboxes[from..]
            .iter()
            .filter(|m| remote_mailboxes.iter().any(|r| r.name == m.name))
            .count()
    };
    for (index, local_mailbox) in local_mailboxes.iter().enumerate() {
        let Some(remote) = remote_mailboxes.iter().find(|r| r.name == local_mailbox.name) else {
            continue;
        };
        if token.is_cancelled() {
            pass.cancelled = true;
            break;
        }
        DownloadState::set_current_folder(account_id, local_mailbox.name.clone())?;
        match scanner.scan(local_mailbox, remote, token.clone()).await {
            Ok(stats) => {
                consecutive_network_failures = 0;
                pass.downloaded += stats.downloaded;
                pass.failed += stats.failed;
                pass.filtered += stats.filtered;
                GapFillState::add_folder_result(account_id, local_mailbox.name.clone(), stats)?;
            }
            Err(e) => {
                let err_msg = format!(
                    "Gap-fill failed for mailbox '{}': {:#?}",
                    local_mailbox.name, e
                );
                warn!(account_id = account_id, "{}", err_msg);
                DownloadState::append_session_error(account_id, err_msg)?;
                pass.failed += 1; // count the mailbox as a failed unit
                let retryable = flow::should_retry_network(e.code(), 0, false);
                if retryable {
                    pass.retryable_failures.push(local_mailbox.name.clone());
                }
                GapFillState::add_folder_result(
                    account_id,
                    local_mailbox.name.clone(),
                    GapFillFolderStats {
                        downloaded: 0,
                        failed: 1,
                        filtered: 0,
                        candidate_count: 0,
                        message: None,
                    },
                )?;
                if e.code() == ErrorCode::ImapServerThrottled {
                    pass.deferred = remaining_folders(index + 1);
                    warn!(
                        account_id,
                        deferred = pass.deferred,
                        "Gap-fill: server throttled this account; deferring the remaining folders"
                    );
                    break;
                }
                if e.code() == ErrorCode::NetworkError {
                    consecutive_network_failures += 1;
                    if consecutive_network_failures >= flow::STALLED_FOLDER_THRESHOLD {
                        pass.deferred = remaining_folders(index + 1);
                        warn!(
                            account_id,
                            deferred = pass.deferred,
                            "Gap-fill: the IMAP server stopped answering; deferring the remaining folders"
                        );
                        break;
                    }
                } else {
                    consecutive_network_failures = 0;
                }
            }
        }
    }
    // A cancellation can land inside the last folder's scan, after the loop
    // above has already passed its check. Honour it so the run is finalized as
    // Cancelled instead of Success.
    if token.is_cancelled() {
        pass.cancelled = true;
    }

    // One more attempt for folders that failed with a transient error, after a
    // backoff: the server that stalled on them minutes ago usually answers
    // again. The session is reused — no new logins (#396).
    if !pass.cancelled && !pass.retryable_failures.is_empty() && !token.is_cancelled() {
        let waited = tokio::select! {
            _ = tokio::time::sleep(gap_fill_retry_delay()) => true,
            _ = token.cancelled() => false,
        };
        if waited {
            pass.retried = pass.retryable_failures.len();
            for name in std::mem::take(&mut pass.retryable_failures) {
                let Some(local) = local_mailboxes.iter().find(|m| m.name == name) else {
                    continue;
                };
                let Some(remote) = remote_mailboxes.iter().find(|r| r.name == name) else {
                    continue;
                };
                DownloadState::set_current_folder(account_id, name.clone())?;
                match scanner.scan(local, remote, token.clone()).await {
                    Ok(stats) => {
                        // The folder was recorded as failed (failed: 1) by the
                        // first pass; replace that entry instead of adding a
                        // second one.
                        pass.failed = pass.failed.saturating_sub(1);
                        pass.downloaded += stats.downloaded;
                        pass.failed += stats.failed;
                        pass.filtered += stats.filtered;
                        pass.recovered += 1;
                        GapFillState::replace_folder_result(account_id, name.clone(), stats)?;
                        info!(account_id, mailbox = %name, "Gap-fill: retry recovered the folder");
                    }
                    Err(e) => {
                        // Already counted as failed; keep that result.
                        warn!(
                            account_id,
                            mailbox = %name,
                            "Gap-fill: retry failed again: {:#?}",
                            e
                        );
                    }
                }
            }
        }
    }
    Ok(pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::code::ErrorCode;
    use std::sync::{Arc, Mutex};

    static TEST_ENV: std::sync::Once = std::sync::Once::new();

    /// The state helpers read the process-global DB, so seed the same shared
    /// test root the other DB-backed tests use.
    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*crate::settings::cli::SETTINGS;
            let _ = &*crate::database::manager::DB_MANAGER;
        });
    }

    fn mailbox(id: u64, name: &str) -> MailBox {
        MailBox {
            id,
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn stats(downloaded: u64, filtered: u64) -> GapFillFolderStats {
        GapFillFolderStats {
            downloaded,
            filtered,
            ..Default::default()
        }
    }

    /// What one fake folder scan does.
    enum ScanAction {
        /// Return the given stats, cancelling the run token first if asked.
        Ok {
            stats: GapFillFolderStats,
            cancel: bool,
        },
        /// Record the folder name, cancel the run, return one downloaded.
        RecordAndCancel(Arc<Mutex<Vec<String>>>),
        /// Fail the first call, succeed from the second on.
        FailFirst(Arc<Mutex<u32>>, GapFillFolderStats),
        /// Fail the first scan of *each* folder, succeed on its retry.
        FailEachFolderOnce(Arc<Mutex<Vec<String>>>, GapFillFolderStats),
        /// Always fail with the given error code.
        AlwaysErr(ErrorCode),
        /// Fail with the given error and cancel the run.
        FailAndCancel(ErrorCode),
    }

    /// A fake [`GapFillScanner`] that counts the folders it served. The loop
    /// must reuse one scanner instance for the whole run: the production
    /// scanner owns the run's single IMAP session (#396).
    struct TestScanner {
        action: ScanAction,
        served: u32,
    }

    impl GapFillScanner for TestScanner {
        fn scan<'a>(
            &'a mut self,
            local: &'a MailBox,
            _remote: &'a MailBox,
            token: CancellationToken,
        ) -> GapFillScanFuture<'a> {
            self.served += 1;
            Box::pin(async move {
                match &mut self.action {
                    ScanAction::Ok { stats, cancel } => {
                        if *cancel {
                            token.cancel();
                        }
                        Ok(stats.clone())
                    }
                    ScanAction::RecordAndCancel(seen) => {
                        seen.lock().unwrap().push(local.name.clone());
                        token.cancel();
                        Ok(stats(1, 0))
                    }
                    ScanAction::FailFirst(attempts, stats) => {
                        let mut count = attempts.lock().unwrap();
                        *count += 1;
                        if *count == 1 {
                            Err(crate::raise_error!(
                                "simulated scan failure".into(),
                                ErrorCode::InternalError
                            ))
                        } else {
                            Ok(stats.clone())
                        }
                    }
                    ScanAction::FailEachFolderOnce(seen, stats) => {
                        let mut seen = seen.lock().unwrap();
                        if seen.iter().any(|n| n == &local.name) {
                            Ok(stats.clone())
                        } else {
                            seen.push(local.name.clone());
                            Err(crate::raise_error!(
                                "simulated network failure".into(),
                                ErrorCode::NetworkError
                            ))
                        }
                    }
                    ScanAction::AlwaysErr(code) => Err(crate::raise_error!(
                        "simulated network failure".into(),
                        *code
                    )),
                    ScanAction::FailAndCancel(code) => {
                        token.cancel();
                        Err(crate::raise_error!("simulated network failure".into(), *code))
                    }
                }
            })
        }
    }

    #[tokio::test]
    async fn cancellation_inside_the_last_folder_is_reported_as_cancelled() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX")];
        let remote = local.clone();
        let token = CancellationToken::new();

        // The user pressed Stop after this folder's scan started; the scan
        // still returns Ok with a partial result.
        let mut scanner = TestScanner {
            action: ScanAction::Ok {
                stats: stats(4, 2),
                cancel: true,
            },
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_001, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(
            pass.cancelled,
            "a cancellation during the last folder must not be finalized as success"
        );
        assert_eq!((pass.downloaded, pass.failed, pass.filtered), (4, 0, 2));
        assert_eq!(scanner.served, 1, "exactly one folder was scanned");
    }

    #[tokio::test]
    async fn completed_phase_is_not_reported_as_cancelled() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();

        let mut scanner = TestScanner {
            action: ScanAction::Ok {
                stats: stats(3, 1),
                cancel: false,
            },
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_002, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(!pass.cancelled);
        assert_eq!((pass.downloaded, pass.failed, pass.filtered), (6, 0, 2));
        assert_eq!(
            scanner.served, 2,
            "one scanner — and therefore one IMAP session — must serve every \
             folder of the run, not one per folder"
        );
    }

    #[tokio::test]
    async fn cancellation_between_folders_stops_before_the_next_scan() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();
        let scanned = Arc::new(Mutex::new(Vec::new()));

        let mut scanner = TestScanner {
            action: ScanAction::RecordAndCancel(scanned.clone()),
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_003, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(pass.cancelled);
        assert_eq!(*scanned.lock().unwrap(), vec!["INBOX".to_string()]);
        assert_eq!((pass.downloaded, pass.failed), (1, 0));
        assert_eq!(scanner.served, 1);
    }

    #[tokio::test]
    async fn scan_error_counts_the_folder_as_failed_and_keeps_going() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();
        let attempts = Arc::new(Mutex::new(0u32));

        let mut scanner = TestScanner {
            action: ScanAction::FailFirst(attempts.clone(), stats(2, 0)),
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_004, &mut scanner)
            .await
            .expect("a folder error must not abort the phase");

        assert!(!pass.cancelled);
        assert_eq!((pass.downloaded, pass.failed, pass.filtered), (2, 1, 0));
        assert_eq!(*attempts.lock().unwrap(), 2);
        assert_eq!(
            scanner.served, 2,
            "a non-retryable failure is not retried"
        );
        assert_eq!(pass.retried, 0);
    }

    /// Two folders in a row that fail with a network error mean the server
    /// stopped answering. The rest are deferred (not failed), instead of
    /// opening another session per folder and prolonging the lockout.
    #[tokio::test]
    async fn repeated_network_failures_defer_the_remaining_folders() {
        init_test_env();
        let local = vec![
            mailbox(1, "INBOX"),
            mailbox(2, "Sent"),
            mailbox(3, "Archive"),
            mailbox(4, "Drafts"),
        ];
        let remote = local.clone();
        let token = CancellationToken::new();

        let mut scanner = TestScanner {
            action: ScanAction::AlwaysErr(ErrorCode::NetworkError),
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_005, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(!pass.cancelled);
        assert_eq!(pass.failed, 2, "both attempted folders are failures");
        assert_eq!(pass.deferred, 2, "the remaining folders are deferred");
        // They are retried once after the backoff; the scanner keeps failing,
        // so they stay failed and are counted once each.
        assert_eq!(pass.retried, 2);
        assert_eq!(pass.recovered, 0);
        assert_eq!(pass.failed, 2, "a failed retry must not double count");
        assert_eq!(
            scanner.served, 4,
            "two first-pass scans plus two retries; the deferred folders were \
             never attempted"
        );
    }

    /// Folders that failed with a transient error are retried once after the
    /// backoff, and a recovered folder replaces its failure instead of being
    /// counted twice.
    #[tokio::test]
    async fn transient_failures_are_retried_and_recovered() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();
        let seen = Arc::new(Mutex::new(Vec::new()));

        let mut scanner = TestScanner {
            action: ScanAction::FailEachFolderOnce(seen.clone(), stats(3, 1)),
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_006, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(!pass.cancelled);
        assert_eq!(pass.retried, 2, "both failed folders are retried");
        assert_eq!(pass.recovered, 2, "both retries succeed");
        assert_eq!(pass.failed, 0, "a recovered folder is no longer failed");
        assert_eq!((pass.downloaded, pass.filtered), (6, 2));
        assert_eq!(scanner.served, 4, "two scans each: first pass + retry");
    }

    /// Cancelling during the first pass skips the retry entirely.
    #[tokio::test]
    async fn cancelled_run_skips_the_retry_pass() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();

        let mut scanner = TestScanner {
            action: ScanAction::FailAndCancel(ErrorCode::NetworkError),
            served: 0,
        };
        let pass = run_gap_fill_folders(&local, &remote, token.clone(), 9_000_007, &mut scanner)
            .await
            .expect("phase loop must not fail");

        assert!(pass.cancelled);
        assert_eq!(pass.failed, 1);
        assert_eq!(pass.retried, 0, "a cancelled run must not sleep and retry");
        assert_eq!(scanner.served, 1);
    }
}
