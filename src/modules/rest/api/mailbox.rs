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

use crate::modules::cache::imap::mailbox::MailBox;
use crate::modules::common::auth::ClientContext;
use crate::modules::mailbox::list::get_account_mailboxes;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::ApiResult;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;

pub struct MailBoxApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Mailbox")]
impl MailBoxApi {
    /// Returns all available mailboxes for the given account.
    ///
    /// - For IMAP/SMTP accounts, this corresponds to folders/mailboxes.
    /// - For Gmail API accounts, this corresponds to labels visible via the
    ///   `list messages` API (serving as mailbox equivalents).
    ///
    /// Both account types support two modes:
    /// - Using the local cache of mailboxes/labels.
    /// - Querying the remote service directly for the latest state.
    #[oai(
        path = "/list-mailboxes/:account_id",
        method = "get",
        operation_id = "list_mailboxes"
    )]
    async fn list_mailboxes(
        &self,
        /// The unique identifier of the account.
        account_id: Path<u64>,
        remote: Query<Option<bool>>,
        context: ClientContext,
    ) -> ApiResult<Json<Vec<MailBox>>> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)
            .await?;
        let remote = remote.0.unwrap_or(false);
        Ok(Json(get_account_mailboxes(account_id, remote).await?))
    }
}
