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

use crate::account::migration::AccountType;
use crate::account::state::{DownloadState, GapFillState};
use crate::context::Initialize;
use crate::{
    {
        account::migration::AccountModel, context::controller::DOWNLOAD_CONTROLLER, error::BichonResult,
    },
    utc_now,
};
use std::sync::LazyLock;
use tracing::{info, warn};

pub static BICHON_CONTEXT: LazyLock<BichonContext> = LazyLock::new(BichonContext::new);

pub struct BichonContext {
    start_at: i64,
}

impl Initialize for BichonContext {
    async fn initialize() -> BichonResult<()> {
        BICHON_CONTEXT.start_account_downloader().await?;
        // Account-level retention sweep (free/community feature). Both the
        // community and Pro servers initialize through this hook, so the
        // scheduler starts exactly once per process.
        crate::retention::start_retention_scheduler();
        // Batch deletions are asynchronous and persisted; a job that was
        // running when the process stopped must not look like it is still
        // progressing. It is never resumed automatically.
        crate::message::batch_delete::interrupt_running_jobs()?;
        // Built-in backup subsystem (community preparers + scheduler). The Pro
        // binary registers its own preparers *before* this hook runs, so
        // `register_base_preparers` plus the Pro additions are all in place
        // before any backup window can open.
        crate::backup::init();
        Ok(())
    }
}

impl BichonContext {
    pub fn new() -> Self {
        Self {
            start_at: utc_now!(),
        }
    }
    pub fn uptime_ms(&self) -> i64 {
        utc_now!() - self.start_at
    }

    pub async fn start_account_downloader(&self) -> BichonResult<()> {
        let accounts = AccountModel::list_all()?;
        let active_accounts: Vec<AccountModel> = accounts
            .into_iter()
            .filter(|a| a.enabled && matches!(a.account_type, AccountType::IMAP))
            .collect();

        if active_accounts.is_empty() {
            info!("No active accounts found for account initialization.");
            return Ok(());
        }
        info!(
            "System has {} active IMAP accounts to initialize.",
            active_accounts.len()
        );
        for account in active_accounts {
            // A Running session surviving startup is a leftover from a previous
            // interrupted run; nothing is downloading yet at this point. Mark it
            // Cancelled so the UI doesn't show a phantom "syncing" state. The
            // scheduler starts regardless — its first tick runs immediately, so
            // the interrupted run is caught up on, and the session's trigger
            // stays Scheduled rather than showing a "Manual" the user never
            // initiated.
            match DownloadState::finalize_stale_session(account.id) {
                Ok(true) => {
                    info!(
                        "Account {}: stale sync session finalized on startup.",
                        account.id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to finalize stale session for account {}: {:#?}",
                        account.id, e
                    );
                }
                Ok(false) => {}
            }
            // Same for a leftover gap-fill run: a Running active run surviving
            // startup is a phantom — nothing is scanning at this point.
            match GapFillState::finalize_stale_run(account.id) {
                Ok(true) => {
                    info!(
                        "Account {}: stale gap-fill run finalized on startup.",
                        account.id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to finalize stale gap-fill run for account {}: {:#?}",
                        account.id, e
                    );
                }
                Ok(false) => {}
            }
            // Same for a cleanup run interrupted by a crash. It must not show
            // as Running forever, and the next scheduled slot has to be able
            // to start.
            match crate::account::state::RemoteCleanupState::finalize_stale_run(account.id) {
                Ok(true) => {
                    info!(
                        "Account {}: stale remote-cleanup run finalized on startup.",
                        account.id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to finalize stale remote-cleanup run for account {}: {:#?}",
                        account.id, e
                    );
                }
                Ok(false) => {}
            }
            DOWNLOAD_CONTROLLER
                .trigger_schedule(account.id, account.email)
                .await
        }

        Ok(())
    }
}
