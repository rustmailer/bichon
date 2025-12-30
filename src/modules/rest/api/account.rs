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

use std::collections::{HashMap, HashSet};

use crate::modules::account::grant::BatchAccountRoleRequest;
use crate::modules::account::migration::AccountModel;
use crate::modules::account::payload::{
    filter_accessible_accounts, AccountCreateRequest, AccountUpdateRequest, MinimalAccount,
};
use crate::modules::account::state::AccountRunningState;
use crate::modules::account::view::AccountResp;
use crate::modules::common::auth::ClientContext;
use crate::modules::common::paginated::paginate_vec;
use crate::modules::error::code::ErrorCode;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::response::DataPage;
use crate::modules::rest::ApiResult;
use crate::modules::users::permissions::Permission;
use crate::modules::users::UserModel;
use crate::raise_error;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;

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
        context: ClientContext,
    ) -> ApiResult<Json<AccountModel>> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)
            .await?;
        Ok(Json(AccountModel::get(account_id).await?))
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
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(AccountModel::delete(account_id).await?)
    }

    /// Create a new account
    #[oai(path = "/account", method = "post", operation_id = "create_account")]
    async fn create_account(
        &self,
        /// Account creation request payload
        payload: Json<AccountCreateRequest>,
        context: ClientContext,
    ) -> ApiResult<Json<AccountModel>> {
        context
            .require_permission(None, Permission::ACCOUNT_CREATE)
            .await?;
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
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(AccountModel::update(account_id, payload.0, true).await?)
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
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<AccountResp>>> {
        let is_admin = context.user.is_admin().await;
        let sort_desc = desc.0.unwrap_or(true);

        let user_map: HashMap<u64, UserModel> = UserModel::list_all()
            .await?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        let page_data: DataPage<AccountModel> = if is_admin {
            AccountModel::paginate_list(page.0, page_size.0, desc.0).await?
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

            let mut accounts: Vec<AccountModel> = AccountModel::list_all()
                .await?
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
        path = "/account-state/:account_id",
        method = "get",
        operation_id = "account_state"
    )]
    async fn account_state(
        &self,
        /// The account ID to check state for
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<AccountRunningState>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id).await?;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)
            .await?;
        let state = AccountRunningState::get(account_id).await?.ok_or_else(|| {
            raise_error!(
                "account running state is not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;
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
        context: ClientContext,
    ) -> ApiResult<Json<Vec<MinimalAccount>>> {
        let is_admin = context.user.is_admin().await;
        let minimal_list = AccountModel::minimal_list().await?;
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
        context: ClientContext,
    ) -> ApiResult<()> {
        req.validate_existence().await?;
        req.0.do_assign(&context).await?;
        Ok(())
    }
}
