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
use bichon_core::account::access::search_scope;
use bichon_core::account::migration::AccountModel;
use bichon_core::common::auth::ClientContext;
use bichon_core::common::paginated::DataPage;
use bichon_core::message::batch_delete::{
    self, BatchDeleteJob, BatchDeletePreviewView, BatchDeleteRequest,
};
use bichon_core::envelope::extractor::{repair_envelope_blobs, BlobRepairReport};
use bichon_core::envelope::repair::BlobRepairRecord;
use bichon_core::error::code::ErrorCode;
use bichon_core::message::append::restore_emails;
use bichon_core::message::append::RestoreMessagesRequest;
use bichon_core::message::attachment::retrieve_attachment_content;
use bichon_core::message::attachment::retrieve_nested_attachment_content;
use bichon_core::message::content::retrieve_nested_eml_content;
use bichon_core::message::content::FullNestedMessageContent;
use bichon_core::message::contacts::Contact;
use bichon_core::message::content::{retrieve_email_content, FullMessageContent};
use bichon_core::message::delete::{
    delete_messages_impl, emit_delete_audit, plan_delete_audit,
};
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::message::list::get_thread_messages;
use bichon_core::message::search::{search_messages_scoped, EmailSearchRequest};
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

    /// Estimates how many messages a batch deletion would remove, and whether
    /// any account in scope is under a legal hold.
    #[oai(
        path = "/batch-delete/preview",
        method = "post",
        operation_id = "batch_delete_preview"
    )]
    async fn batch_delete_preview(
        &self,
        payload: Json<BatchDeleteRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<BatchDeletePreviewView>> {
        let request = payload.0;
        let scope = search_scope(&context, request.filter.account_ids.as_ref())?;
        // A caller may read more than they may delete. Resolve the split once,
        // then report both halves: the job will touch `account_ids` only.
        let (delete_scope, skipped_account_ids) = batch_delete::split_delete_scope(
            scope.clone(),
            |id| context.has_permission(Some(id), Permission::DATA_DELETE),
        )?;
        let account_ids =
            batch_delete::resolve_account_ids(&Some(delete_scope.clone()), &request.filter)?;
        let matched = search_messages_scoped(
            scope,
            EmailSearchRequest {
                filter: request.filter.clone(),
                page: 1,
                page_size: 1,
                sort_by: None,
                desc: None,
            },
        )?
        .total_items;
        let total = if skipped_account_ids.is_empty() {
            matched
        } else {
            search_messages_scoped(
                Some(delete_scope),
                EmailSearchRequest {
                    filter: request.filter,
                    page: 1,
                    page_size: 1,
                    sort_by: None,
                    desc: None,
                },
            )?
            .total_items
        };
        let held_account_ids = account_ids
            .iter()
            .filter(|id| {
                AccountModel::get(**id)
                    .map(|account| account.is_on_hold())
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        let retention_locked_account_ids = account_ids
            .iter()
            .filter(|id| {
                AccountModel::get(**id)
                    .map(|account| account.min_retention_days_effective() > 0)
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        Ok(Json(BatchDeletePreviewView {
            total,
            matched,
            skipped_no_permission: matched.saturating_sub(total),
            account_ids,
            skipped_account_ids,
            held_account_ids,
            retention_locked_account_ids,
        }))
    }

    /// Starts an asynchronous deletion of every message matching the filter.
    #[oai(
        path = "/batch-delete",
        method = "post",
        operation_id = "create_batch_delete"
    )]
    async fn create_batch_delete(
        &self,
        payload: Json<BatchDeleteRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<BatchDeleteJob>> {
        let request = payload.0;
        let scope = search_scope(&context, request.filter.account_ids.as_ref())?;
        // Keep the community rule that an unrestricted caller needs at least
        // one search condition (an empty filter there means "the whole
        // archive"). After the scope is materialized below, `start_job` can no
        // longer see that it started unrestricted.
        if scope.is_none() && request.filter.is_empty() {
            return Err(raise_error!(
                "A batch deletion needs at least one search condition.".to_string(),
                ErrorCode::InvalidParameter
            )
            .into());
        }
        let (delete_scope, _skipped_account_ids) = batch_delete::split_delete_scope(
            scope,
            |id| context.has_permission(Some(id), Permission::DATA_DELETE),
        )?;
        if delete_scope.is_empty() {
            return Err(raise_error!(
                format!(
                    "Access Denied: you do not hold '{}' on any account matching this search.",
                    Permission::DATA_DELETE
                ),
                ErrorCode::Forbidden
            )
            .into());
        }
        let account_ids =
            batch_delete::resolve_account_ids(&Some(delete_scope.clone()), &request.filter)?;
        let job = batch_delete::start_job(
            Some(delete_scope),
            request,
            account_ids,
            context.user.id,
            context.user.username.clone(),
        )?;
        Ok(Json(job))
    }

    /// Lists the most recent batch deletion jobs (creator sees their own;
    /// administrators see all).
    #[oai(
        path = "/batch-delete",
        method = "get",
        operation_id = "list_batch_deletes"
    )]
    async fn list_batch_deletes(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<BatchDeleteJob>>> {
        let mut jobs = batch_delete::list_jobs()?;
        if !context.user.is_admin() {
            jobs.retain(|job| job.user_id == context.user.id);
        }
        Ok(Json(jobs))
    }

    /// Returns one batch deletion job.
    #[oai(
        path = "/batch-delete/:job_id",
        method = "get",
        operation_id = "get_batch_delete"
    )]
    async fn get_batch_delete(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<BatchDeleteJob>> {
        let job = batch_delete::get_job(&job_id.0)?.ok_or_else(|| {
            raise_error!(
                format!("Batch deletion job '{}' not found.", job_id.0),
                ErrorCode::ResourceNotFound
            )
        })?;
        require_job_visible(&context, &job)?;
        Ok(Json(job))
    }

    /// Asks a running batch deletion to stop. The job stops between batches;
    /// already-deleted messages are not restored.
    #[oai(
        path = "/batch-delete/:job_id/cancel",
        method = "post",
        operation_id = "cancel_batch_delete"
    )]
    async fn cancel_batch_delete(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<BatchDeleteJob>> {
        let job = batch_delete::get_job(&job_id.0)?.ok_or_else(|| {
            raise_error!(
                format!("Batch deletion job '{}' not found.", job_id.0),
                ErrorCode::ResourceNotFound
            )
        })?;
        require_job_visible(&context, &job)?;
        Ok(Json(batch_delete::request_cancel(&job_id.0)?))
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
        let search_text = payload
            .0
            .filter
            .text
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        let user = context.user.username.clone();
        // Resolve view accounts into `(source account AND source filter)`
        // groups. For an ordinary search this produces the same account set the
        // old `authorized_ids` did and the query is unchanged; for a view it
        // *narrows* to the view's sources and filters.
        let scope = bichon_core::account::access::search_scope(
            &context,
            payload.0.filter.account_ids.as_ref(),
        )?;
        let result = search_messages_scoped(scope, payload.0)?;
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
        // A view reads matching messages, not whole threads: expanding a
        // thread would list siblings outside the view's filter, and filtering
        // *after* pagination would misreport the total. Refuse instead of
        // leaking or lying; the viewer can search.
        if !context.has_permission(Some(account_id), Permission::DATA_READ) {
            return Err(raise_error!(
                "Access denied: the thread view is not available through a view account; use search instead".to_string(),
                ErrorCode::Forbidden
            ))?;
        }
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
        let envelope_id = envelope_id.0.trim().to_string();
        // Direct permission, or a view whose filter covers this message. The
        // view, when there is one, goes into the audit record: "who read this"
        // is not auditable without "under which delegation".
        let (envelope, via_view) = bichon_core::account::access::load_readable_with_access(
            &context,
            account_id,
            &envelope_id,
        )?;
        let content = retrieve_email_content(account_id, envelope_id.clone(), block_remote)?;
        if let Some(ip) = context.ip_addr {
            emit(Event::EmailViewed {
                email_id: envelope_id.clone(),
                user: context.user.username.clone(),
                ip,
                account_id,
                mailbox_id: envelope.mailbox_id,
                subject: Some(envelope.subject.clone()),
                via_view,
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
        let envelope_id = envelope_id.0.trim().to_string();
        // The embedded message belongs to its parent: if the parent is
        // readable (directly or through a view), so is the nested message.
        bichon_core::account::access::load_readable(&context, account_id, &envelope_id)?;
        let content_hash = content_hash.0.trim();
        Ok(Json(retrieve_nested_eml_content(
            account_id,
            envelope_id,
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
        let envelope_id = envelope_id.0;
        let envelope = bichon_core::account::access::load_readable(
            &context,
            account_id,
            &envelope_id,
        )?;
        Ok(Json(envelope))
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
        let report = repair_envelope_blobs(account_id, &envelope_id.0).await?;
        // Audited before the history record: `emit` is fire-and-forget, while
        // the persist below can fail, and a repair that is missing from the
        // history ring still belongs in the audit trail.
        emit(Event::EmailRepaired {
            email_id: report.envelope_id.clone(),
            user: context.user.username.clone(),
            ip: context.ip_addr,
            account_id: report.account_id,
            mailbox_id: report.mailbox_id,
            subject: report.subject.clone(),
            status: report.status.clone(),
            email_missing_before: report.email_missing_before,
            email_missing_after: report.email_missing_after,
            missing_before: report.missing_before.len(),
            missing_after: report.missing_after.len(),
            skipped_due_to_backup: report.skipped_due_to_backup,
            error: report.error.clone(),
        });
        // Best-effort: failing to record the attempt must not fail the repair,
        // which has already succeeded by this point.
        if let Err(e) = BlobRepairRecord::persist(context.user.id, &context.user.username, &report) {
            tracing::warn!("failed to persist blob repair record: {:#?}", e);
        }
        Ok(Json(report))
    }

    /// Lists recent blob-repair reports, newest first.
    ///
    /// Scoped to the accounts the caller may repair: the filter is the same
    /// `DATA_MANAGE` predicate [`Self::repair_message`] enforces, so a caller
    /// sees exactly the reports for records they could have repaired
    /// themselves.
    #[oai(
        path = "/repair-reports",
        method = "get",
        operation_id = "list_repair_reports"
    )]
    async fn list_repair_reports(
        &self,
        context: WrappedContext,
        /// Maximum number of reports to return.
        limit: Query<Option<usize>>,
        /// Restrict the result to one account.
        account_id: Query<Option<u64>>,
    ) -> ApiResult<Json<Vec<BlobRepairRecord>>> {
        let limit = limit.0.unwrap_or(50).min(BlobRepairRecord::HISTORY_LIMIT);
        // Filter before truncating: taking the newest `limit` first and then
        // filtering would hand a caller who may repair few accounts an
        // arbitrarily short list — or an empty one.
        let reports = BlobRepairRecord::list_recent(usize::MAX)
            .into_iter()
            .filter(|r| account_id.0.map_or(true, |id| id == r.account_id))
            .filter(|r| {
                context.has_permission(Some(r.account_id), Permission::DATA_MANAGE)
            })
            .take(limit)
            .collect();
        Ok(Json(reports))
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
        // Direct permission, or a view whose filter covers the parent message.
        let (_, via_view) = bichon_core::account::access::load_readable_with_access(
            &context,
            account_id,
            &envelope_id,
        )?;
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
            via_view,
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
        // Direct permission, or a view whose filter covers the parent message.
        let (_, via_view) = bichon_core::account::access::load_readable_with_access(
            &context,
            account_id,
            &envelope_id,
        )?;
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
            via_view,
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
        // Direct permission, or a view whose filter covers the parent message.
        let (_, via_view) = bichon_core::account::access::load_readable_with_access(
            &context,
            account_id,
            &envelope_id,
        )?;
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
            via_view,
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
        // Views are expanded into their sources, filters included, and the
        // counts are taken over the same query a search runs — a view owns no
        // documents of its own, so aggregating by account id alone would always
        // return an empty list to a viewer. See
        // `IndexManager::get_all_tags_scoped`.
        let scope = bichon_core::account::access::search_scope(&context, None)?;
        Ok(Json(ENVELOPE_MANAGER.get_all_tags_scoped(scope.as_deref())?))
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

    /// Retrieves the contact addresses visible to the caller, each with the
    /// sender display name the archive recorded for it (when known).
    #[oai(
        path = "/all-contacts",
        method = "get",
        operation_id = "get_all_contacts"
    )]
    async fn get_all_contacts(&self, context: WrappedContext) -> ApiResult<Json<Vec<Contact>>> {
        // Views are expanded, and a filtered source is sampled rather than
        // aggregated — see `contact_suggestions_in_scope`, which shares its
        // scope logic with `contacts_in_scope` (the MCP tool's address-only
        // view), so the two cannot answer differently.
        Ok(Json(
            bichon_core::message::search::contact_suggestions_in_scope(&context)?,
        ))
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

/// A batch deletion needs `data:delete` on every account it would touch.
///
/// When the scope is unrestricted the caller must hold the global permission;
/// a view expands into its *source* accounts, and deleting through a view is
/// not a lesser act than deleting through the source.
/// Job history is visible to its creator and to administrators. Other users
/// with delete rights see only the jobs they started.
fn require_job_visible(
    context: &ClientContext,
    job: &BatchDeleteJob,
) -> bichon_core::error::BichonResult<()> {
    if job.user_id == context.user.id || context.user.is_admin() {
        Ok(())
    } else {
        Err(raise_error!(
            format!("Batch deletion job '{}' is not visible to this user.", job.job_id),
            ErrorCode::Forbidden
        ))
    }
}
