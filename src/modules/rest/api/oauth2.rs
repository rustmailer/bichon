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

use crate::modules::account::migration::AccountModel;
use crate::modules::common::auth::ClientContext;
use crate::modules::error::code::ErrorCode;
use crate::modules::oauth2::entity::{OAuth2, OAuth2CreateRequest, OAuth2UpdateRequest};
use crate::modules::oauth2::flow::{AuthorizeUrlRequest, OAuth2Flow};
use crate::modules::oauth2::token::{ExternalOAuth2Request, OAuth2AccessToken};
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::response::DataPage;
use crate::modules::rest::ApiResult;
use crate::modules::users::permissions::Permission;
use crate::raise_error;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::OpenApi;

pub struct OAuth2Api;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::OAuth2")]
impl OAuth2Api {
    /// Retrieves the OAuth2 configuration for a specified name.
    ///
    /// Requires root privileges.
    /// This endpoint fetches the OAuth2 configuration identified by the given name.
    #[oai(
        path = "/oauth2/:id",
        method = "get",
        operation_id = "get_oauth2_config"
    )]
    async fn get_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to retrieve
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<OAuth2>> {
        let id = id.0;
        let mut oauth2 = OAuth2::get(id).await?.ok_or_else(|| {
            raise_error!(
                format!("OAuth2 configuration id='{id}' not found"),
                ErrorCode::ResourceNotFound
            )
        })?;
        if context
            .has_permission(None, Permission::ROOT)
            .await
        {
            return Ok(Json(oauth2));
        }

        context
            .require_permission(None, Permission::ACCOUNT_CREATE)
            .await?;

        oauth2.scrub_sensitive_fields();
        Ok(Json(oauth2))
    }

    /// Deletes an OAuth2 configuration by name.
    ///
    /// Requires root privileges.
    /// This endpoint removes the OAuth2 configuration identified by the specified name.
    #[oai(
        path = "/oauth2/:id",
        method = "delete",
        operation_id = "remove_oauth2_config"
    )]
    async fn remove_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to retrieve
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context
            .require_permission(None, Permission::ROOT)
            .await?;
        Ok(OAuth2::delete(id.0).await?)
    }

    /// Creates a new OAuth2 configuration.
    ///
    /// Requires root privileges.
    /// This endpoint creates a new OAuth2 configuration based on the provided request data.
    #[oai(
        path = "/oauth2",
        method = "post",
        operation_id = "create_oauth2_config"
    )]
    async fn create_oauth2_config(
        &self,
        /// A JSON payload containing the details for the new OAuth2 configuration
        request: Json<OAuth2CreateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context
            .require_permission(None, Permission::ROOT)
            .await?;
        let entity = OAuth2::new(request.0)?;
        Ok(entity.save().await?)
    }

    /// Updates an existing OAuth2 configuration.
    ///
    /// Requires root privileges.
    /// This endpoint updates the OAuth2 configuration identified by the specified name
    #[oai(
        path = "/oauth2/:id",
        method = "post",
        operation_id = "update_oauth2_config"
    )]
    async fn update_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to update
        id: Path<u64>,
        /// A JSON payload containing the updated configuration details
        payload: Json<OAuth2UpdateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context
            .require_permission(None, Permission::ROOT)
            .await?;
        Ok(OAuth2::update(id.0, payload.0).await?)
    }

    /// Lists OAuth2 configurations with pagination and sorting options.
    ///
    /// This endpoint retrieves a paginated list of OAuth2 configurations, allowing for
    /// optional pagination and sorting parameters. It requires root access.
    #[oai(
        path = "/oauth2-list",
        method = "get",
        operation_id = "list_oauth2_config"
    )]
    async fn list_oauth2_config(
        &self,
        /// Optional. The page number to retrieve (starting from 1).
        page: Query<Option<u64>>,
        /// Optional. The number of items per page.
        page_size: Query<Option<u64>>,
        /// Optional. Whether to sort the list in descending order.
        desc: Query<Option<bool>>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<OAuth2>>> {
        let mut list = OAuth2::paginate_list(page.0, page_size.0, desc.0).await?;
        if context
            .has_permission(None, Permission::ROOT)
            .await
        {
            return Ok(Json(list));
        }

        context
            .require_permission(None, Permission::ACCOUNT_CREATE)
            .await?;

        for item in &mut list.items {
            item.scrub_sensitive_fields();
        }

        Ok(Json(list))
    }

    /// Generates an OAuth2 authorization URL for a specific account.
    ///
    /// This endpoint creates an authorization URL for the specified OAuth2 configuration
    /// and account ID. It requires root access and returns the URL as plain text.
    #[oai(
        path = "/oauth2-authorize-url",
        method = "post",
        operation_id = "create_oauth2_authorize_url"
    )]
    async fn create_oauth2_authorize_url(
        &self,
        /// A JSON payload containing the OAuth2 configuration name and account ID.
        request: Json<AuthorizeUrlRequest>,
        context: ClientContext,
    ) -> ApiResult<PlainText<String>> {
        let request = request.0;
        context
            .require_any_permission(vec![
                (None, Permission::ACCOUNT_CREATE),
                (Some(request.account_id), Permission::ACCOUNT_MANAGE),
            ])
            .await?;

        let flow = OAuth2Flow::new(request.oauth2_id);
        Ok(PlainText(flow.authorize_url(request.account_id).await?))
    }

    /// Retrieves OAuth2 access tokens for a specified account.
    ///
    /// This endpoint fetches the OAuth2 access tokens associated with the given account ID.
    #[oai(
        path = "/oauth2-tokens/:account_id",
        method = "get",
        operation_id = "get_oauth2_tokens"
    )]
    async fn get_oauth2_tokens(
        &self,
        /// The ID of the account to retrieve access tokens for
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<OAuth2AccessToken>> {
        let account = account_id.0;
        context
            .require_permission(Some(account), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(Json(OAuth2AccessToken::get(account).await?.ok_or_else(
            || {
                raise_error!(
                    "OAuth2 access tokens not found".into(),
                    ErrorCode::ResourceNotFound
                )
            },
        )?))
    }

    /// Configures an external OAuth2 token for a specified account.
    ///
    /// This endpoint allows two usage modes:
    /// 1. If only an `access_token` is provided, Bichon will store it directly.
    ///    - In this mode, Bichon **cannot refresh** the token, since it has no
    ///      associated OAuth2 configuration or refresh token.
    ///    - The caller is responsible for periodically updating the access token
    ///      by calling this endpoint again.
    /// 2. If both `oauth2_id` and `refresh_token` are provided, it means the external
    ///    OAuth2 authorization flow has been completed outside Bichon.
    ///    - Since the OAuth2 configuration (including client_id and client_secret)
    ///      is already stored in Bichon, the service can use the refresh token
    ///      to obtain new access tokens automatically.
    ///
    /// Note: The `oauth2_id` must reference a valid OAuth2 configuration
    /// already created in Bichon.
    #[oai(
        path = "/store-external-oauth2-token/:account_id",
        method = "post",
        operation_id = "store_external_oauth2_token"
    )]
    async fn store_external_oauth2_token(
        &self,
        account_id: Path<u64>,
        request: Json<ExternalOAuth2Request>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id).await?;
        // Check account access permissions
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        OAuth2AccessToken::upsert_external_oauth_token(account_id, request.0).await?;
        Ok(())
    }
}
