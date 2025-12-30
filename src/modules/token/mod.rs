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

use std::collections::HashMap;

use super::error::code::ErrorCode;
use crate::modules::database::manager::DB_MANAGER;
use crate::modules::database::{
    async_find_impl, delete_impl, filter_by_secondary_key_impl, with_transaction,
};
use crate::modules::database::{insert_impl, list_all_impl, update_impl};
use crate::modules::settings::cli::SETTINGS;
use crate::modules::token::view::AccessTokenResp;
use crate::modules::users::UserModel;
use crate::raise_error;
use crate::{
    generate_token, modules::error::BichonResult,
    modules::token::payload::AccessTokenCreateRequest, utc_now,
};
use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};

pub mod payload;
pub mod root;
pub mod view;

// Starting from version 0.2.0, this model is deprecated/no longer used
// #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
// #[native_model(id = 1, version = 1)]
// #[native_db]
// pub struct AccessToken {
//     /// The unique token string used for authentication
//     #[primary_key]
//     pub token: String,
//     /// A set of account information associated with the token.
//     pub accounts: BTreeSet<AccountInfo>,
//     /// The timestamp (in milliseconds since epoch) when the token was created.
//     pub created_at: i64,
//     /// The timestamp (in milliseconds since epoch) when the token was last updated.
//     pub updated_at: i64,
//     /// An optional description of the token's purpose or usage.
//     pub description: Option<String>,
//     /// The timestamp (in milliseconds since epoch) when the token was last used.
//     pub last_access_at: i64,
//     /// Optional access control settings
//     pub acl: Option<AccessControl>,
// }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Enum)]
pub enum TokenType {
    WebUI,
    Api,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
#[native_model(id = 11, version = 1)]
#[native_db]
pub struct AccessTokenModel {
    /// The ID of the user who owns this token
    #[secondary_key]
    pub user_id: u64,
    /// The unique token string used for authentication
    #[primary_key]
    pub token: String,
    /// An optional name of the token.
    pub name: Option<String>,
    /// Token type: WebUI or API
    pub token_type: TokenType,
    /// The timestamp (in milliseconds since epoch) when the token was created.
    pub created_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token was last updated.
    pub updated_at: i64,
    /// The timestamp (in milliseconds since epoch) when the token expires.
    /// None means the token does not expire (this applies only to API tokens).
    pub expire_at: Option<i64>,
    /// The timestamp (in milliseconds since epoch) when the token was last used.
    pub last_access_at: i64,
}

impl AccessTokenModel {
    pub fn new_api_token(
        token: String,
        user_id: u64,
        name: Option<String>,
        expire_at: Option<i64>,
    ) -> Self {
        Self {
            token,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            last_access_at: Default::default(),
            name,
            user_id,
            token_type: TokenType::Api,
            expire_at,
        }
    }

    pub fn new_webui_token(user_id: u64) -> AccessTokenModel {
        let now = utc_now!();
        AccessTokenModel {
            token: generate_token!(128),
            created_at: now,
            updated_at: now,
            last_access_at: Default::default(),
            name: None,
            user_id,
            token_type: TokenType::WebUI,
            expire_at: None,
        }
    }

    pub async fn reset_webui_token(user_id: u64) -> BichonResult<String> {
        let old_token = Self::get_user_webui_token(user_id).await?;
        let new_token = Self::new_webui_token(user_id);
        let new_token_str = new_token.token.clone();

        match old_token {
            Some(old) => {
                with_transaction(DB_MANAGER.meta_db(), move |rw| {
                    rw.remove(old)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                    rw.insert(new_token)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                    Ok(())
                })
                .await?;
            }
            None => {
                insert_impl(DB_MANAGER.meta_db(), new_token).await?;
            }
        }

        Ok(new_token_str)
    }

    pub async fn get_user_webui_token(user_id: u64) -> BichonResult<Option<AccessTokenModel>> {
        let tokens = filter_by_secondary_key_impl::<AccessTokenModel>(
            DB_MANAGER.meta_db(),
            AccessTokenModelKey::user_id,
            user_id,
        )
        .await?;

        Ok(tokens
            .into_iter()
            .find(|t| t.token_type == TokenType::WebUI))
    }

    pub async fn get_user_api_tokens(user_id: u64) -> BichonResult<Vec<AccessTokenModel>> {
        let tokens = filter_by_secondary_key_impl::<AccessTokenModel>(
            DB_MANAGER.meta_db(),
            AccessTokenModelKey::user_id,
            user_id,
        )
        .await?;

        Ok(tokens
            .into_iter()
            .filter(|t| t.token_type == TokenType::Api)
            .collect())
    }

    pub async fn resolve_user_from_token(token: &str) -> BichonResult<UserModel> {
        let token = token.to_string();
        let token_option = async_find_impl::<AccessTokenModel>(DB_MANAGER.meta_db(), token).await?;
        let token = match token_option {
            Some(token) => token,
            None => {
                return Err(raise_error!(
                    "Permission denied: no valid access token provided.".into(),
                    ErrorCode::PermissionDenied
                ))
            }
        };

        if matches!(token.token_type, TokenType::WebUI) {
            let life = utc_now!() - token.created_at;
            let max_life = SETTINGS.bichon_webui_token_expiration_hours * 60 * 60 * 1000;

            if life > (max_life as i64) {
                return Err(raise_error!(
                    "Permission denied: the WebUI token has expired.".into(),
                    ErrorCode::PermissionDenied
                ));
            }
        }

        if matches!(token.token_type, TokenType::Api) {
            if let Some(expire_at) = token.expire_at {
                if utc_now!() > expire_at {
                    return Err(raise_error!(
                        "Your API token has expired and is no longer valid.".into(),
                        ErrorCode::PermissionDenied
                    ));
                }
            }
            let token = token.token.clone();
            update_impl(
                DB_MANAGER.meta_db(),
                |rw| {
                    rw.get()
                        .primary::<AccessTokenModel>(token)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                        .ok_or_else(|| {
                            raise_error!(
                                "The access token does not exist or has been reset.".into(),
                                ErrorCode::ResourceNotFound
                            )
                        })
                },
                |current| {
                    let mut updated = current.clone();
                    updated.last_access_at = utc_now!();
                    Ok(updated)
                },
            )
            .await?;
        }

        let user = UserModel::find(token.user_id)
            .await?
            .ok_or_else(|| raise_error!("The user associated with this access token does not exist or may have been deleted.".into(), ErrorCode::ResourceNotFound))?;
        Ok(user)
    }

    pub async fn create_api_token(
        user_id: u64,
        request: AccessTokenCreateRequest,
    ) -> BichonResult<String> {
        // Validate request parameters first
        request.validate().await?;
        let expire_at = request
            .expire_in
            .map(|hours| utc_now!() + (hours as i64) * 60 * 60 * 1000);
        let token = generate_token!(128);
        let access_token =
            AccessTokenModel::new_api_token(token.clone(), user_id, request.name, expire_at);
        insert_impl(DB_MANAGER.meta_db(), access_token).await?;
        Ok(token)
    }

    pub async fn delete(token: &str) -> BichonResult<()> {
        let token = token.to_string();
        delete_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get()
                .primary::<AccessTokenModel>(token.clone())
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!("Token '{}' not found during deletion process.", token),
                        ErrorCode::ResourceNotFound
                    )
                })
        })
        .await
    }

    pub async fn get_token(token: &str) -> BichonResult<AccessTokenModel> {
        async_find_impl(DB_MANAGER.meta_db(), token.to_string())
            .await?
            .ok_or_else(|| {
                raise_error!(
                    format!("Access token '{}' not found", token),
                    ErrorCode::ResourceNotFound
                )
            })
    }

    pub async fn list_all_api_tokens() -> BichonResult<Vec<AccessTokenResp>> {
        let users = UserModel::list_all().await?;
        let mut all = list_all_impl::<AccessTokenModel>(DB_MANAGER.meta_db()).await?;

        all.retain(|t| t.token_type == TokenType::Api);
        let user_map: HashMap<u64, UserModel> = users.into_iter().map(|u| (u.id, u)).collect();

        let resp = all
            .into_iter()
            .map(|token| {
                let user = user_map.get(&token.user_id);
                AccessTokenResp {
                    user_name: user
                        .map(|u| u.username.clone())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    user_email: user
                        .map(|u| u.email.clone())
                        .unwrap_or_else(|| "N/A".to_string()),
                    user_id: token.user_id,
                    name: token.name,
                    token: token.token,
                    token_type: token.token_type,
                    created_at: token.created_at,
                    updated_at: token.updated_at,
                    expire_at: token.expire_at,
                    last_access_at: token.last_access_at,
                }
            })
            .collect();

        Ok(resp)
    }
}
