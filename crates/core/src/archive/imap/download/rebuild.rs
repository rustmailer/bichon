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
    archive::{
        imap::{
            download::flow::{fetch_and_save_by_date, fetch_and_save_full_mailbox, FetchDirection},
            mailbox::MailBox,
        },
        SEMAPHORE,
    },
    error::{code::ErrorCode, BichonResult},
    raise_error,
    store::tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER},
};
use std::collections::HashMap;

use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// The account's mailbox rows as they are before the remote listing overwrites
/// them, keyed by name.
fn existing_rows(account_id: u64) -> BichonResult<HashMap<String, MailBox>> {
    Ok(MailBox::list_all(account_id)?
        .into_iter()
        .map(|mailbox| (mailbox.name.clone(), mailbox))
        .collect())
}

/// Whether the local archive of this folder belongs to the same UIDVALIDITY
/// generation as the server's, i.e. whether its `uid` values can be compared
/// with the server's. After a UIDVALIDITY change the old numbers mean nothing.
fn same_generation(previous: &HashMap<String, MailBox>, remote: &MailBox) -> bool {
    previous
        .get(&remote.name)
        .is_some_and(|old| old.uid_validity == remote.uid_validity)
}

/// Remote rows with the watermarks this account already had. This path runs
/// whenever the download state is missing (fresh data dir, restore, wiped
/// state); a blind insert would reset every `highest_uid` to `None` and make
/// the full enumeration re-download an archive that is already complete.
fn preserve_watermarks(
    remote_mailboxes: &[MailBox],
    previous: &HashMap<String, MailBox>,
) -> Vec<MailBox> {
    remote_mailboxes
        .iter()
        .map(|mailbox| {
            let mut row = mailbox.clone();
            if same_generation(previous, mailbox) {
                row.highest_uid = previous
                    .get(&mailbox.name)
                    .and_then(|old| old.highest_uid);
            }
            row
        })
        .collect()
}

pub async fn rebuild_cache(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    token: CancellationToken,
) -> BichonResult<()> {
    let previous = existing_rows(account.id)?;
    let mailboxes = preserve_watermarks(remote_mailboxes, &previous);
    MailBox::batch_insert(&mailboxes)?;
    DownloadState::init_folder_details(
        account.id,
        mailboxes.iter().map(|m| m.name.clone()).collect(),
    )?;

    let mut has_error = false;
    let mut last_err = None;

    for mailbox in &mailboxes {
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
        let skip_archived = same_generation(&previous, mailbox);
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

        match fetch_and_save_full_mailbox(&account, &mailbox, skip_archived, token.clone()).await {
            Ok(new_highest_uid) => {
                // Only persist a watermark when the folder actually
                // completed: Ok(None) means the download broke mid-way (or
                // the folder is empty), and writing it would clear the row
                // and hide the un-downloaded range from later syncs (#377).
                if let Some(uid) = new_highest_uid {
                    let mut updated = mailbox.clone();
                    updated.highest_uid = Some(uid);
                    MailBox::batch_upsert(&[updated])?;
                }
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

pub async fn rebuild_cache_by_date(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    date: &str,
    direction: FetchDirection,
    token: CancellationToken,
) -> BichonResult<()> {
    let previous = existing_rows(account.id)?;
    let mailboxes = preserve_watermarks(remote_mailboxes, &previous);
    MailBox::batch_insert(&mailboxes)?;
    DownloadState::init_folder_details(
        account.id,
        mailboxes.iter().map(|m| m.name.clone()).collect(),
    )?;

    let mut has_error = false;
    let mut last_err = None;

    for mailbox in &mailboxes {
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
        let skip_archived = same_generation(&previous, mailbox);
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
        match fetch_and_save_by_date(
            &account,
            date.as_str(),
            &mailbox,
            direction,
            skip_archived,
            token.clone(),
        )
        .await
        {
            Ok(new_highest_uid) => {
                // Only persist a watermark when the folder actually
                // completed: Ok(None) means the download broke mid-way (or
                // the folder is empty), and writing it would clear the row
                // and hide the un-downloaded range from later syncs (#377).
                if let Some(uid) = new_highest_uid {
                    let mut updated = mailbox.clone();
                    updated.highest_uid = Some(uid);
                    MailBox::batch_upsert(&[updated])?;
                }
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
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account.id, vec![local_mailbox.id])
        .await?;
    ATTACHMENT_MANAGER
        .delete_mailbox_attachments(account.id, vec![local_mailbox.id])
        .await?;
    if remote_mailbox.exists == 0 {
        info!(
            "Account {}: Mailbox '{}' has no emails on the remote server. The mailbox is empty, no envelopes to fetch.",
            account.id,
            &local_mailbox.name
        );
        DownloadState::update_folder_progress(
            account.id,
            remote_mailbox.name.clone(),
            0,
            0,
            FolderStatus::Success,
            None,
        )?;
        return Ok(None);
    }

    // This is a rebuild of one mailbox: the local envelopes were just deleted,
    // so there is nothing archived to skip.
    let result = fetch_and_save_full_mailbox(account, remote_mailbox, false, token).await?;
    Ok(result)
}

pub async fn rebuild_mailbox_cache_by_date(
    account: &AccountModel,
    local_mailbox_id: u64,
    date: &str,
    remote: &MailBox,
    direction: FetchDirection,
    token: CancellationToken,
) -> BichonResult<Option<u32>> {
    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account.id, vec![local_mailbox_id])
        .await?;
    ATTACHMENT_MANAGER
        .delete_mailbox_attachments(account.id, vec![local_mailbox_id])
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

    // This is a rebuild of one mailbox: the local envelopes were just deleted,
    // so there is nothing archived to skip.
    let result = fetch_and_save_by_date(account, date, remote, direction, false, token).await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, uid_validity: Option<u32>, highest_uid: Option<u32>) -> MailBox {
        MailBox {
            id: 1,
            account_id: 1,
            name: name.to_string(),
            uid_validity,
            highest_uid,
            ..Default::default()
        }
    }

    fn previous(rows: Vec<MailBox>) -> HashMap<String, MailBox> {
        rows.into_iter()
            .map(|mailbox| (mailbox.name.clone(), mailbox))
            .collect()
    }

    /// Losing the download state (fresh data dir, restore) must not make the
    /// next round re-download an archive that is already complete. The
    /// watermark is kept and the full enumeration skips archived UIDs.
    #[test]
    fn watermarks_survive_a_state_loss_when_the_generation_is_unchanged() {
        let previous = previous(vec![row("INBOX", Some(1), Some(180))]);
        let remote = vec![row("INBOX", Some(1), None)];

        let rows = preserve_watermarks(&remote, &previous);

        assert_eq!(rows[0].highest_uid, Some(180));
        assert!(same_generation(&previous, &rows[0]));
    }

    /// After a UIDVALIDITY change the archived `uid` values belong to the old
    /// generation: keeping the watermark or skipping by UID would skip
    /// genuinely missing mail.
    #[test]
    fn a_uidvalidity_change_discards_the_old_watermark_and_skipping() {
        let previous = previous(vec![row("INBOX", Some(1), Some(180))]);
        let remote = vec![row("INBOX", Some(2), None)];

        let rows = preserve_watermarks(&remote, &previous);

        assert_eq!(rows[0].highest_uid, None);
        assert!(!same_generation(&previous, &rows[0]));
    }

    /// A folder the account never synced has nothing to preserve or skip.
    #[test]
    fn a_new_folder_starts_without_a_watermark() {
        let rows = preserve_watermarks(&[row("New", Some(5), None)], &HashMap::new());

        assert_eq!(rows[0].highest_uid, None);
        assert!(!same_generation(&HashMap::new(), &rows[0]));
    }
}
