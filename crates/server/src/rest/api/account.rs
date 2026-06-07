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

use crate::common::auth::WrappedContext;
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;
use bichon_core::account::grant::BatchAccountRoleRequest;
use bichon_core::account::migration::{AccountModel, AccountType};
use bichon_core::account::payload::{
    filter_accessible_accounts, AccountCreateRequest, AccountUpdateRequest, MinimalAccount,
};
use bichon_core::account::state::DownloadState;
use bichon_core::account::stats::AccountStats;
use bichon_core::account::view::AccountResp;
use bichon_core::cache::imap::task::SYNC_TASKS;
use bichon_core::common::paginated::{paginate_vec, DataPage};
use bichon_core::error::code::ErrorCode;
use bichon_core::raise_error;
use bichon_core::store::tantivy::envelope::ENVELOPE_MANAGER;
use bichon_core::users::permissions::Permission;
use bichon_core::users::UserModel;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::collections::{HashMap, HashSet};

pub struct AccountApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Account")]
impl AccountApi {
    /// Get account details by account ID
    #[oai(
        path = "/account/:account_id",
        method = "get",
        operation_id = "get_account"
    )]
    async fn get_account(
        &self,
        /// The account ID to retrieve
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountModel>> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        Ok(Json(AccountModel::get(account_id)?))
    }

    /// Delete an account by ID - WARNING: This permanently removes the account and all associated resources
    #[oai(
        path = "/account/:account_id",
        method = "delete",
        operation_id = "remove_account"
    )]
    async fn remove_account(
        &self,
        /// The account ID to delete
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        AccountModel::delete(account_id).await?;
        Ok(())
    }

    /// Create a new account
    #[oai(path = "/account", method = "post", operation_id = "create_account")]
    async fn create_account(
        &self,
        /// Account creation request payload
        payload: Json<AccountCreateRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountModel>> {
        context.require_permission(None, Permission::ACCOUNT_CREATE)?;
        let account = AccountModel::create_account(context.user.id, payload.0).await?;
        Ok(Json(account))
    }

    /// Update an existing account
    #[oai(
        path = "/account/:account_id",
        method = "post",
        operation_id = "update_account"
    )]
    async fn update_account(
        &self,
        /// The account ID to update
        account_id: Path<u64>,
        /// Account update request payload
        payload: Json<AccountUpdateRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        AccountModel::update(account_id, payload.0, true)?;
        // Reconcile the IDLE supervisor so a config change (idle_mailboxes
        // added, modified or cleared) takes effect without a restart.
        if let Ok(updated) = AccountModel::get(account_id) {
            bichon_core::cache::imap::idle::IDLE_SUPERVISOR
                .start_account(updated)
                .await;
        }
        Ok(())
    }

    /// List accounts with optional pagination parameters
    #[oai(path = "/accounts", method = "get", operation_id = "list_accounts")]
    async fn list_accounts(
        &self,
        /// Optional. The page number to retrieve (starting from 1).
        page: Query<Option<u64>>,
        /// Optional. The number of items per page.
        page_size: Query<Option<u64>>,
        /// Optional. Whether to sort the list in descending order.
        desc: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<DataPage<AccountResp>>> {
        let is_admin = context.user.is_admin();
        let sort_desc = desc.0.unwrap_or(true);

        let user_map: HashMap<u64, UserModel> = UserModel::list_all()?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        let page_data: DataPage<AccountModel> = if is_admin {
            AccountModel::paginate_list(page.0, page_size.0, desc.0)?
        } else {
            let authorized_ids: HashSet<u64> =
                context.user.account_access_map.keys().cloned().collect();

            if authorized_ids.is_empty() {
                return Ok(Json(DataPage {
                    current_page: page.0,
                    page_size: page_size.0,
                    total_items: 0,
                    items: vec![],
                    total_pages: Some(0),
                }));
            }

            let mut accounts: Vec<AccountModel> = AccountModel::list_all()?
                .into_iter()
                .filter(|acct| authorized_ids.contains(&acct.id))
                .collect();

            accounts.sort_by(|a, b| {
                if sort_desc {
                    b.created_at.cmp(&a.created_at)
                } else {
                    a.created_at.cmp(&b.created_at)
                }
            });

            paginate_vec(&accounts, page.0, page_size.0).map(DataPage::from)?
        };

        let items = page_data
            .items
            .into_iter()
            .map(|account| AccountResp::from_model(account, &user_map))
            .collect();

        Ok(Json(DataPage {
            current_page: page_data.current_page,
            page_size: page_data.page_size,
            total_items: page_data.total_items,
            total_pages: page_data.total_pages,
            items,
        }))
    }

    /// Get the running state of an account
    #[oai(
        path = "/accounts/:account_id/download-stats",
        method = "get",
        operation_id = "accounts_download_state"
    )]
    async fn accounts_download_state(
        &self,
        /// The account ID to check state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<DownloadState>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = DownloadState::get(account_id)?;
        let state = state.unwrap_or(DownloadState::empty(account_id));
        Ok(Json(state))
    }

    /// Start a manual download task for an account
    #[oai(
        path = "/accounts/:account_id/start-download",
        method = "post",
        operation_id = "accounts_start_download"
    )]
    async fn accounts_start_download(
        &self,
        /// The account ID to start download for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;
        if !matches!(account.account_type, AccountType::IMAP) {
            return Err(raise_error!(
                format!("Manual download is not supported for '{:#?}' accounts. Only IMAP accounts are supported.", account.account_type),
                ErrorCode::InvalidParameter
            ))?;
        }
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        SYNC_TASKS.start_manual_task(account_id).await?;
        Ok(())
    }

    /// Cancel a running manual download task
    #[oai(
        path = "/accounts/:account_id/cancel-download",
        method = "post",
        operation_id = "accounts_cancel_download"
    )]
    async fn accounts_cancel_download(
        &self,
        /// The account ID to cancel download for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;

        if !matches!(account.account_type, AccountType::IMAP) {
            return Err(raise_error!(
                "This operation is only supported for IMAP accounts.".into(),
                ErrorCode::InvalidParameter
            ))?;
        }
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;

        if !SYNC_TASKS.is_manual_running(account_id).await {
            return Err(raise_error!(
                "No running manual task found for this account.".into(),
                ErrorCode::ResourceNotFound
            ))?;
        }
        SYNC_TASKS.cancel_manual_task(account_id).await;
        Ok(())
    }

    /// Get the stats of an account
    #[oai(
        path = "/accounts/:account_id/stats",
        method = "get",
        operation_id = "accounts_stats"
    )]
    async fn accounts_stats(
        &self,
        /// The account ID to check state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountStats>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = ENVELOPE_MANAGER.get_account_stats(account_id)?;
        Ok(Json(state))
    }

    /// Get a minimal list of active accounts for use in selectors when creating account-related resources
    ///
    /// This endpoint provides a lightweight list of accounts containing only essential information (id and name).
    /// It's primarily designed for UI selectors/dropdowns when creating or associating resources with accounts.
    #[oai(
        path = "/minimal-account-list",
        method = "get",
        operation_id = "minimal_accounts_list"
    )]
    async fn minimal_accounts_list(
        &self,
        only_nosync: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<MinimalAccount>>> {
        let is_admin = context.user.is_admin();
        let only_nosync = only_nosync.0.unwrap_or_default();

        let minimal_list = AccountModel::minimal_list(only_nosync)?;
        if is_admin {
            return Ok(Json(minimal_list));
        }

        let authorized_ids: Vec<u64> = context.user.account_access_map.keys().cloned().collect();
        let result = filter_accessible_accounts(&minimal_list, &authorized_ids);
        Ok(Json(result))
    }

    #[oai(path = "/accounts/access/assignments", method = "post")]
    async fn batch_assign_account_role(
        &self,
        req: Json<BatchAccountRoleRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        req.validate_existence()?;
        req.0.do_assign(&context)?;
        Ok(())
    }
}
