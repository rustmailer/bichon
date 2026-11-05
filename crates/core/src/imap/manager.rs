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

use crate::account::entity::AuthType;
use crate::account::migration::{AccountModel, AccountType};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::imap::capabilities::{capability_to_string, check_capabilities, fetch_capabilities};
use crate::imap::client::Client;
use crate::imap::oauth2::OAuth2;
use crate::imap::session::SessionStream;
use crate::oauth2::token::OAuth2AccessToken;
use crate::{bichon_version, decrypt, raise_error};
use async_imap::Session;
use tracing::{error, warn};

/// Human-readable reason for a failed login, so the log line says *why*
/// rather than always blaming the password (#396).
fn login_failure_label(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ImapServerThrottled => "server throttling, will back off",
        ErrorCode::NetworkError => "network error or timeout",
        ErrorCode::ImapCommandFailed => "server rejected the command",
        _ => "unexpected error",
    }
}

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
        let login_name = account.login_name.clone().unwrap_or(account.email.clone());
        match &imap.auth.auth_type {
            AuthType::Password => {
                let password = &imap.auth.password.clone().ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is Passwd, but password not set".into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;

                let password = decrypt!(&password)?;
                client.login(&login_name, &password).await.map_err(|e| {
                    // "password auth failed" sent users hunting for a
                    // credential problem when the server was actually
                    // throttling or the socket timed out (#396).
                    if e.code() == ErrorCode::ImapAuthenticationFailed {
                        error!(
                            "IMAP login rejected for username '{}': {}",
                            login_name, e
                        );
                    } else {
                        error!(
                            "IMAP login for username '{}' failed ({}): {}",
                            login_name,
                            login_failure_label(e.code()),
                            e
                        );
                    }
                    e
                })
            }
            AuthType::OAuth2 => {
                let record = OAuth2AccessToken::get(account.id)?;
                let access_token = record.and_then(|r| r.access_token).ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is OAuth2, but OAuth2 authorization is not yet complete."
                            .into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;
                client
                    .authenticate(OAuth2::new(login_name.clone(), access_token))
                    .await
                    .map_err(|e| {
                        if e.code() == ErrorCode::ImapAuthenticationFailed {
                            error!(
                                "IMAP OAuth2 login rejected for username '{}': {}",
                                login_name, e
                            );
                        } else {
                            error!(
                                "IMAP OAuth2 login for username '{}' failed ({}): {}",
                                login_name,
                                login_failure_label(e.code()),
                                e
                            );
                        }
                        e
                    })
            }
        }
    }

    pub async fn build(account_id: u64) -> BichonResult<Session<Box<dyn SessionStream>>> {
        let account = AccountModel::get(account_id)?;
        let account_email = account.email.clone();

        let mut client = None;
        for attempt in 0..3u32 {
            match Self::create_client(&account).await {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(error) if error.code() == ErrorCode::NetworkError && attempt < 2 => {
                    warn!(
                        "IMAP connection attempt {}/3 to {} failed (network error), retrying...",
                        attempt + 1,
                        account_email
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                Err(error) => {
                    error!(
                        "Failed to create IMAP {}'s client: {:#?}",
                        account_email, error
                    );
                    return Err(error);
                }
            }
        }

        let client = client.ok_or_else(|| {
            raise_error!(
                format!("Failed to create IMAP {}'s client after 3 attempts", account_email),
                ErrorCode::NetworkError
            )
        })?;

        let mut session = match Self::authenticate(client, &account).await {
            Ok(session) => session,
            Err(error) => {
                error!("Failed to authenticate IMAP session: {:#?}", error);
                return Err(error);
            }
        };

        match fetch_capabilities(&mut session).await {
            Ok(capabilities) => {
                let to_save: Vec<String> = capabilities.iter().map(capability_to_string).collect();
                AccountModel::update_capabilities(account_id, to_save)?;
                if let Err(error) = check_capabilities(&capabilities) {
                    error!("Failed to check IMAP capabilities: {:#?}", error);
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
                return Err(error);
            }
        }

        Ok(session)
    }
}
