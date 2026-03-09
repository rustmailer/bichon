//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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

use std::collections::BTreeSet;

use crate::{
    decode_mailbox_name,
    modules::{
        account::migration::{AccountModel, AccountType},
        cache::imap::mailbox::{AttributeEnum, MailBox},
        error::{code::ErrorCode, BichonResult},
        imap::{executor::ImapExecutor, session::SessionStream},
        mailbox::list::convert_names_to_mailboxes,
    },
    raise_error,
};
use async_imap::{types::Name, Session};
use tracing::{debug, info, warn};

pub async fn get_sync_folders(
    account: &AccountModel,
    session: &mut Session<Box<dyn SessionStream>>,
) -> BichonResult<Vec<MailBox>> {
    assert_eq!(account.account_type, AccountType::IMAP);
    let names = ImapExecutor::list_all_mailboxes(session).await?;
    if names.is_empty() {
        warn!(
            "Account {}: No mailboxes returned from IMAP server.",
            account.id
        );
        return Err(raise_error!(format!(
            "No mailboxes returned from IMAP server for account {}. This is unexpected and may indicate an issue with the IMAP server.",
            &account.id
        ), ErrorCode::ImapUnexpectedResult));
    }
    let mailboxes: Vec<(MailBox, Name)> = names.into_iter().map(|n| ((&n).into(), n)).collect();

    for (mailbox, _) in &mailboxes {
        debug!(
            "[MAILBOX DEBUG] Account {}: mailbox='{}', attributes={:?}",
            account.id, mailbox.name, mailbox.attributes
        );
    }

    detect_mailbox_changes(
        account,
        mailboxes.iter().map(|(m, _)| m.name.clone()).collect(),
    )
    .await?;
    let account = AccountModel::async_get(account.id).await?;
    let subscribed = &account.sync_folders.unwrap_or_default();
    let is_noselect = |mailbox: &MailBox| {
        mailbox
            .attributes
            .iter()
            .any(|attr| matches!(attr.attr, AttributeEnum::NoSelect))
    };

    let is_default_mailbox = |mailbox: &MailBox| {
        mailbox.name.eq_ignore_ascii_case("INBOX")
            || mailbox
                .attributes
                .iter()
                .any(|attr| matches!(attr.attr, AttributeEnum::Sent))
    };

    let mut matched_mailboxes: Vec<&Name> = if !subscribed.is_empty() {
        mailboxes
            .iter()
            .filter(|(mailbox, _)| subscribed.contains(&mailbox.name) && !is_noselect(mailbox))
            .map(|(_, name)| name)
            .collect()
    } else {
        Vec::new()
    };

    if matched_mailboxes.is_empty() {
        matched_mailboxes = mailboxes
            .iter()
            .filter(|(mailbox, _)| !is_noselect(mailbox) && is_default_mailbox(mailbox))
            .map(|(_, name)| name)
            .collect();

        debug!(
            "[MAILBOX DEBUG] Account {}: matched_mailboxes (default selection) = {:?}",
            account.id,
            matched_mailboxes
                .iter()
                .map(|n| decode_mailbox_name!(n.name().to_string()))
                .collect::<Vec<_>>()
        );

        if !matched_mailboxes.is_empty() {
            let sync_folders: Vec<String> = matched_mailboxes
                .iter()
                .map(|n| decode_mailbox_name!(n.name().to_string()))
                .collect();
            AccountModel::update_sync_folders(account.id, sync_folders).await?;
        } else {
            warn!(
                "Account {}: No subscribed mailboxes found. This is unexpected — IMAP server should at least provide INBOX.",
                account.id
            );
            return Err(raise_error!(format!(
                "No subscribed mailboxes found for account {}. This is unexpected — IMAP server should at least provide INBOX.",
                &account.id
            ), ErrorCode::ImapUnexpectedResult));
        }
    }
    convert_names_to_mailboxes(account.id, session, matched_mailboxes).await
}

pub async fn detect_mailbox_changes(
    account: &AccountModel,
    all_names: BTreeSet<String>,
) -> BichonResult<()> {
    if account.known_folders.is_none() {
        // First time sync: just save without comparing
        AccountModel::update_known_folders(account.id, all_names).await?;
        return Ok(());
    }
    let known_folders = account.known_folders.clone().unwrap_or_default();
    // Compute differences
    let new_folders: Vec<String> = all_names.difference(&known_folders).cloned().collect();
    let deleted_folders: Vec<String> = known_folders.difference(&all_names).cloned().collect();

    let has_changes = !new_folders.is_empty() || !deleted_folders.is_empty();
    let sync_folders = account.sync_folders.as_deref().unwrap_or_default();
    // Handle deleted folders in sync_folders
    if !deleted_folders.is_empty() {
        // Check if any deleted folders are in sync_folders
        let remaining_sync_folders: Vec<String> = sync_folders
            .iter()
            .filter(|folder| !deleted_folders.contains(folder))
            .cloned()
            .collect();

        // If sync_folders changed, update them
        if remaining_sync_folders.len() != sync_folders.len() {
            let removed_count = sync_folders.len() - remaining_sync_folders.len();
            info!(
                "Account {}: Removed {} deleted folders from sync_folders",
                account.id, removed_count
            );
            // Note: When all subscribed folders are deleted (remaining_sync_folders empty),
            // the system's default behavior is to automatically fall back to syncing
            // only the default folders (INBOX and Sent) in subsequent operations
            AccountModel::update_sync_folders(account.id, remaining_sync_folders).await?;
        }

        info!(
            "Account {}: Folders deleted: {:?}",
            account.id, deleted_folders
        );
    }

    // Fire events for new folders if needed
    if !new_folders.is_empty() {
        info!(
            "Account {}: New folders detected: {:?}",
            account.id, new_folders
        );
    }

    // Update known folders only if there were changes
    if has_changes {
        AccountModel::update_known_folders(account.id, all_names).await?;
    }
    Ok(())
}
