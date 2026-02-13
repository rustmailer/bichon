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

use crate::modules::common::auth::ClientContext;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::ApiResult;
use crate::modules::sync::{
    fetch_raw_emails, get_mailbox_status, sync_account_on_demand, sync_single_folder,
    verify_sync_completeness, FetchEmlRequest, MailboxStatusEntry, RawEmailExport,
    SyncFolderResult, SyncVerifyResult,
};
use crate::modules::users::permissions::Permission;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;

pub struct SyncApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Sync")]
impl SyncApi {
    /// Trigger an on-demand sync for the given IMAP account.
    ///
    /// This runs the same sync logic as the background task but
    /// is triggered manually via the API.
    #[oai(
        path = "/sync/:account_id",
        method = "post",
        operation_id = "sync_account"
    )]
    async fn sync_account(
        &self,
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(sync_account_on_demand(account_id).await?)
    }

    /// Sync a single mailbox/folder for the given IMAP account.
    #[oai(
        path = "/sync/:account_id/:mailbox_id",
        method = "post",
        operation_id = "sync_folder"
    )]
    async fn sync_folder(
        &self,
        account_id: Path<u64>,
        mailbox_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<SyncFolderResult>> {
        let account_id = account_id.0;
        let mailbox_id = mailbox_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(Json(sync_single_folder(account_id, mailbox_id).await?))
    }

    /// Get mailbox status for an account (offline, no IMAP connection).
    ///
    /// Returns each mailbox with its server count (from last sync)
    /// and the actual local indexed message count.
    #[oai(
        path = "/sync/mailbox-status/:account_id",
        method = "get",
        operation_id = "mailbox_status"
    )]
    async fn mailbox_status(
        &self,
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<Vec<MailboxStatusEntry>>> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)
            .await?;
        Ok(Json(get_mailbox_status(account_id).await?))
    }

    /// Verify sync completeness by comparing local data with the IMAP server.
    ///
    /// Returns per-mailbox counts (local vs remote) and lists any
    /// folders present on the server but missing locally.
    #[oai(
        path = "/sync/verify/:account_id",
        method = "get",
        operation_id = "verify_sync"
    )]
    async fn verify_sync(
        &self,
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<SyncVerifyResult>> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)
            .await?;
        Ok(Json(verify_sync_completeness(account_id).await?))
    }

    /// Fetch raw EML content for specific UIDs from IMAP (without storing).
    #[oai(
        path = "/sync/:account_id/:mailbox_id/fetch-eml",
        method = "post",
        operation_id = "fetch_eml"
    )]
    async fn fetch_eml(
        &self,
        account_id: Path<u64>,
        mailbox_id: Path<u64>,
        body: Json<FetchEmlRequest>,
        context: ClientContext,
    ) -> ApiResult<Json<Vec<RawEmailExport>>> {
        let account_id = account_id.0;
        let mailbox_id = mailbox_id.0;
        context
            .require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)
            .await?;
        Ok(Json(fetch_raw_emails(account_id, mailbox_id, body.0.uids).await?))
    }
}
