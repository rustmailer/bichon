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

use crate::modules::account::dispatcher::STATUS_DISPATCHER;
use crate::modules::account::entity::AuthType;
use crate::modules::account::migration::{AccountModel, AccountType};
use crate::modules::error::code::ErrorCode;
use crate::modules::error::BichonResult;
use crate::modules::imap::capabilities::{
    capability_to_string, check_capabilities, fetch_capabilities,
};
use crate::modules::imap::client::Client;
use crate::modules::imap::oauth2::OAuth2;
use crate::modules::imap::session::SessionStream;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::{bichon_version, decrypt, raise_error};
use async_imap::Session;
use tracing::{error, warn};

pub struct ImapConnectionManager;

impl ImapConnectionManager {
    async fn create_client(account: &AccountModel) -> BichonResult<Client> {
        assert_eq!(account.account_type, AccountType::IMAP);
        let imap = account.imap.as_ref().unwrap();
        Client::connection(
            &imap.host,
            &imap.encryption,
            imap.port,
            imap.use_proxy,
            account.use_dangerous,
        )
        .await
    }

    async fn authenticate(
        client: Client,
        account: &AccountModel,
    ) -> BichonResult<Session<Box<dyn SessionStream>>> {
        assert_eq!(account.account_type, AccountType::IMAP);
        let imap = account.imap.as_ref().unwrap();
        let username = account.name.clone().unwrap_or(account.email.clone());
        match &imap.auth.auth_type {
            AuthType::Password => {
                let password = &imap.auth.password.clone().ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is Passwd, but password not set".into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;

                let password = decrypt!(&password)?;
                client.login(&username, &password).await.map_err(|e| {
                    error!(
                        "IMAP password auth failed for username '{}': {}",
                        username, e
                    );
                    e
                })
            }
            AuthType::OAuth2 => {
                let record = OAuth2AccessToken::get(account.id).await?;
                let access_token = record.and_then(|r| r.access_token).ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is OAuth2, but OAuth2 authorization is not yet complete."
                            .into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;
                client
                    .authenticate(OAuth2::new(username.clone(), access_token))
                    .await
                    .map_err(|e| {
                        error!("IMAP OAuth2 auth failed for username '{}': {}", username, e);
                        e
                    })
            }
        }
    }

    pub async fn build(account_id: u64) -> BichonResult<Session<Box<dyn SessionStream>>> {
        let account = AccountModel::async_get(account_id).await?;
        let client = match Self::create_client(&account).await {
            Ok(client) => client,
            Err(error) => {
                error!(
                    "Failed to create IMAP {}'s client: {:#?}",
                    &account.email, error
                );
                STATUS_DISPATCHER
                    .append_error(
                        account_id,
                        format!("imap client connect error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        };

        let mut session = match Self::authenticate(client, &account).await {
            Ok(session) => session,
            Err(error) => {
                error!("Failed to authenticate IMAP session: {:#?}", error);
                STATUS_DISPATCHER
                    .append_error(
                        account_id,
                        format!("imap client authenticate error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        };

        match fetch_capabilities(&mut session).await {
            Ok(capabilities) => {
                let to_save: Vec<String> = capabilities.iter().map(capability_to_string).collect();
                AccountModel::update_capabilities(account_id, to_save).await?;
                if let Err(error) = check_capabilities(&capabilities) {
                    error!("Failed to check IMAP capabilities: {:#?}", error);
                    STATUS_DISPATCHER
                        .append_error(
                            account_id,
                            format!("imap client check capabilities error: {:#?}", error),
                        )
                        .await;
                    return Err(error);
                }

                if capabilities.has_str("ID") || capabilities.has_str("id") {
                    if let Err(e) = session
                        .id([
                            ("name", Some("bichon")),
                            ("version", Some(bichon_version!())),
                            ("vendor", Some("rustmailer")),
                        ])
                        .await
                    {
                        warn!("IMAP ID command failed (ignored): {:#?}", e);
                    }
                }
            }
            Err(error) => {
                error!("Failed to fetch IMAP capabilities: {:#?}", error);
                STATUS_DISPATCHER
                    .append_error(
                        account_id,
                        format!("imap client fetch capabilities error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        }

        Ok(session)
    }
}
