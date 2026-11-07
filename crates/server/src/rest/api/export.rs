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
use bichon_core::common::auth::ClientContext;
use crate::export::{
    cancel_export, create_download_ticket, create_export, delete_export, download_artifact,
    get_export, list_exports, preview, resolve_download_ticket,
};
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;
use bichon_core::error::code::ErrorCode;
use bichon_core::export::{
    ExportCreateRequest, ExportJobView, ExportPreviewRequest, ExportPreviewView,
};
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::raise_error;
use poem::{handler, Body, IntoResponse, Response};
use poem_openapi::param::Path;
use poem_openapi::payload::{Attachment, AttachmentType, Json};
use poem_openapi::OpenApi;
use std::collections::HashSet;

pub struct ExportApi;

/// Response carrying a short-lived, one-time download URL.
#[derive(serde::Serialize, poem_openapi::Object)]
struct DownloadTicketView {
    url: String,
}

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Export")]
impl ExportApi {
    /// Previews what an export from a saved search would contain (accounts,
    /// email count, estimated size), restricted to the caller's RBAC scope.
    #[oai(
        path = "/exports/preview",
        method = "post",
        operation_id = "preview_export"
    )]
    async fn preview_export(
        &self,
        context: WrappedContext,
        payload: Json<ExportPreviewRequest>,
    ) -> ApiResult<Json<ExportPreviewView>> {
        let scope = export_scope(&context)?;
        let view = preview(
            context.user.id,
            &payload.0.source(),
            payload.0.format.unwrap_or_default(),
            scope,
        )?;
        Ok(Json(view))
    }

    /// Starts a background batch export from a saved search.
    #[oai(
        path = "/exports",
        method = "post",
        operation_id = "create_export"
    )]
    async fn create_export_handler(
        &self,
        context: WrappedContext,
        payload: Json<ExportCreateRequest>,
    ) -> ApiResult<Json<ExportJobView>> {
        let scope = export_scope(&context)?;
        let job = create_export(
            context.user.id,
            context.user.username.clone(),
            &payload.0.source(),
            payload.0.format.unwrap_or_default(),
            scope,
        )?;
        Ok(Json(job))
    }

    /// Lists the caller's export jobs, newest first.
    #[oai(
        path = "/exports",
        method = "get",
        operation_id = "list_exports"
    )]
    async fn list_exports_handler(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<ExportJobView>>> {
        Ok(Json(list_exports(context.user.id)))
    }

    /// Snapshot of an export job.
    #[oai(
        path = "/exports/:job_id",
        method = "get",
        operation_id = "get_export"
    )]
    async fn get_export_handler(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<ExportJobView>> {
        Ok(Json(get_export(context.user.id, &job_id.0)?))
    }

    /// Requests cancellation of a running export job.
    #[oai(
        path = "/exports/:job_id/cancel",
        method = "post",
        operation_id = "cancel_export"
    )]
    async fn cancel_export_handler(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<ExportJobView>> {
        Ok(Json(cancel_export(context.user.id, &job_id.0)?))
    }

    /// Deletes an export job and its on-disk artifact.
    #[oai(
        path = "/exports/:job_id",
        method = "delete",
        operation_id = "delete_export"
    )]
    async fn delete_export_handler(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        Ok(delete_export(context.user.id, &job_id.0)?)
    }

    /// Downloads the finished export artifact (mbox for whole-message
    /// exports, zip for attachment exports).
    #[oai(
        path = "/exports/:job_id/download",
        method = "get",
        operation_id = "download_export"
    )]
    async fn download_export_handler(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Attachment<Body>> {
        let job_id = job_id.0;
        let (path, name) = download_artifact(context.user.id, &job_id)?;
        let view = get_export(context.user.id, &job_id)?;
        emit(Event::ExportDownloaded {
            user: context.user.username.clone(),
            export_id: job_id.clone(),
            email_count: view.exported,
            artifact_size: view.artifact_size,
        });
        let reader = tokio::fs::File::open(&path).await.map_err(|e| {
            raise_error!(
                format!("Failed to open export artifact: {e}"),
                ErrorCode::InternalError
            )
        })?;
        let body = Body::from_async_read(reader);
        let attachment = Attachment::new(body)
            .attachment_type(AttachmentType::Attachment)
            .filename(name);
        Ok(attachment)
    }

    /// Returns a short-lived, one-time URL that streams the finished mbox
    /// artifact directly to the browser (no in-memory buffering on the client).
    #[oai(
        path = "/exports/:job_id/download-ticket",
        method = "post",
        operation_id = "create_download_ticket"
    )]
    async fn create_download_ticket_handler(
        &self,
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<DownloadTicketView>> {
        let ticket = create_download_ticket(context.user.id, &context.user.username, &job_id.0)?;
        Ok(Json(DownloadTicketView {
            url: format!("api/v1/exports/download/{ticket}"),
        }))
    }
}

/// Streams the artifact referenced by a one-time download ticket. This route
/// is mounted outside `ApiGuard` because a top-level navigation cannot carry
/// the Bearer header; the ticket itself is single-use and short-lived.
#[handler]
pub async fn download_export_ticket_handler(
    ticket: poem::web::Path<String>,
) -> poem::Result<Response> {
    let (user_id, username, job_id) = resolve_download_ticket(&ticket.0).ok_or_else(|| {
        poem::Error::from_response(
            Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .content_type("application/json")
                .body(r#"{"message":"Download ticket is invalid or expired."}"#)
                .into_response(),
        )
    })?;
    let (path, name) = download_artifact(user_id, &job_id).map_err(|e| {
        poem::Error::from_response(
            Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .content_type("application/json")
                .body(format!(r#"{{"message":"{}"}}"#, e))
                .into_response(),
        )
    })?;
    if let Ok(view) = get_export(user_id, &job_id) {
        emit(Event::ExportDownloaded {
            user: username,
            export_id: job_id,
            email_count: view.exported,
            artifact_size: view.artifact_size,
        });
    }
    let reader = tokio::fs::File::open(&path).await.map_err(|_| {
        poem::Error::from_response(
            Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .content_type("application/json")
                .body(r#"{"message":"Failed to open export artifact"}"#)
                .into_response(),
        )
    })?;
    let body = Body::from_async_read(reader);
    let attachment = Attachment::new(body)
        .attachment_type(AttachmentType::Attachment)
        .filename(name);
    Ok(attachment.into_response())
}

/// Computes the account scope the caller may export: all accounts when the
/// caller holds the global `data:export:batch` permission (or is an admin),
/// otherwise the subset of their accessible accounts whose role grants
/// `data:export:batch`. Returns `Forbidden` when nothing is exportable.
///
/// The resolution lives in `bichon_core::account::access` beside the search
/// scope, so "what may this caller read" has one implementation and an export
/// cannot disagree with a search about it — including the view handling: a
/// view contributes its **unfiltered** sources, and a caller whose only grant
/// is a filtered view is refused with that reason (the export engine and the
/// approval record both carry account ids, which cannot express the filter).
///
/// Public, and over `ClientContext` rather than the `WrappedContext` extractor,
/// because the Pro dual-control gate has to resolve the same scope *before* the
/// handler runs — it queues the request instead of letting it through, so this
/// is the only copy of the check that executes on the gated path. Duplicating
/// it there would be a second security decision to keep in step with this one.
pub fn export_scope(
    context: &ClientContext,
) -> bichon_core::error::BichonResult<Option<HashSet<u64>>> {
    bichon_core::account::access::export_scope_accounts(context)
}
