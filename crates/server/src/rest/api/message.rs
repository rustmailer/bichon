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
use bichon_core::envelope::extractor::{repair_envelope_blobs, BlobRepairReport};
use bichon_core::error::code::ErrorCode;
use bichon_core::message::append::restore_emails;
use bichon_core::message::append::RestoreMessagesRequest;
use bichon_core::message::attachment::retrieve_attachment_content;
use bichon_core::message::attachment::retrieve_nested_attachment_content;
use bichon_core::message::content::retrieve_nested_eml_content;
use bichon_core::message::content::FullNestedMessageContent;
use bichon_core::message::content::{retrieve_email_content, FullMessageContent};
use bichon_core::message::delete::{
    delete_messages_impl, emit_delete_audit, plan_delete_audit,
};
use bichon_core::ext::event_bus::{emit, Event};
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
        // Audit: plan the records and capture whatever must be captured before
        // the messages are gone — inline snapshots, or a detail file for large
        // batches — so the trail stays self-describing.
        let user = context.user.username.clone();
        let plan = plan_delete_audit(&request);
        let result = delete_messages_impl(request).await;
        // Emit only on success: a failed delete must not leave "deleted"
        // records for messages that still exist.
        if result.is_ok() {
            emit_delete_audit(plan, &user);
        }
        result?;
        Ok(())
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
        let search_text = payload
            .0
            .filter
            .text
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        let user = context.user.username.clone();
        let result = search_messages_impl(authorized_ids, payload.0)?;
        if !search_text.is_empty() {
            emit(Event::SearchPerformed {
                query: search_text,
                user,
            });
        }
        Ok(Json(result))
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
        let envelope_id = envelope_id.0.trim().to_string();
        let envelope = ENVELOPE_MANAGER
            .get_envelope_by_id(account_id, &envelope_id)?
            .map(|ea| ea.envelope);
        let content = retrieve_email_content(account_id, envelope_id.clone(), block_remote)?;
        if let Some(ip) = context.ip_addr {
            emit(Event::EmailViewed {
                email_id: envelope_id.clone(),
                user: context.user.username.clone(),
                ip,
                account_id,
                mailbox_id: envelope.as_ref().map(|e| e.mailbox_id).unwrap_or(0),
                subject: envelope.as_ref().map(|e| e.subject.clone()),
            });
        }
        Ok(Json(content))
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
        let subject = ENVELOPE_MANAGER
            .get_envelope_by_id(account_id, &envelope_id)
            .ok()
            .flatten()
            .map(|ea| ea.envelope.subject.clone());
        emit(Event::EmailExported {
            email_id: envelope_id.clone(),
            user: context.user.username.clone(),
            account_id,
            subject,
        });
        let reader = get_reader(account_id, envelope_id.clone()).await?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(format!("{envelope_id}.eml"));
        Ok(attachment)
    }

    /// Re-fetches one archived message from IMAP and restores missing email or
    /// attachment blobs in place. The envelope and its id are kept, so no
    /// duplicate is created; nothing is written unless the fetched message
    /// hashes to the archived content hash.
    #[oai(
        path = "/repair-message/:account_id/:envelope_id",
        method = "post",
        operation_id = "repair_message"
    )]
    async fn repair_message(
        &self,
        /// The ID of the account.
        account_id: Path<u64>,
        /// The ID of the message to repair.
        envelope_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<BlobRepairReport>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::DATA_MANAGE)?;
        Ok(Json(repair_envelope_blobs(account_id, &envelope_id.0).await?))
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
        for eid in &payload.0.envelope_ids {
            let subject = ENVELOPE_MANAGER
                .get_envelope_by_id(account_id, eid)
                .ok()
                .flatten()
                .map(|ea| ea.envelope.subject.clone());
            emit(Event::EmailRestored {
                email_id: eid.clone(),
                user: context.user.username.clone(),
                account_id,
                subject,
            });
        }
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
        let meta = attachment_meta_for_audit(account_id, &envelope_id, content_hash);
        let reader = retrieve_attachment_content(account_id, envelope_id.clone(), content_hash)?;
        emit(Event::AttachmentDownloaded {
            email_id: envelope_id.clone(),
            content_hash: content_hash.to_string(),
            user: context.user.username.clone(),
            account_id,
            mailbox_id: meta.mailbox_id,
            filename: meta.filename,
            size: meta.size,
            ext: meta.ext,
            parent_content_hash: meta.parent_content_hash,
        });
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
        let meta = attachment_meta_for_audit(account_id, &envelope_id, content_hash);
        let reader = retrieve_attachment_content(account_id, envelope_id.clone(), content_hash)?;
        emit(Event::AttachmentPreviewed {
            email_id: envelope_id.clone(),
            content_hash: content_hash.to_string(),
            user: context.user.username.clone(),
            account_id,
            mailbox_id: meta.mailbox_id,
            filename: meta.filename,
        });
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
        let meta = attachment_meta_for_audit(account_id, &envelope_id, content_hash);
        let reader = retrieve_nested_attachment_content(
            account_id,
            envelope_id.clone(),
            content_hash,
            nested_content_hash,
        )?;
        emit(Event::AttachmentDownloaded {
            email_id: envelope_id.clone(),
            content_hash: nested_content_hash.to_string(),
            user: context.user.username.clone(),
            account_id,
            mailbox_id: meta.mailbox_id,
            filename: None,
            size: None,
            ext: None,
            parent_content_hash: meta.parent_content_hash,
        });
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

        let total_updates: u64 = req.updates.values().map(|ids| ids.len() as u64).sum();
        if total_updates > 0 {
            for account_id in req.updates.keys() {
                emit(Event::EmailTagged {
                    user: context.user.username.clone(),
                    account_id: *account_id,
                    count: req
                        .updates
                        .get(account_id)
                        .map(|ids| ids.len() as u64)
                        .unwrap_or(0),
                });
            }
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

/// Attachment metadata captured for the audit trail.
struct AttachmentAuditMeta {
    mailbox_id: u64,
    filename: Option<String>,
    size: Option<u64>,
    ext: Option<String>,
    parent_content_hash: Option<String>,
}

impl Default for AttachmentAuditMeta {
    fn default() -> Self {
        Self {
            mailbox_id: 0,
            filename: None,
            size: None,
            ext: None,
            parent_content_hash: None,
        }
    }
}

/// Resolves attachment display metadata (name, size, extension) and the
/// parent envelope's mailbox/content hash, for the audit trail. Best-effort:
/// failures degrade to defaults rather than failing the download.
fn attachment_meta_for_audit(
    account_id: u64,
    envelope_id: &str,
    content_hash: &str,
) -> AttachmentAuditMeta {
    let mut meta = AttachmentAuditMeta::default();
    if let Ok(Some(ea)) = ENVELOPE_MANAGER.get_envelope_by_id(account_id, envelope_id) {
        meta.mailbox_id = ea.envelope.mailbox_id;
        meta.parent_content_hash = Some(ea.envelope.content_hash);
        if let Some(atts) = ea.attachments {
            for att in atts {
                if att.content_hash == content_hash {
                    meta.filename = att.filename.clone();
                    meta.size = Some(att.size as u64);
                    meta.ext = att
                        .filename
                        .as_ref()
                        .and_then(|n| std::path::Path::new(n).extension())
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_ascii_lowercase());
                    break;
                }
            }
        }
    }
    meta
}

// The delete-batch audit helpers used to live here. They moved to
// `bichon_core::message::delete` when the dual-control executor became a
// second caller: a handler-private copy would have left the approval path
// emitting `email.deleted` records with no subject.
