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
        state::{DownloadState, DownloadStatus, TriggerType},
    },
    cache::imap::{download::flow::FetchDirection, mailbox::MailBox},
    error::BichonResult,
    imap::executor::ImapExecutor,
};
use download_folders::get_download_folders;
use download_type::{decide_next_download_task, DownloadTask};
use flow::reconcile_mailboxes;
use rebuild::{rebuild_cache, rebuild_cache_by_date};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub mod download_folders;
pub mod download_type;
pub mod flow;
pub mod rebuild;

pub async fn process_imap_download(
    account: &AccountModel,
    token: CancellationToken,
    trigger_type: TriggerType,
) -> BichonResult<()> {
    assert_eq!(account.account_type, AccountType::IMAP);
    let start_time = Instant::now();
    let account_id = account.id;
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
    // The discovery connection refreshes cached capabilities. Route the
    // download from that fresh snapshot, not the stale task input.
    let account = AccountModel::get(account_id)?;
    if matches!(download_task, DownloadTask::FullFetch) {
        let result = match &account.date_since {
            Some(date_since) => {
                rebuild_cache_by_date(
                    &account,
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
                        &account,
                        &remote_mailboxes,
                        &r.calculate_date()?,
                        FetchDirection::Before,
                        token,
                    )
                    .await
                }
                None => rebuild_cache(&account, &remote_mailboxes, token).await,
            },
        };
        match result {
            Ok(_) => {
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
    match reconcile_mailboxes(&account, &remote_mailboxes, &local_mailboxes, token).await {
        Ok(_) => DownloadState::update_session_status(account_id, DownloadStatus::Success, None)?,
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
