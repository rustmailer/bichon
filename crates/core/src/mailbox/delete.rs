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
    archive::imap::mailbox::MailBox,
    error::code::ErrorCode,
    error::BichonResult,
    raise_error,
    store::tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER},
};

/// Every mailbox id a delete of `mailbox_id` would destroy: the mailbox
/// itself plus its whole subtree, matched by name prefix.
///
/// Extracted so the deletion path and any caller that needs to *count* what is
/// about to be destroyed agree by construction. A separate counting
/// implementation would drift from this one and report a blast radius that
/// does not match what actually gets deleted — which is exactly the number an
/// approver is being asked to sign off on.
pub fn mailbox_subtree_ids(account_id: u64, mailbox_id: u64) -> BichonResult<Vec<u64>> {
    let mailbox = MailBox::get(mailbox_id)?;
    let prefix = format!("{}{}", mailbox.name, mailbox.delimiter.unwrap_or("/".to_owned()));
    let all_mailboxes = MailBox::list_all(account_id)?;
    Ok(all_mailboxes
        .into_iter()
        .filter(|m| m.id == mailbox_id || m.name.starts_with(&prefix))
        .map(|m| m.id)
        .collect())
}

pub async fn delete_mailbox_impl(account_id: u64, mailbox_id: u64) -> BichonResult<()> {
    let account = AccountModel::get(account_id)?;

    // Legal hold: deleting a mailbox cascade-purges its envelopes and
    // attachments, which would destroy held data. Refuse before any mutation
    // (mailbox rows, envelopes, attachments). In the community edition
    // `legal_hold` is always false, so this guard is a no-op there.
    if account.is_on_hold() {
        return Err(raise_error!(
            format!(
                "Account {} ({}) is under a legal hold; deleting its mailboxes is disabled while the hold is active",
                account.id, account.email
            ),
            ErrorCode::Forbidden
        ));
    }

    let ids_to_delete = mailbox_subtree_ids(account_id, mailbox_id)?;

    if ids_to_delete.is_empty() {
        return Ok(());
    }

    for id in &ids_to_delete {
        MailBox::delete(*id)?;
    }

    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account_id, ids_to_delete.clone())
        .await?;
    ATTACHMENT_MANAGER
        .delete_mailbox_attachments(account_id, ids_to_delete.clone())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use crate::account::migration::{AccountModel, AccountType};
    use crate::account::payload::AccountCreateRequest;
    use crate::database::{insert_impl, manager::DB_MANAGER};
    use crate::error::code::ErrorCode;
    use crate::settings::cli::SETTINGS;
    use crate::settings::dir::DATA_DIR_MANAGER;

    use super::delete_mailbox_impl;

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

    fn insert_account(id: u64, email: &str) -> AccountModel {
        let request = AccountCreateRequest {
            email: email.to_string(),
            login_name: None,
            account_name: None,
            imap: None,
            enabled: true,
            date_since: None,
            date_before: None,
            account_type: AccountType::NoSync,
            download_interval_min: None,
            download_batch_size: None,
            max_email_size_bytes: None,
            use_dangerous: false,
            pgp_key: None,
            imap_quota_bytes: None,
            imap_quota_window: None,
            auto_download_new_mailboxes: None,
            download_schedule: None,
            archive_rules: None,
            extraction_rules: None,
            override_from_headers: None,
            retention_days: None,
            remote_cleanup: None,
            kind: None,
            view: None,
        };
        let account = AccountModel::new(id, request).unwrap();
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();
        account
    }

    #[tokio::test]
    async fn delete_mailbox_guard_fires_before_any_mutation() {
        init_test_env();
        let account = insert_account(9011, "hold-mailbox@test.example");

        // No hold: the guard passes and the (nonexistent) mailbox lookup fails
        // with a non-Forbidden error — proving the guard is what blocks the
        // held case, not the mailbox lookup itself.
        let err = delete_mailbox_impl(account.id, 12345).await.unwrap_err();
        assert_ne!(err.code(), ErrorCode::Forbidden);

        // Under a hold: refused with Forbidden before touching any mailbox row.
        AccountModel::place_legal_hold(account.id, 7, Some("freeze".into())).unwrap();
        let err = delete_mailbox_impl(account.id, 12345).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Forbidden);
    }
}
