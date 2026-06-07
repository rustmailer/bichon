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
    account::migration::AccountModel,
    cache::imap::{idle::IDLE_SUPERVISOR, task::SYNC_TASKS},
    error::BichonResult,
};
use std::{sync::LazyLock, time::Duration};
use tokio::sync::mpsc;
use tracing::{error, info};

pub static DOWNLOAD_CONTROLLER: LazyLock<DownloadController> =
    LazyLock::new(DownloadController::new);

pub struct DownloadController {
    channel: mpsc::Sender<(u64, String)>, // Channel to trigger account download by account ID
}

impl DownloadController {
    pub fn new() -> Self {
        let (tx, mut rx) = mpsc::channel::<(u64, String)>(100);

        tokio::spawn(async move {
            while let Some((account_id, email)) = rx.recv().await {
                match Self::start_download(account_id, email.clone()).await {
                    Ok(_) => {}
                    Err(err) => {
                        error!(
                            "Failed to prepare and start scheduled download of account {{{}-{}}}, error: {:#?}",
                            &account_id, &email, err
                        );
                    }
                }
            }
        });

        DownloadController { channel: tx }
    }

    /// Trigger synchronization for a specific account
    pub async fn trigger_schedule(&self, account_id: u64, email: String) {
        if let Err(e) = self.channel.send((account_id, email)).await {
            error!(
                "Failed to trigger download for account={{{}}}, error: {:?}",
                account_id, e
            );
        }
    }

    async fn start_download(account_id: u64, email: String) -> BichonResult<()> {
        info!(
            "Account download starting for account: {}-{}.",
            account_id, email
        );
        SYNC_TASKS.start_download_task(account_id, email).await;
        // Best-effort: if the user has opted into IDLE for one or more
        // mailboxes AND the server advertises IDLE, also spin up the
        // real-time supervisor alongside the legacy poll loop. Failure
        // here must NEVER block the legacy path, so the result is
        // swallowed and logged.
        if let Ok(account) = AccountModel::get(account_id) {
            IDLE_SUPERVISOR.start_account(account).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }
}
