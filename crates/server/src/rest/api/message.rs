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

use crate::common::auth::WrappedContext;
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;
use bichon_core::account::migration::AccountModel;
use bichon_core::common::paginated::DataPage;
use bichon_core::error::code::ErrorCode;
use bichon_core::message::append::restore_emails;
use bichon_core::message::append::RestoreMessagesRequest;
use bichon_core::message::attachment::retrieve_attachment_content_self_healing;
use bichon_core::message::attachment::retrieve_nested_attachment_content;
use bichon_core::message::content::retrieve_nested_eml_content;
use bichon_core::message::content::FullNestedMessageContent;
use bichon_core::message::content::{retrieve_email_content_self_healing, FullMessageContent};
use bichon_core::message::delete::delete_messages_impl;
use bichon_core::message::list::get_thread_messages;
use bichon_core::message::search::{search_messages_impl, EmailSearchRequest};
use bichon_core::message::tags::TagCount;
use bichon_core::message::tags::TagsRequest;
use bichon_core::raise_error;
use bichon_core::store::blob::get_reader;
use bichon_core::store::envelope::Envelope;
use bichon_core::store::tantivy::envelope::ENVELOPE_MANAGER;
use bichon_core::store::tantivy::validate_facet;
use bichon_core::users::permissions::Permission;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Attachment, AttachmentType, Json};
use poem_openapi::OpenApi;
use std::collections::HashMap;
use std::collections::HashSet;

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
        payload: Json<HashMap<u64, Vec<String>>>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let request = payload.0;
        for account_id in request.keys() {
            context.require_permission(Some(*account_id), Permission::DATA_DELETE)?;
        }
        Ok(delete_messages_impl(request).await?)
    }

    /// Searches messages across all mailboxes using various filter criteria.
    /// The search filters are provided in the request body.
    #[oai(
        path = "/search-messages",
        method = "post",
        operation_id = "search_messages"
    )]
    async fn search_messages(
        &self,
        payload: Json<EmailSearchRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<DataPage<Envelope>>> {
        let authorized_ids: Option<HashSet<u64>> =
            if context.has_permission(None, Permission::DATA_READ_ALL) {
                None
            } else {
                Some(context.user.account_access_map.keys().cloned().collect())
            };
        Ok(Json(search_messages_impl(authorized_ids, payload.0)?))
    }

    /// Retrieves all messages belonging to a specific thread. Requires `thread_id`, `page`, and `page_size` query parameters.
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
        thread_id: Query<String>,
        /// The page number for pagination (1-based).
        page: Query<u64>,
        /// The number of messages per page.
        page_size: Query<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<DataPage<Envelope>>> {
        let account_id = account_id.0;
        let thread_id = thread_id.0.trim();
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        Ok(Json(get_thread_messages(
            account_id,
            thread_id,
            page.0,
            page_size.0,
        )?))
    }

    /// Fetches the content of a specific email.
    /// Set `block_remote_content=true` to strip external images, scripts,
    /// and other content loaded from http/https URLs.
    #[oai(
        path = "/message-content/:account_id/:envelope_id",
        method = "get",
        operation_id = "fetch_message_content"
    )]
    async fn fetch_message_content(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to fetch.
        envelope_id: Path<String>,
        /// Block remote content (http/https URLs) from email body.
        block_remote_content: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<FullMessageContent>> {
        let account_id = account_id.0;
        let block_remote = block_remote_content.0.unwrap_or(false);
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        Ok(Json(
            retrieve_email_content_self_healing(account_id, envelope_id.0, block_remote).await?,
        ))
    }

    /// Retrieves the content of an email embedded as an attachment.
    #[oai(
        path = "/nested-message-content/:account_id/:envelope_id",
        method = "get",
        operation_id = "fetch_nested_message_content"
    )]
    async fn fetch_nested_message_content(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to fetch.
        envelope_id: Path<String>,
        content_hash: Query<String>,
        block_remote_content: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<FullNestedMessageContent>> {
        let account_id = account_id.0;
        let block_remote = block_remote_content.0.unwrap_or(false);
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        let content_hash = content_hash.0.trim();
        Ok(Json(retrieve_nested_eml_content(
            account_id,
            envelope_id.0,
            content_hash,
            block_remote,
        )?))
    }

    /// Retrieves the envelope (metadata) of a specific message.
    #[oai(
        path = "/envelope/:account_id/:envelope_id",
        method = "get",
        operation_id = "get_envelope"
    )]
    async fn get_envelope(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message.
        envelope_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<Envelope>> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        let envelope_id = envelope_id.0;
        let e = ENVELOPE_MANAGER
            .get_envelope_by_id(account_id, &envelope_id)?
            .ok_or_else(|| {
                raise_error!(
                    format!(
                        "Envelope not found: account_id={} envelope_id={}",
                        account_id, &envelope_id
                    ),
                    ErrorCode::ResourceNotFound
                )
            })?;
        Ok(Json(e.envelope))
    }

    /// Downloads the raw EML file of a specific email.
    #[oai(
        path = "/download-message/:account_id/:envelope_id",
        method = "get",
        operation_id = "download_message"
    )]
    async fn download_message(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to download.
        envelope_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::DATA_RAW_DOWNLOAD)?;
        let envelope_id = envelope_id.0;
        let reader = get_reader(account_id, envelope_id.clone()).await?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(format!("{envelope_id}.eml"));
        Ok(attachment)
    }

    /// Restore an email to an account's IMAP server.
    #[oai(
        path = "/restore-messages/:account_id",
        method = "post",
        operation_id = "restore_messages"
    )]
    async fn restore_messages(
        &self,
        account_id: Path<u64>,
        /// Message IDs to restore.
        payload: Json<RestoreMessagesRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::DATA_EXPORT_BATCH)?;
        Ok(restore_emails(account_id, payload.0.envelope_ids).await?)
    }

    /// Downloads a specific attachment from an email. Requires `content_hash` query parameter.
    #[oai(
        path = "/download-attachment/:account_id/:envelope_id",
        method = "get",
        operation_id = "download_attachment"
    )]
    async fn download_attachment(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message containing the attachment.
        envelope_id: Path<String>,
        /// The content_hash of the attachment to download.
        content_hash: Query<String>,
        context: WrappedContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        let envelope_id = envelope_id.0.trim().to_string();
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        let content_hash = content_hash.0.trim();
        let reader =
            retrieve_attachment_content_self_healing(account_id, envelope_id, content_hash).await?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(content_hash);
        Ok(attachment)
    }

    /// Returns raw attachment content for in-browser preview with
    /// `Content-Disposition: inline` and the correct MIME type.
    #[oai(
        path = "/preview-attachment/:account_id/:envelope_id",
        method = "get",
        operation_id = "preview_attachment"
    )]
    async fn preview_attachment(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message containing the attachment.
        envelope_id: Path<String>,
        /// The content_hash of the attachment to preview.
        content_hash: Query<String>,
        context: WrappedContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        let envelope_id = envelope_id.0.trim().to_string();
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        let content_hash = content_hash.0.trim();
        let reader =
            retrieve_attachment_content_self_healing(account_id, envelope_id, content_hash).await?;
        let body = Body::from_async_read(reader);
        Ok(Attachment::new(body).attachment_type(AttachmentType::Inline))
    }

    /// Downloads an attachment from within a nested email (EML file).
    #[oai(
        path = "/download-nested-attachment/:account_id/:envelope_id",
        method = "get",
        operation_id = "download_nested_attachment"
    )]
    async fn download_nested_attachment(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message containing the attachment.
        envelope_id: Path<String>,
        /// The filename of the attachment to download.
        content_hash: Query<String>,
        nested_content_hash: Query<String>,
        context: WrappedContext,
    ) -> ApiResult<Attachment<Body>> {
        let account_id = account_id.0;
        let envelope_id = envelope_id.0.trim().to_string();
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::DATA_READ)?;
        let content_hash = content_hash.0.trim();
        let nested_content_hash = nested_content_hash.0.trim();
        let reader = retrieve_nested_attachment_content(
            account_id,
            envelope_id,
            content_hash,
            nested_content_hash,
        )?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(nested_content_hash);
        Ok(attachment)
    }

    /// Returns all facets in the index along with their document counts.
    #[oai(path = "/all-tags", method = "get", operation_id = "get_all_tags")]
    async fn get_all_tags(&self, context: WrappedContext) -> ApiResult<Json<Vec<TagCount>>> {
        let authorized_ids: Option<HashSet<u64>> =
            if context.has_permission(None, Permission::DATA_READ_ALL) {
                None
            } else {
                Some(context.user.account_access_map.keys().cloned().collect())
            };
        Ok(Json(ENVELOPE_MANAGER.get_all_tags(authorized_ids)?))
    }

    /// Adds or removes facet tags for multiple emails across accounts.
    #[oai(
        path = "/update-tags",
        method = "post",
        operation_id = "update_envelope_tags"
    )]
    async fn update_envelope_tags(
        &self,
        req: Json<TagsRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let req = req.0;
        for tag in &req.tags {
            validate_facet(tag)?;
        }

        for account_id in req.updates.keys() {
            context.require_permission(Some(*account_id), Permission::DATA_MANAGE)?;
        }

        ENVELOPE_MANAGER.update_envelope_tags(req).await?;
        Ok(())
    }

    /// Retrieves a unique list of all contact email addresses across authorized accounts.
    #[oai(
        path = "/all-contacts",
        method = "get",
        operation_id = "get_all_contacts"
    )]
    async fn get_all_contacts(&self, context: WrappedContext) -> ApiResult<Json<HashSet<String>>> {
        let authorized_ids: Option<HashSet<u64>> =
            if context.has_permission(None, Permission::DATA_READ_ALL) {
                None
            } else {
                Some(context.user.account_access_map.keys().cloned().collect())
            };
        Ok(Json(ENVELOPE_MANAGER.get_all_contacts(authorized_ids)?))
    }
}
