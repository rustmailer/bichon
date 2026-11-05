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

use crate::account::entity::AuthType;
use crate::account::state::{DownloadState, TriggerType};
use crate::archive::imap::download::process_imap_download;
use crate::common::periodic::{PeriodicTask, TaskHandle};
use crate::error::code::ErrorCode;
use crate::oauth2::token::OAuth2AccessToken;
use crate::{account::migration::AccountModel, error::BichonResult};
use crate::{raise_error, utc_now};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::{sync::LazyLock, time::Duration};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

static _DESCRIPTION: &str = "This task periodically synchronizes mailbox data for a specified account, ensuring that all local data is up-to-date.";
const TASK_INTERVAL: Duration = Duration::from_secs(10);
pub static SYNC_TASKS: LazyLock<AccountDownTask> = LazyLock::new(AccountDownTask::new);
static LAST_WARN_TIME: AtomicI64 = AtomicI64::new(0);
const WARN_INTERVAL_MS: i64 = 600_000;

pub struct AccountDownTask {
    tasks: Mutex<Option<HashMap<u64, (TaskHandle, CancellationToken)>>>,
    manual_tasks: Mutex<HashMap<u64, (JoinHandle<()>, CancellationToken)>>,
    busy_accounts: Mutex<HashSet<u64>>,
}

impl AccountDownTask {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(Some(HashMap::new())),
            manual_tasks: Mutex::new(HashMap::new()),
            busy_accounts: Mutex::new(HashSet::new()),
        }
    }

    async fn set_busy(&self, account_id: u64, is_busy: bool) {
        let mut guard = self.busy_accounts.lock().await;
        if is_busy {
            guard.insert(account_id);
        } else {
            guard.remove(&account_id);
        }
    }

    /// Atomically check and set busy. Returns true if we claimed the slot,
    /// false if another task is already busy on this account.
    async fn try_set_busy(&self, account_id: u64) -> bool {
        let mut guard = self.busy_accounts.lock().await;
        if guard.contains(&account_id) {
            false
        } else {
            guard.insert(account_id);
            true
        }
    }

    /// Whether a scheduled sync holds the account slot. Source-mailbox
    /// cleanup uses this to avoid two IMAP sessions mutating the same mailbox.
    pub async fn is_account_busy(&self, account_id: u64) -> bool {
        self.busy_accounts.lock().await.contains(&account_id)
    }

    /// Take the same per-account slot the download tasks use. Cleanup holds it
    /// for its whole run so a sync cannot start between its pre-check and the
    /// first move.
    pub async fn try_lock_account(&self, account_id: u64) -> bool {
        self.try_set_busy(account_id).await
    }

    pub async fn unlock_account(&self, account_id: u64) {
        self.set_busy(account_id, false).await;
    }

    pub async fn start_download_task(&self, account_id: u64, email: String) {
        let task_name = format!("account-download-task-{}-{}", account_id, &email);
        let periodic_task = PeriodicTask::new(&task_name);

        let cancel_token = CancellationToken::new();
        let task_token = cancel_token.clone();

        let task = move |param: Option<u64>| {
            let account_id = param.unwrap();
            let internal_token = task_token.clone();
            Box::pin(async move {
                if SYNC_TASKS.is_manual_running(account_id).await {
                    debug!(
                        "Account {}: Scheduled task skipped (Manual task is running).",
                        account_id
                    );
                    return Ok(());
                }

                if !SYNC_TASKS.try_set_busy(account_id).await {
                    debug!(
                        "Account {}: Scheduled task skipped (Previous sync still active).",
                        account_id
                    );
                    return Ok(());
                }

                let _busy_guard = scopeguard::guard(account_id, |id| {
                    tokio::spawn(async move {
                        SYNC_TASKS.set_busy(id, false).await;
                    });
                });
                let account = AccountModel::get(account_id).ok();
                match account {
                    Some(account) => {
                        if account.deleting {
                            return Ok(());
                        }
                        if !account.enabled {
                            let last = LAST_WARN_TIME.load(Ordering::Relaxed);
                            let now = utc_now!();
                            if now - last >= WARN_INTERVAL_MS {
                                LAST_WARN_TIME.store(now, Ordering::Relaxed);
                                warn!(
                                    "Account {}: download aborted. Account is currently disabled.",
                                    account_id
                                );
                            }
                        } else {
                            if let Some(imap) = &account.imap {
                                if let AuthType::OAuth2 = imap.auth.auth_type {
                                    if OAuth2AccessToken::get(account.id)?.is_none() {
                                        if utc_now!() % 300_000 == 0 {
                                            warn!("Account {}: download aborted. OAuth2 authorization not completed. Please visit the rustmailer admin page to authorize this account.", account_id);
                                        }
                                        return Ok(());
                                    }
                                }
                            }
                            if let Err(e) = process_imap_download(
                                &account,
                                internal_token,
                                TriggerType::Scheduled,
                                false,
                            )
                            .await
                            {
                                DownloadState::append_session_error(
                                    account.id,
                                    format!("error in account download task: {:#?}", e),
                                )?;
                                error!(
                                    "Failed to download mailbox data for '{}': {:?}",
                                    account_id, e
                                )
                            }
                        }
                    }
                    None => {
                        error!(
                            "Account {}: download aborted. Account entity not found.",
                            account_id
                        );
                    }
                }
                Ok(())
            })
        };
        let handler = periodic_task.start(task, Some(account_id), TASK_INTERVAL, true, true);
        self.add_task(account_id, (handler, cancel_token)).await;
    }

    pub async fn add_task(&self, account_id: u64, handler: (TaskHandle, CancellationToken)) {
        let mut guard = self.tasks.lock().await;
        if let Some(map) = guard.as_mut() {
            map.insert(account_id, handler);
        } else {
            tracing::error!("Failed to add task: HashMap has been taken during shutdown.");
        }
    }

    pub async fn stop(&self, account_id: u64) -> BichonResult<()> {
        let mut guard = self.tasks.lock().await;
        if let Some(map) = guard.as_mut() {
            if let Some((handler, token)) = map.remove(&account_id) {
                drop(guard);
                token.cancel();
                handler.cancel().await;
            }
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        let mut guard = self.tasks.lock().await;
        if let Some(map) = guard.take() {
            drop(guard);
            for (account_id, (handler, token)) in map {
                info!(
                    "Shutdown: Sending cancel signal to account {}...",
                    account_id
                );
                token.cancel();
                if let Err(_) = tokio::time::timeout(Duration::from_secs(5), handler.stop()).await {
                    error!(
                        "Shutdown: Account {} download task forced timeout.",
                        account_id
                    );
                }
            }
            info!("Shutdown: All download tasks processed.");
        }
    }

    pub async fn start_manual_task(&self, account_id: u64, run_gap_fill: bool) -> BichonResult<()> {
        if !crate::ext::license_gate::sync_allowed() {
            return Err(crate::ext::license_gate::sync_refused_error());
        }
        {
            if self.is_manual_running(account_id).await {
                return Err(raise_error!(
                    "Manual task already running.".into(),
                    ErrorCode::Forbidden
                ));
            }
            if !self.try_set_busy(account_id).await {
                return Err(raise_error!(
                    "The background synchronization is currently active. Please try again in a few seconds.".into(),
                    ErrorCode::Forbidden
                ));
            }
        }

        let cancel_token = CancellationToken::new();
        let token_clone = cancel_token.clone();
        let handle = tokio::spawn(async move {
            // busy already claimed by caller via try_set_busy
            let _cleanup = scopeguard::guard(account_id, |id| {
                tokio::spawn(async move {
                    SYNC_TASKS.set_busy(id, false).await;
                    let mut guard = SYNC_TASKS.manual_tasks.lock().await;
                    guard.remove(&id);
                });
            });
            if token_clone.is_cancelled() {
                return;
            }
            let account = match AccountModel::get(account_id) {
                Ok(acc) => acc,
                Err(e) => {
                    error!("Failed to fetch account {}: {:?}", account_id, e);
                    return;
                }
            };

            if account.deleting {
                return;
            }

            if let Err(e) =
                process_imap_download(&account, token_clone, TriggerType::Manual, run_gap_fill)
                    .await
            {
                error!("Manual download failed for {}: {:?}", account_id, e);
                let error_msg = format!("error in account download task: {:#?}", e);
                let _ = DownloadState::append_session_error(account.id, error_msg);
            }
        });
        {
            let mut guard = self.manual_tasks.lock().await;
            guard.insert(account_id, (handle, cancel_token));
        }

        Ok(())
    }

    pub async fn cancel_manual_task(&self, account_id: u64) {
        let mut guard = self.manual_tasks.lock().await;
        if let Some((handle, token)) = guard.remove(&account_id) {
            token.cancel();
            let _ = handle.await;
        }
    }

    pub async fn is_manual_running(&self, account_id: u64) -> bool {
        let guard = self.manual_tasks.lock().await;
        guard.contains_key(&account_id)
    }
}
