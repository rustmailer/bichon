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
    error::BichonResult,
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
    let mut session = match ImapExecutor::create_connection(account_id).await {
        Ok(session) => session,
        Err(e) => {
            let err_msg = format!("Failed to connect to IMAP server: {:#?}", e);
            DownloadState::append_session_error(account_id, err_msg.clone())?;
            DownloadState::update_session_status(
                account_id,
                DownloadStatus::Failed,
                Some(err_msg),
            )?;
            return Err(e);
        }
    };
    let remote_mailboxes = match get_download_folders(account, &mut session).await {
        Ok(mailboxes) => mailboxes,
        Err(err) => {
            let err_msg = format!("Failed to fetch mailboxes: {:#?}", err);
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
    session.logout().await.ok();
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
        let (cancelled, total_downloaded, total_failed, total_filtered) = match run_outcome {
            Ok(v) => v,
            Err(e) => {
                warn!(account_id = account_id, "Gap-fill phase error: {:#?}", e);
                (false, 0, 1, 0)
            }
        };
        let status = if cancelled {
            GapFillStatus::Cancelled
        } else if total_failed > 0 {
            GapFillStatus::Failed
        } else {
            GapFillStatus::Success
        };
        GapFillState::finish_run(
            account_id,
            status,
            total_downloaded,
            total_failed,
            total_filtered,
        )?;
        let skipped = if total_filtered > 0 {
            format!(", {} skipped by archive rules", total_filtered)
        } else {
            String::new()
        };
        let summary = if cancelled {
            format!(
                "Gap-fill cancelled: {} downloaded, {} failed{}",
                total_downloaded, total_failed, skipped
            )
        } else {
            format!(
                "Gap-fill finished: {} downloaded, {} failed{}",
                total_downloaded, total_failed, skipped
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

/// Runs the gap-fill phase across all download-folder mailboxes. Errors inside
/// are converted into a failed-run outcome instead of propagating, so the
/// caller can always finalize the active run.
async fn gap_fill_phase(
    account: &AccountModel,
    local_mailboxes: &[MailBox],
    remote_mailboxes: &[MailBox],
    token: CancellationToken,
    account_id: u64,
) -> BichonResult<(bool, u64, u64, u64)> {
    run_gap_fill_folders(
        local_mailboxes,
        remote_mailboxes,
        token,
        account_id,
        |local, remote, token| Box::pin(gap_fill::gap_fill_mailbox(account, local, remote, token)),
    )
    .await
}

/// The folder loop, split out from [`gap_fill_phase`] so tests can inject a
/// fake scan and exercise the cancellation rules without an IMAP server.
/// Returns `(cancelled, downloaded, failed, filtered)`.
async fn run_gap_fill_folders<'a, F>(
    local_mailboxes: &'a [MailBox],
    remote_mailboxes: &'a [MailBox],
    token: CancellationToken,
    account_id: u64,
    mut scan: F,
) -> BichonResult<(bool, u64, u64, u64)>
where
    F: FnMut(&'a MailBox, &'a MailBox, CancellationToken) -> GapFillScanFuture<'a>,
{
    let mut total_downloaded = 0u64;
    let mut total_failed = 0u64;
    let mut total_filtered = 0u64;
    let mut cancelled = false;
    for local_mailbox in local_mailboxes {
        let Some(remote) = remote_mailboxes.iter().find(|r| r.name == local_mailbox.name) else {
            continue;
        };
        if token.is_cancelled() {
            cancelled = true;
            break;
        }
        DownloadState::set_current_folder(account_id, local_mailbox.name.clone())?;
        match scan(local_mailbox, remote, token.clone()).await {
            Ok(stats) => {
                total_downloaded += stats.downloaded;
                total_failed += stats.failed;
                total_filtered += stats.filtered;
                GapFillState::add_folder_result(account_id, local_mailbox.name.clone(), stats)?;
            }
            Err(e) => {
                let err_msg = format!(
                    "Gap-fill failed for mailbox '{}': {:#?}",
                    local_mailbox.name, e
                );
                warn!(account_id = account_id, "{}", err_msg);
                DownloadState::append_session_error(account_id, err_msg)?;
                total_failed += 1; // count the mailbox as a failed unit
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
            }
        }
    }
    // A cancellation can land inside the last folder's scan, after the loop
    // above has already passed its check. Honour it so the run is finalized as
    // Cancelled instead of Success.
    if token.is_cancelled() {
        cancelled = true;
    }
    Ok((cancelled, total_downloaded, total_failed, total_filtered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::code::ErrorCode;

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

    #[tokio::test]
    async fn cancellation_inside_the_last_folder_is_reported_as_cancelled() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX")];
        let remote = local.clone();
        let token = CancellationToken::new();

        let (cancelled, downloaded, failed, filtered) = run_gap_fill_folders(
            &local,
            &remote,
            token.clone(),
            9_000_001,
            |_local, _remote, scan_token| {
                // The user pressed Stop after this folder's scan started; the
                // scan still returns Ok with a partial result.
                scan_token.cancel();
                Box::pin(async { Ok(stats(4, 2)) })
            },
        )
        .await
        .expect("phase loop must not fail");

        assert!(
            cancelled,
            "a cancellation during the last folder must not be finalized as success"
        );
        assert_eq!((downloaded, failed, filtered), (4, 0, 2));
    }

    #[tokio::test]
    async fn completed_phase_is_not_reported_as_cancelled() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();

        let (cancelled, downloaded, failed, filtered) = run_gap_fill_folders(
            &local,
            &remote,
            token.clone(),
            9_000_002,
            |_local, _remote, _token| Box::pin(async { Ok(stats(3, 1)) }),
        )
        .await
        .expect("phase loop must not fail");

        assert!(!cancelled);
        assert_eq!((downloaded, failed, filtered), (6, 0, 2));
    }

    #[tokio::test]
    async fn cancellation_between_folders_stops_before_the_next_scan() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();
        let scanned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let scanned_in_scan = scanned.clone();
        let (cancelled, downloaded, failed, _filtered) = run_gap_fill_folders(
            &local,
            &remote,
            token.clone(),
            9_000_003,
            move |local, _remote, scan_token| {
                scanned_in_scan.lock().unwrap().push(local.name.clone());
                scan_token.cancel();
                Box::pin(async { Ok(stats(1, 0)) })
            },
        )
        .await
        .expect("phase loop must not fail");

        assert!(cancelled);
        assert_eq!(*scanned.lock().unwrap(), vec!["INBOX".to_string()]);
        assert_eq!((downloaded, failed), (1, 0));
    }

    #[tokio::test]
    async fn scan_error_counts_the_folder_as_failed_and_keeps_going() {
        init_test_env();
        let local = vec![mailbox(1, "INBOX"), mailbox(2, "Sent")];
        let remote = local.clone();
        let token = CancellationToken::new();
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(0u32));

        let attempts_in_scan = attempts.clone();
        let (cancelled, downloaded, failed, filtered) = run_gap_fill_folders(
            &local,
            &remote,
            token.clone(),
            9_000_004,
            move |_local, _remote, _token| {
                let mut count = attempts_in_scan.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    Box::pin(async {
                        Err(crate::raise_error!(
                            "simulated scan failure".into(),
                            ErrorCode::InternalError
                        ))
                    })
                } else {
                    Box::pin(async { Ok(stats(2, 0)) })
                }
            },
        )
        .await
        .expect("a folder error must not abort the phase");

        assert!(!cancelled);
        assert_eq!((downloaded, failed, filtered), (2, 1, 0));
        assert_eq!(*attempts.lock().unwrap(), 2);
    }
}
