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
        migration::AccountModel,
        state::{DownloadState, DownloadStatus, FolderStatus},
    },
    cache::{
        imap::{
            download::flow::{
                fetch_and_save_by_date, fetch_and_save_full_mailbox, FetchDirection,
            },
            mailbox::MailBox,
        },
        SEMAPHORE,
    },
    error::{code::ErrorCode, BichonResult},
    imap::uidonly_acquisition::account_requires_uidonly,
    raise_error,
    store::tantivy::envelope::ENVELOPE_MANAGER,
};

use tokio_util::sync::CancellationToken;
use tracing::{error, info};

fn can_trust_empty_mailbox(account: &AccountModel, mailbox: &MailBox) -> bool {
    mailbox.exists == 0
        && mailbox.uidonly_source_scope.is_none()
        && !account_requires_uidonly(account)
}

pub async fn rebuild_cache(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    token: CancellationToken,
) -> BichonResult<()> {
    MailBox::batch_insert(remote_mailboxes)?;
    DownloadState::init_folder_details(
        account.id,
        remote_mailboxes.iter().map(|m| m.name.clone()).collect(),
    )?;

    let mut has_error = false;
    let mut last_err = None;

    for mailbox in remote_mailboxes {
        if token.is_cancelled() {
            DownloadState::update_session_status(
                account.id,
                DownloadStatus::Cancelled,
                Some("Received termination signal (User stop or System shutdown)".to_string()),
            )?;
            break;
        }
        if can_trust_empty_mailbox(account, mailbox) {
            info!(
                "Account {}: Mailbox '{}' on the remote server has no emails. Skipping fetch for this mailbox.",
                account.id, &mailbox.name
            );
            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Success,
                None,
            )?;
            continue;
        }
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

        match fetch_and_save_full_mailbox(&account, &mailbox, false, token.clone()).await {
            Ok(updated) => MailBox::batch_upsert(&[updated])?,
            Err(err) => {
                has_error = true;
                tracing::error!("Folder sync task failed: {:#?}", err);
                last_err = Some(err);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ordinary_empty_mailboxes_use_the_fast_path() {
        let mailbox = MailBox::default();
        assert!(can_trust_empty_mailbox(&AccountModel::default(), &mailbox));
        let limited = AccountModel {
            capabilities: Some(vec!["UIDONLY".into(), "MESSAGELIMIT=10000".into()]),
            ..Default::default()
        };
        assert!(!can_trust_empty_mailbox(&limited, &mailbox));
        assert!(!can_trust_empty_mailbox(
            &AccountModel::default(),
            &MailBox {
                uidonly_source_scope: Some("proof".into()),
                ..Default::default()
            }
        ));
    }
}

pub async fn rebuild_cache_by_date(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    date: &str,
    direction: FetchDirection,
    token: CancellationToken,
) -> BichonResult<()> {
    if account_requires_uidonly(account)
        || remote_mailboxes
            .iter()
            .any(|mailbox| mailbox.uidonly_source_scope.is_some())
    {
        return Err(raise_error!(
            "Date-scoped acquisition is unsafe on a UIDONLY-limited server".into(),
            ErrorCode::Incompatible
        ));
    }
    MailBox::batch_insert(remote_mailboxes)?;
    DownloadState::init_folder_details(
        account.id,
        remote_mailboxes.iter().map(|m| m.name.clone()).collect(),
    )?;

    let mut has_error = false;
    let mut last_err = None;

    for mailbox in remote_mailboxes {
        if token.is_cancelled() {
            DownloadState::update_session_status(
                account.id,
                DownloadStatus::Cancelled,
                Some("Received termination signal (User stop or System shutdown)".to_string()),
            )?;
            break;
        }
        if mailbox.exists == 0 {
            info!(
                "Account {}: Mailbox '{}' on the remote server has no emails. Skipping fetch for this mailbox.",
                account.id, &mailbox.name
            );

            DownloadState::update_folder_progress(
                account.id,
                mailbox.name.clone(),
                0,
                0,
                FolderStatus::Success,
                None,
            )?;
            continue;
        }
        let account = account.clone();
        let mailbox = mailbox.clone();
        let date = date.to_string();
        let direction = direction.clone();

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
        match fetch_and_save_by_date(&account, date.as_str(), &mailbox, direction, token.clone())
            .await
        {
            Ok(new_highest_uid) => {
                let mut updated = mailbox.clone();
                updated.highest_uid = new_highest_uid;
                MailBox::batch_upsert(&[updated])?;
            }
            Err(err) => {
                has_error = true;
                tracing::error!("Folder sync task failed: {:#?}", err);
                last_err = Some(err);
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

    Ok(())
}

pub async fn rebuild_mailbox_cache(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    force_uidonly: bool,
    token: CancellationToken,
) -> BichonResult<MailBox> {
    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account.id, vec![local_mailbox.id])
        .await?;
    fetch_and_save_full_mailbox(account, remote_mailbox, force_uidonly, token).await
}

pub async fn rebuild_mailbox_cache_by_date(
    account: &AccountModel,
    local_mailbox_id: u64,
    date: &str,
    remote: &MailBox,
    direction: FetchDirection,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    if account_requires_uidonly(account) || remote.uidonly_source_scope.is_some() {
        return Err(raise_error!(
            "Date-scoped acquisition is unsafe on a UIDONLY-limited server".into(),
            ErrorCode::Incompatible
        ));
    }
    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account.id, vec![local_mailbox_id])
        .await?;
    if remote.exists == 0 {
        info!(
            "Account {}: Mailbox '{}' has no emails on the remote server. The mailbox is empty, no envelopes to fetch.",
            account.id,
            &remote.name
        );
        DownloadState::update_folder_progress(
            account.id,
            remote.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
        return Ok(None);
    }

    let result = fetch_and_save_by_date(account, date, remote, direction, token).await?;
    Ok(result)
}
