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

use native_db::*;
use native_model::{native_model, Model};

use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tracing::info;

use crate::{
    encrypt,
    modules::{
        account::{
            entity::ImapConfig,
            since::{DateSince, RelativeDate},
            state::AccountRunningState,
        },
        cache::imap::mailbox::MailBox,
        database::{list_all_impl, with_transaction},
        error::BichonResult,
        indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER},
        users::{role::DEFAULT_ACCOUNT_MANAGER_ROLE_ID, UserModel, DEFAULT_ADMIN_USER_ID},
    },
    utc_now,
};

use crate::id;
use crate::modules::account::payload::AccountCreateRequest;
use crate::modules::account::payload::AccountUpdateRequest;
use crate::modules::account::payload::MinimalAccount;
use crate::modules::cache::imap::task::SYNC_TASKS;
use crate::modules::context::controller::SYNC_CONTROLLER;
use crate::modules::context::executors::MAIL_CONTEXT;
use crate::modules::database::count_by_unique_secondary_key_impl;
use crate::modules::database::delete_impl;
use crate::modules::database::manager::DB_MANAGER;
use crate::modules::database::{
    paginate_query_primary_scan_all_impl, secondary_find_impl, update_impl,
};
use crate::modules::error::code::ErrorCode;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::modules::rest::response::DataPage;
use crate::raise_error;

pub type AccountModel = AccountV3;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Enum)]
pub enum AccountType {
    #[default]
    IMAP,
    NoSync,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 4, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV1 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub use_proxy: Option<u64>,
}
impl AccountV1 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 4, version = 2, from = AccountV1)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV2 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
}

impl AccountV2 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 4, version = 3, from = AccountV2)]
#[native_db(primary_key(pk -> String))]
pub struct AccountV3 {
    #[secondary_key(unique)]
    pub id: u64,
    pub imap: Option<ImapConfig>,
    pub enabled: bool,
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub email: String,
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub date_since: Option<DateSince>,
    pub date_before: Option<RelativeDate>,
    pub folder_limit: Option<u32>,
    pub sync_folders: Option<Vec<String>>,
    pub account_type: AccountType,
    pub sync_interval_min: Option<i64>,
    pub sync_batch_size: Option<u32>,
    pub known_folders: Option<BTreeSet<String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: u64, //user id
    pub use_proxy: Option<u64>,
    pub use_dangerous: bool,
    pub pgp_key: Option<String>,
}

impl AccountV3 {
    fn pk(&self) -> String {
        format!("{}_{}", self.created_at, self.id)
    }

    pub fn new(user_id: u64, request: AccountCreateRequest) -> BichonResult<Self> {
        Ok(Self {
            id: id!(64),
            email: request.email,
            name: request.name,
            imap: request.imap.map(|i| i.try_encrypt_password()).transpose()?,
            enabled: request.enabled,
            capabilities: None,
            date_since: request.date_since,
            sync_folders: None,
            known_folders: None,
            account_type: request.account_type,
            sync_interval_min: request.sync_interval_min,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            use_proxy: request.use_proxy,
            folder_limit: request.folder_limit,
            use_dangerous: request.use_dangerous,
            pgp_key: request.pgp_key,
            created_by: user_id,
            sync_batch_size: request.sync_batch_size,
            date_before: request.date_before,
        })
    }

    pub async fn check_account_exists(account_id: u64) -> BichonResult<AccountModel> {
        let account =
            secondary_find_impl::<AccountModel>(DB_MANAGER.meta_db(), AccountV3Key::id, account_id)
                .await?
                .ok_or_else(|| {
                    raise_error!(
                        format!("Account id='{account_id}' not found"),
                        ErrorCode::ResourceNotFound
                    )
                })?;

        // if !account.enabled {
        //     return Err(raise_error!(
        //         format!("Account id='{account_id}' is disabled"),
        //         ErrorCode::AccountDisabled
        //     ));
        // }
        Ok(account)
    }

    /// Fetches an `AccountEntity` by its `id`.
    pub async fn get(account_id: u64) -> BichonResult<AccountModel> {
        let result: AccountModel = Self::find(account_id).await?.ok_or_else(|| {
            raise_error!(
                format!("Account with ID '{account_id}' not found"),
                ErrorCode::ResourceNotFound
            )
        })?;
        Ok(result)
    }

    pub async fn find(account_id: u64) -> BichonResult<Option<AccountModel>> {
        secondary_find_impl::<AccountModel>(DB_MANAGER.meta_db(), AccountV3Key::id, account_id)
            .await
    }

    // /// Saves the current `AccountEntity` by persisting it to storage.
    // pub async fn save(&self) -> BichonResult<()> {
    //     insert_impl(DB_MANAGER.meta_db(), self.to_owned()).await
    // }

    pub async fn create_account(
        user_id: u64,
        request: AccountCreateRequest,
    ) -> BichonResult<AccountModel> {
        let entity = request.create_entity(user_id)?;
        let cloned = entity.clone();
        with_transaction(DB_MANAGER.meta_db(), move |rw| {
            let account_id = entity.id;
            rw.insert::<AccountModel>(entity)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let user = rw
                .get()
                .primary::<UserModel>(user_id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!("User with id={} not found.", user_id),
                        ErrorCode::ResourceNotFound
                    )
                })?;

            let mut updated = user.clone();
            updated
                .account_access_map
                .insert(account_id, DEFAULT_ACCOUNT_MANAGER_ROLE_ID);
            updated.updated_at = utc_now!();
            rw.update(user, updated)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            Ok(())
        })
        .await?;

        if matches!(cloned.account_type, AccountType::IMAP) {
            SYNC_CONTROLLER
                .trigger_start(cloned.id, cloned.email.clone())
                .await;
        }
        Ok(cloned)
    }

    pub async fn update(
        account_id: u64,
        request: AccountUpdateRequest,
        validate: bool,
    ) -> BichonResult<()> {
        let account = AccountModel::get(account_id).await?;
        if validate {
            request.validate_update_request(&account)?;
        }
        update_impl(
            DB_MANAGER.meta_db(),
            move |_| Ok(account),
            move |current| Self::apply_update_fields(current, request),
        )
        .await?;

        Ok(())
    }

    pub async fn delete(account_id: u64) -> BichonResult<()> {
        let account = Self::get(account_id).await?;
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

    async fn delete_account(account_id: u64) -> BichonResult<()> {
        delete_impl(DB_MANAGER.meta_db(), move|rw|{
            rw.get().secondary::<AccountModel>(AccountV3Key::id, account_id).map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(||raise_error!(format!("The account entity with id={account_id} that you want to delete was not found."), ErrorCode::ResourceNotFound))
        }).await
    }

    async fn cleanup_account_resources_sequential(account: &AccountModel) -> BichonResult<()> {
        if matches!(account.account_type, AccountType::IMAP) {
            SYNC_TASKS.stop(account.id).await?;
            AccountRunningState::delete(account.id).await?;
            MAIL_CONTEXT.clean_account(account.id).await?;
        }
        OAuth2AccessToken::try_delete(account.id).await?;
        UserModel::cleanup_account(account.id).await?;
        MailBox::clean(account.id).await?;
        ENVELOPE_INDEX_MANAGER
            .delete_account_envelopes(account.id)
            .await?;
        EML_INDEX_MANAGER
            .delete_account_envelopes(account.id)
            .await?;
        Self::delete_account(account.id).await?;
        info!("Sequential cleanup completed for account: {}", account.id);
        Ok(())
    }

    pub async fn update_sync_folders(
        account_id: u64,
        sync_folders: Vec<String>,
    ) -> BichonResult<()> {
        update_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get().secondary::<AccountModel>(AccountV3Key::id, account_id).map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(|| raise_error!(format!("When trying to update account sync_folders, the corresponding record was not found. account_id={}", account_id), ErrorCode::ResourceNotFound))
        }, |current|{
            let mut updated = current.clone();
            updated.sync_folders = Some(sync_folders);
            Ok(updated)
        }).await?;
        Ok(())
    }

    pub async fn update_known_folders(
        account_id: u64,
        known_folders: BTreeSet<String>,
    ) -> BichonResult<()> {
        update_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get().secondary::<AccountModel>(AccountV3Key::id, account_id).map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(|| raise_error!(format!("When trying to update account known_folders, the corresponding record was not found. account_id={}", account_id), ErrorCode::ResourceNotFound))
        }, |current|{
            let mut updated = current.clone();
            updated.known_folders = Some(known_folders);
            Ok(updated)
        }).await?;
        Ok(())
    }

    pub async fn update_capabilities(
        account_id: u64,
        capabilities: Vec<String>,
    ) -> BichonResult<()> {
        update_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get().secondary::<AccountModel>(AccountV3Key::id, account_id).map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(|| raise_error!(format!("When trying to update account capabilities, the corresponding record was not found. account_id={}", account_id), ErrorCode::ResourceNotFound))
        }, |current|{
            let mut updated = current.clone();
            updated.capabilities = Some(capabilities);
            Ok(updated)
        }).await?;
        Ok(())
    }

    /// Retrieves a list of all `AccountEntity` instances.
    pub async fn list_all() -> BichonResult<Vec<AccountModel>> {
        list_all_impl(DB_MANAGER.meta_db()).await
    }

    pub async fn minimal_list() -> BichonResult<Vec<MinimalAccount>> {
        let result = list_all_impl(DB_MANAGER.meta_db())
            .await?
            .into_iter()
            //.filter(|a: &AccountModel| a.enabled)
            .map(|account: AccountModel| MinimalAccount {
                id: account.id,
                email: account.email,
            })
            .collect::<Vec<MinimalAccount>>();
        Ok(result)
    }

    pub async fn count() -> BichonResult<usize> {
        count_by_unique_secondary_key_impl::<AccountModel>(DB_MANAGER.meta_db(), AccountV3Key::id)
            .await
    }

    pub async fn paginate_list(
        page: Option<u64>,
        page_size: Option<u64>,
        desc: Option<bool>,
    ) -> BichonResult<DataPage<AccountModel>> {
        paginate_query_primary_scan_all_impl(DB_MANAGER.meta_db(), page, page_size, desc)
            .await
            .map(DataPage::from)
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

        if let Some(folder_limit) = request.folder_limit {
            new.folder_limit = Some(folder_limit);
        }

        if let Some(name) = &request.name {
            if name.trim().is_empty() {
                new.name = None;
            } else {
                new.name = Some(name.clone());
            }
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
                new.sync_folders = Some(folder_names);
            }
            if let Some(sync_interval_min) = &request.sync_interval_min {
                new.sync_interval_min = Some(*sync_interval_min);
            }

            if let Some(sync_batch_size) = &request.sync_batch_size {
                new.sync_batch_size = Some(*sync_batch_size);
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

        new.updated_at = utc_now!();
        Ok(new)
    }
}

impl From<AccountV1> for AccountV2 {
    fn from(value: AccountV1) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
            use_dangerous: false,
            pgp_key: None,
        }
    }
}

impl From<AccountV2> for AccountV1 {
    fn from(value: AccountV2) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
        }
    }
}

impl From<AccountV3> for AccountV2 {
    fn from(value: AccountV3) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            use_proxy: value.use_proxy,
            use_dangerous: value.use_dangerous,
            pgp_key: value.pgp_key,
        }
    }
}

impl From<AccountV2> for AccountV3 {
    fn from(value: AccountV2) -> Self {
        Self {
            id: value.id,
            imap: value.imap,
            enabled: value.enabled,
            email: value.email,
            name: value.name,
            capabilities: value.capabilities,
            date_since: value.date_since,
            folder_limit: value.folder_limit,
            sync_folders: value.sync_folders,
            account_type: value.account_type,
            sync_interval_min: value.sync_interval_min,
            known_folders: value.known_folders,
            created_at: value.created_at,
            updated_at: value.updated_at,
            created_by: DEFAULT_ADMIN_USER_ID,
            use_proxy: value.use_proxy,
            use_dangerous: value.use_dangerous,
            pgp_key: value.pgp_key,
            sync_batch_size: None,
            date_before: None,
        }
    }
}
