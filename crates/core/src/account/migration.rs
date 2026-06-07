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

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tracing::info;

use crate::{
    account::{
        entity::ImapConfig,
        payload::{AccountCreateRequest, AccountUpdateRequest, MinimalAccount},
        since::{DateSince, RelativeDate},
        state::DownloadState,
    },
    cache::imap::{idle::IDLE_SUPERVISOR, mailbox::MailBox, task::SYNC_TASKS},
    common::paginated::DataPage,
    context::controller::DOWNLOAD_CONTROLLER,
    database::{
        count_impl, delete_impl, find_impl, insert_impl, list_all_impl, manager::DB_MANAGER,
        paginate_impl, update_impl, MemDbModel,
    },
    encrypt,
    error::{code::ErrorCode, BichonResult},
    id,
    oauth2::token::OAuth2AccessToken,
    raise_error,
    store::tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER},
    users::{payload::UserUpdateRequest, role::DEFAULT_ACCOUNT_MANAGER_ROLE_ID, UserModel},
    utc_now,
};

pub type AccountModel = Account;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum AccountType {
    #[default]
    IMAP,
    NoSync,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum QuotaWindow {
    Hourly,
    #[default]
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Account {
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    #[cfg_attr(
        feature = "web-api",
        oai(validator(custom = "crate::common::validator::EmailValidator"))
    )]
    pub email: String,
    pub account_name: Option<String>,
    pub login_name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub download_folders: Option<Vec<String>>,
    /// Mailboxes on which to keep a permanent IMAP IDLE connection
    /// (RFC 2177). When set and the server advertises `IDLE`, each
    /// listed mailbox spawns a dedicated supervisor that triggers an
    /// incremental sync as soon as the server pushes a notification.
    /// `None` or an empty list keeps the legacy poll-only behaviour.
    pub idle_mailboxes: Option<Vec<String>>,
    pub account_type: AccountType,
    pub download_interval_min: Option<i64>,
    pub download_batch_size: Option<u32>,
    #[serde(default)]
    pub max_email_size_bytes: Option<u64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
    pub imap_quota_bytes: Option<u64>,
    pub imap_quota_window: Option<QuotaWindow>,
    pub auto_download_new_mailboxes: Option<bool>,
    pub download_schedule: Option<String>,
}

impl MemDbModel for Account {
    fn collection() -> &'static str {
        "accounts"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

impl Account {
    pub fn new(user_id: u64, request: AccountCreateRequest) -> BichonResult<Self> {
        Ok(Self {
            id: id!(64),
            email: request.email,
            login_name: request.login_name,
            account_name: request.account_name,
            imap: request.imap.map(|i| i.try_encrypt_password()).transpose()?,
            enabled: request.enabled,
            capabilities: None,
            date_since: request.date_since,
            download_folders: None,
            idle_mailboxes: None,
            known_folders: None,
            account_type: request.account_type,
            download_interval_min: request.download_interval_min,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            use_proxy: request.use_proxy,
            use_dangerous: request.use_dangerous,
            pgp_key: request.pgp_key,
            created_by: user_id,
            download_batch_size: request.download_batch_size,
            max_email_size_bytes: request.max_email_size_bytes,
            date_before: request.date_before,
            auto_download_new_mailboxes: request.auto_download_new_mailboxes,
            imap_quota_bytes: request.imap_quota_bytes,
            imap_quota_window: request.imap_quota_window,
            download_schedule: request.download_schedule,
        })
    }

    pub fn check_account_exists(account_id: u64) -> BichonResult<AccountModel> {
        Self::get(account_id)
    }

    pub fn get(account_id: u64) -> BichonResult<AccountModel> {
        let result: AccountModel = Self::find(account_id)?.ok_or_else(|| {
            raise_error!(
                format!("Account with ID '{account_id}' not found"),
                ErrorCode::ResourceNotFound
            )
        })?;
        Ok(result)
    }

    pub fn find(account_id: u64) -> BichonResult<Option<AccountModel>> {
        let result = find_impl::<AccountModel>(DB_MANAGER.db(), &account_id.to_string())?;
        Ok(result)
    }

    pub async fn create_account(
        user_id: u64,
        request: AccountCreateRequest,
    ) -> BichonResult<AccountModel> {
        let entity = request.create_entity(user_id)?;
        let cloned = entity.clone();

        // Insert account into memdb
        insert_impl(DB_MANAGER.db(), entity)?;

        // Update user's account_access_map
        let user = UserModel::find(user_id)?.ok_or_else(|| {
            raise_error!(
                format!("User with id={} not found.", user_id),
                ErrorCode::ResourceNotFound
            )
        })?;

        let mut updated_map = user.account_access_map.clone();
        updated_map.insert(cloned.id, DEFAULT_ACCOUNT_MANAGER_ROLE_ID);

        UserModel::update(
            user_id,
            UserUpdateRequest {
                username: None,
                email: None,
                password: None,
                avatar_base64: None,
                global_roles: None,
                account_access_map: Some(updated_map),
                acl: None,
                description: None,
                theme: None,
                language: None,
            },
        )?;

        if matches!(cloned.account_type, AccountType::IMAP) {
            DOWNLOAD_CONTROLLER
                .trigger_schedule(cloned.id, cloned.email.clone())
                .await;
        }
        Ok(cloned)
    }

    pub fn update(
        account_id: u64,
        request: AccountUpdateRequest,
        validate: bool,
    ) -> BichonResult<()> {
        let account = AccountModel::get(account_id)?;
        if validate {
            request.validate_update_request(&account)?;
        }
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| Self::apply_update_fields(&current, request),
        )?;

        Ok(())
    }

    pub async fn delete(account_id: u64) -> BichonResult<()> {
        let account = Self::get(account_id)?;
        if let Err(error) = Self::cleanup_account_resources_sequential(&account).await {
            tracing::error!(
                "[CLEANUP_ACCOUNT_ERROR] Account {}: failed to cleanup resources: {:#?}",
                account_id,
                error
            );
            return Err(error);
        }
        Ok(())
    }

    fn delete_account(account: &AccountModel) -> BichonResult<()> {
        delete_impl::<AccountModel>(DB_MANAGER.db(), &account.id.to_string())
    }

    async fn cleanup_account_resources_sequential(account: &AccountModel) -> BichonResult<()> {
        if matches!(account.account_type, AccountType::IMAP) {
            SYNC_TASKS.stop(account.id).await?;
            IDLE_SUPERVISOR.stop_account(account.id).await;
            DownloadState::delete(account.id)?;
        }
        OAuth2AccessToken::try_delete(account.id)?;
        UserModel::cleanup_account(account.id)?;
        MailBox::clean(account.id)?;
        ENVELOPE_MANAGER
            .delete_account_envelopes(account.id)
            .await?;
        ATTACHMENT_MANAGER
            .delete_account_attachments(account.id)
            .await?;
        Self::delete_account(account)?;
        info!("Sequential cleanup completed for account: {}", account.id);
        Ok(())
    }

    pub fn update_download_folders(
        account_id: u64,
        download_folders: Vec<String>,
    ) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.download_folders = Some(download_folders);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    pub fn update_known_folders(
        account_id: u64,
        known_folders: BTreeSet<String>,
    ) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.known_folders = Some(known_folders);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    pub fn update_capabilities(account_id: u64, capabilities: Vec<String>) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.db(),
            &account_id.to_string(),
            move |current: Account| {
                let mut updated = current.clone();
                updated.capabilities = Some(capabilities);
                Ok(updated)
            },
        )?;
        Ok(())
    }

    /// Retrieves a list of all `AccountEntity` instances.
    pub fn list_all() -> BichonResult<Vec<AccountModel>> {
        list_all_impl::<AccountModel>(DB_MANAGER.db())
    }

    pub fn find_by_email(email: &str) -> BichonResult<Option<AccountModel>> {
        let all: Vec<AccountModel> = list_all_impl::<AccountModel>(DB_MANAGER.db())?;
        let target_email = email.trim().to_lowercase();

        let first_match = all
            .into_iter()
            .find(|acc| acc.email.to_lowercase() == target_email);

        Ok(first_match)
    }

    pub fn minimal_list(only_nosync: bool) -> BichonResult<Vec<MinimalAccount>> {
        let result = list_all_impl::<AccountModel>(DB_MANAGER.db())?
            .into_iter()
            .filter(|account: &AccountModel| {
                !only_nosync || matches!(account.account_type, AccountType::NoSync)
            })
            .map(|account: AccountModel| MinimalAccount {
                id: account.id,
                email: account.email,
            })
            .collect::<Vec<MinimalAccount>>();
        Ok(result)
    }

    pub fn count() -> BichonResult<usize> {
        count_impl::<AccountModel>(DB_MANAGER.db())
    }

    pub fn paginate_list(
        page: Option<u64>,
        page_size: Option<u64>,
        desc: Option<bool>,
    ) -> BichonResult<DataPage<AccountModel>> {
        paginate_impl::<AccountModel>(DB_MANAGER.db(), page, page_size, desc).map(DataPage::from)
    }

    // This method applies the updates from the request to the old account entity
    fn apply_update_fields(
        old: &AccountModel,
        request: AccountUpdateRequest,
    ) -> BichonResult<AccountModel> {
        let mut new = old.clone();

        if let Some(date_since) = request.date_since {
            new.date_since = Some(date_since);
            new.date_before = None;
        }

        if let Some(date_before) = request.date_before {
            new.date_before = Some(date_before);
            new.date_since = None;
        }

        if let Some(clear_date_range) = request.clear_date_range {
            if clear_date_range {
                new.date_since = None;
                new.date_before = None;
            }
        }

        if let Some(account_name) = request.account_name {
            new.account_name = Some(account_name);
        }

        if matches!(old.account_type, AccountType::IMAP) {
            if let Some(imap) = &request.imap {
                if let Some(current_imap) = &mut new.imap {
                    current_imap.host = imap.host.clone();
                    current_imap.port = imap.port.clone();
                    current_imap.encryption = imap.encryption.clone();
                    current_imap.auth.auth_type = imap.auth.auth_type.clone();
                    if let Some(password) = &imap.auth.password {
                        let encrypted_password = encrypt!(password)?;
                        current_imap.auth.password = Some(encrypted_password);
                    }
                    current_imap.use_proxy = imap.use_proxy;
                }
            }

            if let Some(folder_names) = request.sync_folders {
                new.download_folders = Some(folder_names);
            }
            if let Some(sync_interval_min) = &request.download_interval_min {
                new.download_interval_min = Some(*sync_interval_min);
            }

            if let Some(download_batch_size) = &request.download_batch_size {
                new.download_batch_size = Some(*download_batch_size);
            }

            if let Some(max_email_size_bytes) = request.max_email_size_bytes {
                new.max_email_size_bytes = Some(max_email_size_bytes);
            }

            if let Some(use_proxy) = request.use_proxy {
                new.use_proxy = Some(use_proxy);
            }
        }

        if matches!(old.account_type, AccountType::NoSync) {
            if let Some(email) = &request.email {
                new.email = email.clone();
            }
        }

        if let Some(enabled) = request.enabled {
            new.enabled = enabled;
        }

        if let Some(use_dangerous) = request.use_dangerous {
            new.use_dangerous = use_dangerous;
        }

        if let Some(pgp_key) = request.pgp_key {
            new.pgp_key = Some(pgp_key);
        }

        if let Some(imap_quota_bytes) = request.imap_quota_bytes {
            new.imap_quota_bytes = Some(imap_quota_bytes);
        }

        if let Some(imap_quota_window) = request.imap_quota_window {
            new.imap_quota_window = Some(imap_quota_window);
        }

        if let Some(auto_download_new_mailboxes) = request.auto_download_new_mailboxes {
            new.auto_download_new_mailboxes = Some(auto_download_new_mailboxes);
        }
        if let Some(download_schedule) = request.download_schedule {
            new.download_schedule = Some(download_schedule);
        }
        if request.clear_download_schedule == Some(true) {
            new.download_schedule = None;
        }
        new.updated_at = utc_now!();
        Ok(new)
    }
}
