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

use std::collections::BTreeSet;

use crate::{
    decode_mailbox_name, raise_error,
    {
        account::migration::{AccountModel, AccountType, REMOTE_CLEANUP_FOLDER},
        archive::imap::mailbox::{AttributeEnum, MailBox},
        archive::imap::mailbox_cache,
        error::{code::ErrorCode, BichonResult},
        imap::{executor::ImapExecutor, session::SessionStream},
        mailbox::list::convert_names_to_mailboxes,
    },
};
use async_imap::{types::Name, Session};
use tracing::{debug, info, warn};

pub async fn get_download_folders(
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

    let is_noselect = |mailbox: &MailBox| {
        mailbox
            .attributes
            .iter()
            .any(|attr| matches!(attr.attr, AttributeEnum::NoSelect))
    };

    // bichon_auto_created_trash is Bichon-owned cleanup output. It must never
    // be a download source, even if a user added it to the subscription list.
    let is_cleanup_destination =
        |mailbox: &MailBox| mailbox.name.eq_ignore_ascii_case(REMOTE_CLEANUP_FOLDER);

    let is_default_mailbox = |mailbox: &MailBox| {
        mailbox.name.eq_ignore_ascii_case("INBOX")
            || mailbox
                .attributes
                .iter()
                .any(|attr| matches!(attr.attr, AttributeEnum::Sent))
    };

    // The set `get_download_folders` falls back to when the account has no
    // explicit selection. Auto-add uses it as its baseline so an account that
    // never picked folders does not lose INBOX/Sent when a new folder is
    // appended (see `detect_mailbox_changes`).
    let default_folders: Vec<String> = mailboxes
        .iter()
        .filter(|(mailbox, _)| {
            !is_noselect(mailbox) && !is_cleanup_destination(mailbox) && is_default_mailbox(mailbox)
        })
        .map(|(mailbox, _)| mailbox.name.clone())
        .collect();

    detect_mailbox_changes(
        account,
        mailboxes.iter().map(|(m, _)| m.name.clone()).collect(),
        &default_folders,
    )
    .await?;
    let account = AccountModel::get(account.id)?;
    let subscribed = &account.download_folders.unwrap_or_default();

    let mut matched_mailboxes: Vec<&Name> = if !subscribed.is_empty() {
        mailboxes
            .iter()
            .filter(|(mailbox, _)| {
                subscribed.contains(&mailbox.name)
                    && !is_noselect(mailbox)
                    && !is_cleanup_destination(mailbox)
            })
            .map(|(_, name)| name)
            .collect()
    } else {
        Vec::new()
    };

    if matched_mailboxes.is_empty() {
        matched_mailboxes = mailboxes
            .iter()
            .filter(|(mailbox, _)| {
                !is_noselect(mailbox)
                    && !is_cleanup_destination(mailbox)
                    && is_default_mailbox(mailbox)
            })
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
            AccountModel::update_download_folders(account.id, sync_folders)?;
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
    default_folders: &[String],
) -> BichonResult<()> {
    if account.known_folders.is_none() {
        // First time sync: just save without comparing
        AccountModel::update_known_folders(account.id, all_names)?;
        return Ok(());
    }
    let known_folders = account.known_folders.clone().unwrap_or_default();
    // Compute differences
    let new_folders: Vec<String> = all_names
        .difference(&known_folders)
        .filter(|name| !name.eq_ignore_ascii_case(REMOTE_CLEANUP_FOLDER))
        .cloned()
        .collect();
    let deleted_folders: Vec<String> = known_folders.difference(&all_names).cloned().collect();

    let has_changes = !new_folders.is_empty() || !deleted_folders.is_empty();
    let download_folders = account.download_folders.as_deref().unwrap_or_default();
    // Handle deleted folders in sync_folders
    if !deleted_folders.is_empty() {
        // Check if any deleted folders are in sync_folders
        let remaining_sync_folders: Vec<String> = download_folders
            .iter()
            .filter(|folder| !deleted_folders.contains(folder))
            .cloned()
            .collect();

        // If sync_folders changed, update them
        if remaining_sync_folders.len() != download_folders.len() {
            let removed_count = download_folders.len() - remaining_sync_folders.len();
            info!(
                "Account {}: Removed {} deleted folders from sync_folders",
                account.id, removed_count
            );
            // Note: When all subscribed folders are deleted (remaining_sync_folders empty),
            // the system's default behavior is to automatically fall back to syncing
            // only the default folders (INBOX and Sent) in subsequent operations
            AccountModel::update_download_folders(account.id, remaining_sync_folders)?;
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
        if account.auto_download_new_mailboxes.unwrap_or(false) {
            // Baseline: the account's explicit selection. When it never chose
            // one (`None` or empty), use the same defaults the fallback in
            // `get_download_folders` persists — otherwise the append would
            // write a list of new folders only, which is non-empty and would
            // suppress that fallback, silently dropping INBOX/Sent.
            let baseline: &[String] = if download_folders.is_empty() {
                default_folders
            } else {
                download_folders
            };
            let mut updated: Vec<String> = baseline.to_vec();
            updated.extend(new_folders.iter().cloned());
            AccountModel::update_download_folders(account.id, updated)?;
            info!(
                "Account {}: Auto-added {} new folders to download list",
                account.id,
                new_folders.len()
            );
        }
    }

    // Update known folders only if there were changes
    if has_changes {
        AccountModel::update_known_folders(account.id, all_names)?;
        mailbox_cache::invalidate(account.id).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::payload::AccountCreateRequest;
    use crate::database::{insert_impl, manager::DB_MANAGER};
    use crate::settings::{cli::SETTINGS, dir::DATA_DIR_MANAGER};
    use std::sync::Once;

    /// DB-backed tests in this binary share one on-disk root (the global
    /// statics freeze on first deref), so assertions are scoped to the account
    /// each test creates.
    static TEST_ENV: Once = Once::new();

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
        });
    }

    fn insert_imap_account(
        email: &str,
        download_folders: Option<Vec<String>>,
        known_folders: &[&str],
        auto_add: bool,
    ) -> AccountModel {
        let mut account = AccountModel::new(
            1,
            AccountCreateRequest {
                email: email.to_string(),
                enabled: true,
                account_type: AccountType::IMAP,
                auto_download_new_mailboxes: Some(auto_add),
                ..Default::default()
            },
        )
        .unwrap();
        account.download_folders = download_folders;
        account.known_folders = Some(known_folders.iter().map(|s| s.to_string()).collect());
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();
        account
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// An account that never picked folders (`download_folders = None`) must
    /// not lose INBOX/Sent when auto-add appends a newly discovered folder.
    /// Before the fix the append started from an empty baseline, wrote only
    /// the new folder, and the fallback in `get_download_folders` never ran
    /// because the result was non-empty.
    #[tokio::test]
    async fn auto_add_keeps_default_folders_when_selection_is_none() {
        init_test_env();
        let account =
            insert_imap_account("auto-add-none@test.example", None, &["INBOX", "Sent"], true);
        let default_folders = vec!["INBOX".to_string(), "Sent".to_string()];

        detect_mailbox_changes(
            &account,
            names(&["INBOX", "Sent", "INBOX/New"]),
            &default_folders,
        )
        .await
        .unwrap();

        let stored = AccountModel::get(account.id).unwrap();
        assert_eq!(
            stored.download_folders,
            Some(vec![
                "INBOX".to_string(),
                "Sent".to_string(),
                "INBOX/New".to_string()
            ])
        );
    }

    /// An explicit selection stays the baseline — the defaults are only a
    /// fallback for accounts that never chose folders.
    #[tokio::test]
    async fn auto_add_appends_to_the_explicit_selection() {
        init_test_env();
        let account = insert_imap_account(
            "auto-add-explicit@test.example",
            Some(vec!["INBOX".to_string()]),
            &["INBOX", "Sent", "Sent Items"],
            true,
        );
        let default_folders = vec!["INBOX".to_string(), "Sent".to_string()];

        detect_mailbox_changes(
            &account,
            names(&["INBOX", "Sent", "Sent Items", "Junk"]),
            &default_folders,
        )
        .await
        .unwrap();

        let stored = AccountModel::get(account.id).unwrap();
        assert_eq!(
            stored.download_folders,
            Some(vec!["INBOX".to_string(), "Junk".to_string()])
        );
    }

    /// With auto-add off, discovery only updates `known_folders`; the download
    /// list stays exactly as it was.
    #[tokio::test]
    async fn no_auto_add_leaves_the_selection_untouched() {
        init_test_env();
        let account =
            insert_imap_account("auto-add-off@test.example", None, &["INBOX", "Sent"], false);
        let default_folders = vec!["INBOX".to_string(), "Sent".to_string()];

        detect_mailbox_changes(
            &account,
            names(&["INBOX", "Sent", "INBOX/New"]),
            &default_folders,
        )
        .await
        .unwrap();

        let stored = AccountModel::get(account.id).unwrap();
        assert_eq!(stored.download_folders, None);
        assert!(stored
            .known_folders
            .as_ref()
            .is_some_and(|folders| folders.contains("INBOX/New")));
    }
}
