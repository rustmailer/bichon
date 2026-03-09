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

use crate::{
    encrypt, id,
    modules::{
        database::{
            delete_impl, insert_impl, manager::DB_MANAGER, paginate_query_primary_scan_all_impl,
            async_secondary_find_impl, update_impl,
        },
        error::{code::ErrorCode, BichonResult},
        rest::response::DataPage,
    },
    raise_error, utc_now,
};
use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Represents the OAuth2 configuration for a client, including initialization and runtime values.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
#[native_model(id = 5, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct OAuth2 {
    /// A unique identifier for the OAuth2 configuration.
    #[secondary_key(unique)]
    pub id: u64,
    /// A description of what this configuration is used for.
    pub description: Option<String>,
    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,
    /// The client secret used in conjunction with the client ID.
    ///
    /// Users should provide a plaintext secret.
    /// The server will encrypt it using AES-256-GCM and securely store it.
    /// The plaintext secret is never stored, so users must ensure it is valid for OAuth2 authentication.
    pub client_secret: String,
    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,
    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,
    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,
    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,
    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,
    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,
    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
    /// The timestamp when the configuration was created, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// The timestamp when the configuration was last updated, in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl OAuth2 {
    fn pk(&self) -> String {
        format!("{}_{}", &self.created_at, &self.id)
    }

    pub fn new(request: OAuth2CreateRequest) -> BichonResult<Self> {
        let request = request.encrypt()?;
        Ok(OAuth2 {
            id: id!(64),
            description: request.description,
            client_id: request.client_id,
            client_secret: request.client_secret,
            auth_url: request.auth_url,
            token_url: request.token_url,
            redirect_uri: request.redirect_uri,
            scopes: request.scopes,
            extra_params: request.extra_params,
            enabled: request.enabled,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            use_proxy: request.use_proxy,
        })
    }

    pub fn scrub_sensitive_fields(&mut self) {
        let mask = "********";
        let notice =
            " [REDACTED: You do not have permission to view sensitive configuration details]";

        let original_desc = self
            .description
            .clone()
            .unwrap_or_else(|| "OAuth2 Config".to_string());
        self.description = Some(format!("{}{}", original_desc, notice));

        self.client_id = mask.to_string();
        self.client_secret = mask.to_string();
        self.auth_url = mask.to_string();
        self.token_url = mask.to_string();
        self.redirect_uri = mask.to_string();

        self.scopes = None;
        self.extra_params = None;
    }

    pub async fn save(&self) -> BichonResult<()> {
        insert_impl(DB_MANAGER.meta_db(), self.to_owned()).await?;
        Ok(())
    }

    pub async fn paginate_list(
        page: Option<u64>,
        page_size: Option<u64>,
        desc: Option<bool>,
    ) -> BichonResult<DataPage<OAuth2>> {
        paginate_query_primary_scan_all_impl(DB_MANAGER.meta_db(), page, page_size, desc)
            .await
            .map(DataPage::from)
    }

    pub async fn get(id: u64) -> BichonResult<Option<OAuth2>> {
        async_secondary_find_impl(DB_MANAGER.meta_db(), OAuth2Key::id, id).await
    }

    pub async fn delete(id: u64) -> BichonResult<()> {
        delete_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get()
                .secondary::<OAuth2>(OAuth2Key::id, id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!(
                            "The oauth2 entity with id={id} that you want to delete was not found."
                        ),
                        ErrorCode::ResourceNotFound
                    )
                })
        })
        .await
    }

    pub async fn update(id: u64, request: OAuth2UpdateRequest) -> BichonResult<()> {
        update_impl(
            DB_MANAGER.meta_db(),
            move |rw| {
                rw.get()
                    .secondary::<OAuth2>(OAuth2Key::id, id)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            format!("The oauth2 entity with id={id} that you want to modify was not found."),
                            ErrorCode::ResourceNotFound
                        )
                    })
            },
            |current| apply_update(current, request),
        )
        .await?;

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct OAuth2CreateRequest {
    /// A description of what this configuration is used for.
    pub description: Option<String>,

    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,

    /// The client secret used in conjunction with the client ID.
    pub client_secret: String,

    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,

    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,

    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,

    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,

    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,

    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,

    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
}

impl OAuth2CreateRequest {
    pub fn encrypt(self) -> BichonResult<Self> {
        Ok(Self {
            description: self.description,
            client_id: self.client_id,
            client_secret: encrypt!(&self.client_secret)?,
            auth_url: self.auth_url,
            token_url: self.token_url,
            redirect_uri: self.redirect_uri,
            scopes: self.scopes,
            extra_params: self.extra_params,
            enabled: self.enabled,
            use_proxy: self.use_proxy,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct OAuth2UpdateRequest {
    /// A description of what this configuration is used for.
    pub description: Option<String>,

    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: Option<String>,

    /// The client secret used in conjunction with the client ID.
    pub client_secret: Option<String>,

    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: Option<String>,

    /// The URL to exchange authorization codes for access tokens.
    pub token_url: Option<String>,

    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: Option<String>,

    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,

    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,

    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: Option<bool>,

    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
}

fn apply_update(old: &OAuth2, request: OAuth2UpdateRequest) -> BichonResult<OAuth2> {
    let mut new = old.clone();
    if request.description.is_some() {
        new.description = request.description;
    }
    if let Some(client_id) = request.client_id {
        new.client_id = client_id;
    }
    if let Some(client_secret) = request.client_secret {
        new.client_secret = encrypt!(&client_secret)?;
    }
    if let Some(auth_url) = request.auth_url {
        new.auth_url = auth_url;
    }
    if let Some(token_url) = request.token_url {
        new.token_url = token_url;
    }
    if let Some(redirect_uri) = request.redirect_uri {
        new.redirect_uri = redirect_uri;
    }
    if request.scopes.is_some() {
        new.scopes = request.scopes;
    }
    if request.extra_params.is_some() {
        new.extra_params = request.extra_params;
    }
    if let Some(enabled) = request.enabled {
        new.enabled = enabled;
    }
    if let Some(use_proxy) = request.use_proxy {
        new.use_proxy = Some(use_proxy);
    }
    new.updated_at = utc_now!();
    Ok(new)
}
