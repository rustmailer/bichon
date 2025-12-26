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
use crate::modules::indexer::envelope::Envelope;
use crate::modules::indexer::manager::EML_INDEX_MANAGER;
use crate::modules::indexer::manager::ENVELOPE_INDEX_MANAGER;
use crate::modules::message::content::{retrieve_email_content, FullMessageContent};
use crate::modules::message::delete::delete_messages_impl;
use crate::modules::message::list::{get_thread_messages, list_messages_impl};
use crate::modules::message::search::{search_messages_impl, SearchRequest};
use crate::modules::message::tags::TagCount;
use crate::modules::message::tags::UpdateTagsRequest;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::response::DataPage;
use crate::modules::rest::ApiResult;
use crate::modules::rest::ErrorCode;
use crate::modules::users::permissions::Permission;
use crate::raise_error;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Attachment, AttachmentType, Json};
use poem_openapi::OpenApi;
use std::collections::HashMap;
use std::collections::HashSet;
use tantivy::schema::Facet;

pub struct MessageApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Message")]
impl MessageApi {
    /// Deletes messages from a mailbox or moves them to the trash for the specified account.
    #[oai(
        path = "/delete-messages",
        method = "post",
        operation_id = "delete_messages"
    )]
    async fn delete_messages(
        &self,
        /// specifying the mailbox and messages to delete.
        payload: Json<HashMap<u64, Vec<u64>>>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let request = payload.0;
        for account_id in request.keys() {
            context
                .require_permission(Some(*account_id), Permission::DATA_DELETE)
                .await?;
        }
        Ok(delete_messages_impl(request).await?)
    }

    /// Lists messages in a specified mailbox for the given account.
    #[oai(
        path = "/list-messages/:account_id",
        method = "get",
        operation_id = "list_messages"
    )]
    async fn list_messages(
        &self,
        account_id: Path<u64>,
        mailbox_id: Query<u64>,
        page: Query<u64>,
        page_size: Query<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<Envelope>>> {
        let account_id = account_id.0;
        let mailbox_id = mailbox_id.0;
        context
            .require_permission(Some(account_id), Permission::DATA_READ)
            .await?;
        Ok(Json(
            list_messages_impl(account_id, mailbox_id, page.0, page_size.0).await?,
        ))
    }

    /// Lists messages in a specified mailbox for the given account.
    #[oai(
        path = "/search-messages",
        method = "post",
        operation_id = "search_messages"
    )]
    async fn search_messages(
        &self,
        payload: Json<SearchRequest>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<Envelope>>> {
        let authorized_ids: Option<HashSet<u64>> = if context
            .has_permission(None, Permission::DATA_READ_ALL)
            .await
        {
            None
        } else {
            Some(context.user.account_access_map.keys().cloned().collect())
        };
        Ok(Json(search_messages_impl(authorized_ids, payload.0).await?))
    }

    /// Get thread's envelopes in a specified mailbox for the given account.
    #[oai(
        path = "/get-thread-messages/:account_id",
        method = "get",
        operation_id = "get_thread_messages"
    )]
    async fn get_thread_messages(
        &self,
        /// The ID of the account owning the mailbox.
        account_id: Path<u64>,
        // Thread ID
        thread_id: Query<u64>,
        /// The page number for pagination (1-based).
        page: Query<u64>,
        /// The number of messages per page.
        page_size: Query<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<Envelope>>> {
        let account_id = account_id.0;
        let thread_id = thread_id.0;
        context
            .require_permission(Some(account_id), Permission::DATA_READ)
            .await?;
        Ok(Json(
            get_thread_messages(account_id, thread_id, page.0, page_size.0).await?,
        ))
    }

    /// Fetches the content of a specific email for the given account.
    #[oai(
        path = "/message-content/:account_id",
        method = "get",
        operation_id = "fetch_message_content"
    )]
    async fn fetch_message_content(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to fetch.
        message_id: Query<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<FullMessageContent>> {
        let account_id = account_id.0;
        context
            .require_permission(Some(account_id), Permission::DATA_READ)
            .await?;
        Ok(Json(retrieve_email_content(account_id, message_id.0).await?))
    }

    /// Fetches the full content of a specific email for the given account.
    #[oai(
        path = "/download-message/:account_id",
        method = "get",
        operation_id = "download_message"
    )]
    async fn download_message(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to download.
        message_id: Query<u64>,
        context: ClientContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id).await?;
        context
            .require_permission(Some(account_id), Permission::DATA_RAW_DOWNLOAD)
            .await?;
        let message_id = message_id.0;
        let reader = EML_INDEX_MANAGER.get_reader(account_id, message_id).await?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(format!("{message_id}.eml"));
        Ok(attachment)
    }

    /// Downloads a specific attachment by filename.
    #[oai(
        path = "/download-attachment/:account_id",
        method = "get",
        operation_id = "download_attachment"
    )]
    async fn download_attachment(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message containing the attachment.
        message_id: Query<u64>,
        /// The filename of the attachment to download.
        name: Query<String>,
        context: ClientContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id).await?;
        context
            .require_permission(Some(account_id), Permission::DATA_READ)
            .await?;
        let message_id = message_id.0;
        let name = name.0.trim();
        let reader = EML_INDEX_MANAGER
            .get_attachment(account_id, message_id, name)
            .await?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(name);
        Ok(attachment)
    }
    /// Returns all facets in the index along with their document counts.
    #[oai(path = "/all-tags", method = "get", operation_id = "get_all_tags")]
    async fn get_all_tags(&self, context: ClientContext) -> ApiResult<Json<Vec<TagCount>>> {
        let authorized_ids: Option<HashSet<u64>> = if context
            .has_permission(None, Permission::DATA_READ_ALL)
            .await
        {
            None
        } else {
            Some(context.user.account_access_map.keys().cloned().collect())
        };
        Ok(Json(
            ENVELOPE_INDEX_MANAGER.get_all_tags(authorized_ids).await?,
        ))
    }

    /// Adds or removes facet tags for multiple emails across accounts.
    #[oai(
        path = "/update-tags",
        method = "post",
        operation_id = "update_envelope_tags"
    )]
    async fn update_envelope_tags(
        &self,
        req: Json<UpdateTagsRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let req = req.0;
        for tag in &req.tags {
            Facet::from_text(tag)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
        }

        for account_id in req.updates.keys() {
            context
                .require_permission(Some(*account_id), Permission::DATA_MANAGE)
                .await?;
        }

        ENVELOPE_INDEX_MANAGER
            .update_envelope_tags(req.updates, req.tags)
            .await?;
        Ok(())
    }
}
