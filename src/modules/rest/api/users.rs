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

use std::collections::BTreeMap;

use crate::modules::common::auth::ClientContext;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::ApiResult;
use crate::modules::token::AccessTokenModel;
use crate::modules::users::minimal::MinimalUser;
use crate::modules::users::payload::{
    RoleCreateRequest, RoleUpdateRequest, UserCreateRequest, UserUpdateRequest,
};
use crate::modules::users::permissions::Permission;
use crate::modules::users::role::UserRole;
use crate::modules::users::view::UserView;
use crate::modules::users::UserModel;
use poem::web::Path;
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;

pub struct UsersApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Users")]
impl UsersApi {
    #[oai(path = "/list-roles", method = "get", operation_id = "list_roles")]
    async fn list_roles(&self, context: ClientContext) -> ApiResult<Json<Vec<UserRole>>> {
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;

        Ok(Json(UserRole::list_all().await?))
    }

    #[oai(path = "/roles/:id", method = "delete", operation_id = "remove_role")]
    async fn remove_role(
        &self,
        /// The Role ID to delete
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        Ok(UserRole::delete(id).await?)
    }

    /// Create a new account
    #[oai(path = "/roles", method = "post", operation_id = "create_role")]
    async fn create_role(
        &self,
        /// Role creation request payload
        payload: Json<RoleCreateRequest>,
        context: ClientContext,
    ) -> ApiResult<Json<UserRole>> {
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        let role = UserRole::create(payload.0).await?;
        Ok(Json(role))
    }

    /// Update an existing account
    #[oai(path = "/roles/:id", method = "post", operation_id = "update_role")]
    async fn update_role(
        &self,
        /// The Role ID to update
        id: Path<u64>,
        /// Role update request payload
        payload: Json<RoleUpdateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        Ok(UserRole::update(id, payload.0).await?)
    }

    #[oai(path = "/list-users", method = "get", operation_id = "list_users")]
    async fn list_users(&self, context: ClientContext) -> ApiResult<Json<Vec<UserView>>> {
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        let roles = UserRole::list_all().await?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        let users = UserModel::list_all().await?;
        let users = users
            .into_iter()
            .map(|u| u.to_view(&role_lookup))
            .collect();
        Ok(Json(users))
    }

    #[oai(
        path = "/user-tokens/:id",
        method = "get",
        operation_id = "get_user_tokens"
    )]
    async fn get_user_tokens(
        &self,
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<Vec<AccessTokenModel>>> {
        let target_user_id = id.0;
        let tokens = AccessTokenModel::get_user_api_tokens(target_user_id).await?;
        if context.user.id == target_user_id {
            return Ok(Json(tokens));
        }
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        Ok(Json(tokens))
    }

    #[oai(path = "/users/:id", method = "delete", operation_id = "remove_user")]
    async fn remove_user(
        &self,
        /// The User ID to delete
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        Ok(UserModel::remove(id).await?)
    }

    #[oai(path = "/users", method = "post", operation_id = "create_user")]
    async fn create_user(
        &self,
        payload: Json<UserCreateRequest>,
        context: ClientContext,
    ) -> ApiResult<Json<UserView>> {
        context
            .require_permission(None, Permission::USER_MANAGE)
            .await?;
        let user = UserModel::create(payload.0).await?;
        let roles = UserRole::list_all().await?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        Ok(Json(user.to_view(&role_lookup)))
    }

    #[oai(path = "/users/:id", method = "post", operation_id = "update_user")]
    async fn update_user(
        &self,
        id: Path<u64>,
        payload: Json<UserUpdateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let target_id = id.0;
        let current_user_id = context.user.id;
        if current_user_id != target_id {
            context
                .require_permission(None, Permission::USER_MANAGE)
                .await?;
        }
        let mut update_data = payload.0;
        if current_user_id == target_id
            && !context.has_permission(None, Permission::USER_MANAGE).await
        {
            update_data.global_roles = None;
            update_data.account_access_map = None;
            update_data.acl = None;
        }
        Ok(UserModel::update(target_id, update_data).await?)
    }

    #[oai(
        path = "/current-user",
        method = "get",
        operation_id = "get_current_user"
    )]
    async fn get_current_user(&self, context: ClientContext) -> ApiResult<Json<UserView>> {
        let roles = UserRole::list_all().await?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        Ok(Json(context.user.to_view(&role_lookup)))
    }

    #[oai(
        path = "/minimal-user-list",
        method = "get",
        operation_id = "get_minimal_user_list"
    )]
    async fn get_minimal_user_list(
        &self,
        context: ClientContext,
    ) -> ApiResult<Json<Vec<MinimalUser>>> {
        let is_admin = context.user.is_admin().await;
        let minimal_list = MinimalUser::list_all().await?;
        if is_admin {
            return Ok(Json(minimal_list));
        }
        context
            .require_permission(None, Permission::USER_VIEW)
            .await?;

        Ok(Json(minimal_list))
    }
}
