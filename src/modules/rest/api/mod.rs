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

use access_token::AccessTokenApi;
use account::AccountApi;
use auto_config::AutoConfigApi;
use mailbox::MailBoxApi;
use message::MessageApi;
use oauth2::OAuth2Api;
use poem_openapi::{OpenApiService, Tags};
use sync::SyncApi;
use system::SystemApi;

use crate::{
    bichon_version,
    modules::rest::api::{import::ImportApi, users::UsersApi},
};

pub mod access_token;
pub mod account;
pub mod auto_config;
pub mod import;
pub mod mailbox;
pub mod message;
pub mod oauth2;
pub mod sync;
pub mod system;
pub mod users;

#[derive(Tags)]
pub enum ApiTags {
    AccessToken,
    AutoConfig,
    Account,
    Mailbox,
    OAuth2,
    Message,
    Sync,
    System,
    Import,
    Users,
}

type RustMailOpenApi = (
    AccessTokenApi,
    AutoConfigApi,
    AccountApi,
    SystemApi,
    MailBoxApi,
    OAuth2Api,
    MessageApi,
    ImportApi,
    UsersApi,
    SyncApi,
);

pub fn create_openapi_service() -> OpenApiService<RustMailOpenApi, ()> {
    OpenApiService::new(
        (
            AccessTokenApi,
            AutoConfigApi,
            AccountApi,
            SystemApi,
            MailBoxApi,
            OAuth2Api,
            MessageApi,
            ImportApi,
            UsersApi,
            SyncApi,
        ),
        "BichonApi",
        bichon_version!(),
    )
}
