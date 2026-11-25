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

use std::collections::BTreeMap;

use crate::common::auth::WrappedContext;
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::saved_search::SavedSearchModel;
use bichon_core::token::AccessTokenModel;
use bichon_core::users::minimal::MinimalUser;
use bichon_core::users::payload::{
    RoleCreateRequest, RoleUpdateRequest, UserCreateRequest, UserUpdateRequest,
};
use bichon_core::users::permissions::Permission;
use bichon_core::users::role::{RoleType, UserRole};
use bichon_core::users::view::UserView;
use bichon_core::users::UserModel;
use poem::web::Path;
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;

pub struct UsersApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Users")]
impl UsersApi {
    #[oai(path = "/list-roles", method = "get", operation_id = "list_roles")]
    async fn list_roles(&self, context: WrappedContext) -> ApiResult<Json<Vec<UserRole>>> {
        context.require_permission(None, Permission::USER_MANAGE)?;
        Ok(Json(UserRole::list_all()?))
    }

    #[oai(path = "/roles/:id", method = "delete", operation_id = "remove_role")]
    async fn remove_role(
        &self,
        /// The Role ID to delete
        id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context.require_permission(None, Permission::USER_MANAGE)?;
        let role_name = UserRole::list_all()?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.name)
            .unwrap_or_else(|| format!("role-{id}"));
        UserRole::delete(id)?;
        emit(Event::RoleRemoved {
            removed_by: context.user.username.clone(),
            role_name,
        });
        Ok(())
    }

    /// Create a new account
    #[oai(path = "/roles", method = "post", operation_id = "create_role")]
    async fn create_role(
        &self,
        /// Role creation request payload
        payload: Json<RoleCreateRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<UserRole>> {
        context.require_permission(None, Permission::USER_MANAGE)?;
        let role = UserRole::create(payload.0)?;
        emit(Event::RoleCreated {
            created_by: context.user.username.clone(),
            role_name: role.name.clone(),
        });
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
        context: WrappedContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context.require_permission(None, Permission::USER_MANAGE)?;
        let role = UserRole::list_all()?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.name)
            .unwrap_or_else(|| format!("role-{id}"));
        UserRole::update(id, payload.0)?;
        emit(Event::RoleUpdated {
            updated_by: context.user.username.clone(),
            role_name: role,
        });
        Ok(())
    }

    #[oai(path = "/list-users", method = "get", operation_id = "list_users")]
    async fn list_users(&self, context: WrappedContext) -> ApiResult<Json<Vec<UserView>>> {
        context.require_permission(None, Permission::USER_MANAGE)?;
        let roles = UserRole::list_all()?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        let users = UserModel::list_all()?;
        let users = users.into_iter().map(|u| u.to_view(&role_lookup)).collect();
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
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<bichon_core::token::view::AccessTokenResp>>> {
        let target_user_id = id.0;
        let tokens = AccessTokenModel::get_user_api_tokens(target_user_id)?;
        if context.user.id == target_user_id {
            return Ok(Json(tokens));
        }
        context.require_permission(None, Permission::USER_MANAGE)?;
        Ok(Json(tokens))
    }

    #[oai(path = "/users/:id", method = "delete", operation_id = "remove_user")]
    async fn remove_user(
        &self,
        /// The User ID to delete
        id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let id = id.0;
        context.require_permission(None, Permission::USER_MANAGE)?;
        let target_username = UserModel::find(id)?
            .map(|u| u.username)
            .unwrap_or_else(|| format!("user-{id}"));
        UserModel::remove(id)?;
        let _ = SavedSearchModel::delete_all_for_user(id);
        emit(Event::UserRemoved {
            removed_by: context.user.username.clone(),
            target_user: target_username,
        });
        Ok(())
    }

    #[oai(path = "/users", method = "post", operation_id = "create_user")]
    async fn create_user(
        &self,
        payload: Json<UserCreateRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<UserView>> {
        context.require_permission(None, Permission::USER_MANAGE)?;
        let user = UserModel::create(payload.0)?;
        emit(Event::UserCreated {
            created_by: context.user.username.clone(),
            new_user: user.username.clone(),
        });
        let roles = UserRole::list_all()?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        Ok(Json(user.to_view(&role_lookup)))
    }

    #[oai(path = "/users/:id", method = "post", operation_id = "update_user")]
    async fn update_user(
        &self,
        id: Path<u64>,
        payload: Json<UserUpdateRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let target_id = id.0;
        let current_user_id = context.user.id;
        if current_user_id != target_id {
            context.require_permission(None, Permission::USER_MANAGE)?;
        }
        let mut update_data = payload.0;
        if current_user_id == target_id && !context.has_permission(None, Permission::USER_MANAGE) {
            update_data.global_roles = None;
            update_data.account_access_map = None;
            update_data.acl = None;
        }
        // An administrator resetting someone else's password must sign that
        // account out; a user editing their own profile keeps the session.
        UserModel::update(target_id, update_data, current_user_id != target_id)?;
        let target_username = UserModel::find(target_id)?
            .map(|u| u.username)
            .unwrap_or_else(|| format!("user-{target_id}"));
        emit(Event::UserUpdated {
            updated_by: context.user.username.clone(),
            target_user: target_username,
        });
        Ok(())
    }

    #[oai(
        path = "/current-user",
        method = "get",
        operation_id = "get_current_user"
    )]
    async fn get_current_user(&self, context: WrappedContext) -> ApiResult<Json<UserView>> {
        let roles = UserRole::list_all()?;
        let role_lookup: BTreeMap<u64, UserRole> = roles.into_iter().map(|r| (r.id, r)).collect();
        Ok(Json(context.0.user.to_view(&role_lookup)))
    }

    #[oai(
        path = "/minimal-user-list",
        method = "get",
        operation_id = "get_minimal_user_list"
    )]
    async fn get_minimal_user_list(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<MinimalUser>>> {
        let is_admin = context.user.is_admin();
        let minimal_list = MinimalUser::list_all()?;
        if is_admin {
            return Ok(Json(minimal_list));
        }
        context.require_permission(None, Permission::USER_VIEW)?;
        Ok(Json(minimal_list))
    }

    #[oai(
        path = "/list-account-roles",
        method = "get",
        operation_id = "list_account_roles"
    )]
    async fn list_account_roles(&self, context: WrappedContext) -> ApiResult<Json<Vec<UserRole>>> {
        context.require_permission(None, Permission::USER_VIEW)?;
        let all = UserRole::list_all()?;
        Ok(Json(
            all.into_iter()
                .filter(|r| matches!(r.role_type, RoleType::Account))
                .collect(),
        ))
    }
}
