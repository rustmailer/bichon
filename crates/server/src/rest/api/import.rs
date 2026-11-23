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

use std::time::{SystemTime, UNIX_EPOCH};

use bichon_core::{
    account::migration::AccountModel,
    database::{manager::DB_MANAGER, MemDbModel},
    error::code::ErrorCode,
    ext::event_bus::{emit, Event},
    import::{
        check_temp_disk_space, detect_text_file, get_import_progress,
        history::{save_import_history, MAX_HISTORY_PER_USER},
        process_uploaded_file, update_progress, BatchEmlRequest, BatchEmlResult, FileFormat,
        ImportEmls, ImportHistory, ImportProgress, ImportStatus, MAX_WEB_EML_BYTES,
    },
    raise_error,
    settings::{dir::DATA_DIR_MANAGER, runtime},
    users::permissions::Permission,
};
use futures::StreamExt;
use poem::Body;
use poem_openapi::{
    param::{Path, Query},
    payload::{Binary, Json},
    OpenApi,
};
use tokio::io::AsyncWriteExt;

use crate::{
    common::auth::WrappedContext,
    rest::{api::ApiTags, ApiResult},
};

pub struct ImportApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Import")]
impl ImportApi {
    /// Batch import one or more EML files into a specified account and mail
    /// folder.
    ///
    /// This endpoint accepts a JSON payload containing:
    /// - `account_id`: the target account to import emails into
    /// - `mail_folder`: the mailbox/folder name
    /// - `emls`: a list of base64-encoded .eml files
    ///
    /// Returns a summary of the import result, including total processed,
    /// successful, and failed emails.
    #[oai(path = "/import", method = "post", operation_id = "do_batch_import")]
    async fn do_batch_import(
        &self,
        /// JSON payload with account info and EML files to import
        payload: Json<BatchEmlRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<BatchEmlResult>> {
        let account_id = payload.0.account_id;
        let folder = payload.0.mail_folder.clone();
        context.require_permission(Some(account_id), Permission::DATA_IMPORT_BATCH)?;
        let result = ImportEmls::do_import(payload.0).await?;
        emit(Event::ImportPerformed {
            user: context.user.username.clone(),
            account_id,
            format: "eml".to_string(),
            total: result.total as u64,
            success: result.success as u64,
            duplicates: result.duplicates as u64,
            failed: result.failed as u64,
        });

        // Save import history
        let progress = ImportProgress {
            import_id: format!(
                "batch_{:x}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            status: if result.failed == 0 {
                ImportStatus::Completed
            } else if result.success == 0 && result.duplicates == 0 {
                ImportStatus::Failed
            } else {
                ImportStatus::Completed
            },
            format: "eml".to_string(),
            total: result.total,
            success: result.success,
            duplicates: result.duplicates,
            failed: result.failed,
            failed_details: result.failed_details.clone(),
        };
        save_import_history(context.user.id, account_id, &folder, &progress);

        Ok(Json(result))
    }

    /// Upload an EML or MBOX file for import into a NoSync account.
    ///
    /// The file is sent as the raw request body. Both `account_id` and
    /// `mail_folder` must be provided as query parameters, along with the
    /// original `file_name` for extension validation.
    ///
    /// Returns an `import_id` to poll for progress via
    /// `/import-progress/:import_id`.
    #[oai(
        path = "/upload-import",
        method = "post",
        operation_id = "upload_import"
    )]
    async fn upload_import(
        &self,
        /// Target account ID (must be NoSync type).
        account_id: Query<u64>,
        /// Target mail folder name.
        mail_folder: Query<String>,
        /// Original file name, used for extension validation (e.g.
        /// "export.eml").
        file_name: Query<String>,
        /// The raw file bytes (.eml or .mbox).
        data: Binary<Body>,
        context: WrappedContext,
    ) -> ApiResult<Json<ImportProgress>> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::DATA_IMPORT_BATCH)?;

        // Basic account validation (fails fast)
        AccountModel::check_account_exists(account_id)?;

        let folder = mail_folder.0.trim().to_string();
        if folder.is_empty() {
            return Err(raise_error!(
                "mail_folder is required.".into(),
                ErrorCode::InvalidParameter
            ))?;
        }

        let file_name = file_name.0.trim().to_string();
        if file_name.is_empty() {
            return Err(raise_error!(
                "file_name is required.".into(),
                ErrorCode::InvalidParameter
            ))?;
        }

        // Validate file extension
        let ext_lower = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let is_mbox_ext = ext_lower == "mbox";
        let is_eml_ext = ext_lower == "eml";
        let is_pst_ext = ext_lower == "pst";
        if !is_mbox_ext && !is_eml_ext && !is_pst_ext {
            return Err(raise_error!(
                format!(
                    "Unsupported file type '.{}'. Only .eml, .mbox and .pst files are allowed.",
                    ext_lower
                ),
                ErrorCode::InvalidParameter
            ))?;
        }

        // Check disk space (fail fast before streaming)
        let max_mbox = runtime::web_mbox_upload_limit_mb() as usize * 1024 * 1024;
        let max_pst = runtime::web_pst_upload_limit_mb() as usize * 1024 * 1024;
        let min_required = if is_mbox_ext {
            max_mbox
        } else if is_pst_ext {
            max_pst
        } else {
            MAX_WEB_EML_BYTES
        };
        let free = check_temp_disk_space()?;
        if free < min_required as u64 * 2 {
            let free_gb = free as f64 / 1024.0 / 1024.0 / 1024.0;
            let need_gb = (min_required as f64 * 2.0) / 1024.0 / 1024.0 / 1024.0;
            return Err(raise_error!(
                format!(
                    "Insufficient disk space. Free: {:.1} GB. Need at least {:.1} GB.",
                    free_gb, need_gb
                ),
                ErrorCode::InvalidParameter
            ))?;
        }

        // Stream body to temp file, enforcing size limits and validating content
        let import_id = format!(
            "imp_{:x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let temp_path = DATA_DIR_MANAGER
            .temp_dir
            .join(format!("import_{}.tmp", import_id));

        let (format_detected, file_len) =
            stream_body_to_temp(data.0, &temp_path, is_mbox_ext, is_pst_ext).await?;

        let format = format_detected.unwrap_or_else(|| {
            if is_mbox_ext {
                FileFormat::Mbox
            } else if is_pst_ext {
                FileFormat::Pst
            } else {
                FileFormat::Eml
            }
        });

        let format_str = match format {
            FileFormat::Mbox => "mbox".to_string(),
            FileFormat::Eml => "eml".to_string(),
            FileFormat::Pst => "pst".to_string(),
        };

        let max_mbox = runtime::web_mbox_upload_limit_mb() as usize * 1024 * 1024;
        let max_pst = runtime::web_pst_upload_limit_mb() as usize * 1024 * 1024;
        let max_size = match format {
            FileFormat::Mbox => max_mbox,
            FileFormat::Pst => max_pst,
            FileFormat::Eml => MAX_WEB_EML_BYTES,
        };
        if file_len > max_size {
            let _ = std::fs::remove_file(&temp_path);
            let max_mb = max_size as f64 / 1024.0 / 1024.0;
            let actual_mb = file_len as f64 / 1024.0 / 1024.0;
            return Err(raise_error!(
                format!(
                    "File too large ({:.1} MB). Maximum for {} is {:.0} MB. Use the CLI for larger files.",
                    actual_mb, format_str.to_uppercase(), max_mb
                ),
                ErrorCode::InvalidParameter
            ))?;
        }

        // Record initial progress
        let initial = ImportProgress {
            import_id: import_id.clone(),
            status: ImportStatus::Pending,
            format: format_str.clone(),
            total: 0,
            success: 0,
            duplicates: 0,
            failed: 0,
            failed_details: vec![],
        };

        // Store initial progress so polling can find it immediately
        update_progress(&import_id, initial.clone());

        // Spawn background processing
        let id = import_id.clone();
        let folder_clone = folder.clone();
        let user_id = context.user.id;
        emit(Event::ImportPerformed {
            user: context.user.username.clone(),
            account_id,
            format: format_str.clone(),
            total: 0,
            success: 0,
            duplicates: 0,
            failed: 0,
        });
        tokio::task::spawn_blocking(move || {
            process_uploaded_file(
                &id,
                &temp_path,
                &file_name,
                account_id,
                &folder_clone,
                user_id,
            );
        });

        Ok(Json(initial))
    }

    /// Poll import progress by import ID.
    #[oai(
        path = "/import-progress/:import_id",
        method = "get",
        operation_id = "get_import_progress"
    )]
    async fn get_import_progress(
        &self,
        import_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<ImportProgress>> {
        let _ = context; // progress queries don't need per-account auth
        match get_import_progress(&import_id.0) {
            Some(progress) => Ok(Json(progress)),
            None => Err(raise_error!(
                format!("Import {} not found.", import_id.0),
                ErrorCode::ResourceNotFound
            ))?,
        }
    }

    /// Check available disk space on the server's temp directory.
    #[oai(
        path = "/check-disk-space",
        method = "get",
        operation_id = "check_disk_space"
    )]
    async fn check_disk_space(&self, _context: WrappedContext) -> ApiResult<Json<u64>> {
        let free = check_temp_disk_space()?;
        Ok(Json(free))
    }

    /// List import history for the current user (latest first, up to 5
    /// entries).
    #[oai(
        path = "/import-history",
        method = "get",
        operation_id = "list_import_history"
    )]
    async fn list_import_history(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<ImportHistory>>> {
        let prefix = format!("{}:", context.user.id);
        let coll = DB_MANAGER.db().collection(ImportHistory::collection());
        let mut entries: Vec<ImportHistory> = coll
            .scan_prefix(&prefix)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        // Sort by created_at descending (newest first), keep at most N per user
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        entries.truncate(MAX_HISTORY_PER_USER);
        Ok(Json(entries))
    }
}

/// Stream a poem `Body` to a temp file while enforcing size limits and
/// validating that the content looks like a text-based email file.
///
/// PST files are binary (OLE2) — text detection is skipped for them.
///
/// Returns the detected format (if any) and the total bytes written.
async fn stream_body_to_temp(
    body: Body,
    temp_path: &std::path::Path,
    is_mbox_ext: bool,
    is_pst_ext: bool,
) -> ApiResult<(Option<FileFormat>, usize)> {
    let max_mbox = runtime::web_mbox_upload_limit_mb() as usize * 1024 * 1024;
    let max_pst = runtime::web_pst_upload_limit_mb() as usize * 1024 * 1024;
    let max_stream = if is_mbox_ext {
        max_mbox
    } else if is_pst_ext {
        max_pst
    } else {
        MAX_WEB_EML_BYTES
    };

    let mut file = tokio::fs::File::create(temp_path).await.map_err(|e| {
        raise_error!(
            format!("Failed to create temp file: {}", e),
            ErrorCode::InternalError
        )
    })?;

    let mut body_stream = body.into_bytes_stream();
    let mut total: usize = 0;
    let mut first_chunk: Vec<u8> = Vec::new();
    let mut format_detected: Option<FileFormat> = None;
    let mut text_checked = false;

    while let Some(chunk_result) = body_stream.next().await {
        let chunk = chunk_result.map_err(|e| {
            raise_error!(
                format!("Failed to read request body: {}", e),
                ErrorCode::InternalError
            )
        })?;

        total += chunk.len();

        // Enforce size limit during streaming
        if total > max_stream {
            // Clean up partial temp file
            drop(file);
            let _ = tokio::fs::remove_file(temp_path).await;
            let max_mb = max_stream as f64 / 1024.0 / 1024.0;
            return Err(raise_error!(
                format!(
                    "Upload exceeds maximum size of {:.0} MB. Use the CLI for larger files.",
                    max_mb
                ),
                ErrorCode::InvalidParameter
            ))?;
        }

        // Accumulate first ~8 KB for format & text detection
        if first_chunk.len() < 8192 {
            let remaining = 8192 - first_chunk.len();
            first_chunk.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }

        // Once we have enough data, validate format and text
        if first_chunk.len() >= 512 && !text_checked {
            text_checked = true;
            format_detected = bichon_core::import::detect_format(&first_chunk, "upload");

            // If extension is .eml but content looks like MBOX (or vice versa), that's OK.
            // PST files are binary — skip text detection.
            if !is_pst_ext && !detect_text_file(&first_chunk) {
                drop(file);
                let _ = tokio::fs::remove_file(temp_path).await;
                return Err(raise_error!(
                    "The uploaded file appears to be binary (not a valid email file). Only .eml, .mbox and .pst files are accepted.".into(),
                    ErrorCode::InvalidParameter
                ))?;
            }
        }

        file.write_all(&chunk).await.map_err(|e| {
            raise_error!(
                format!("Failed to write temp file: {}", e),
                ErrorCode::InternalError
            )
        })?;
    }

    file.flush().await.map_err(|e| {
        raise_error!(
            format!("Failed to flush temp file: {}", e),
            ErrorCode::InternalError
        )
    })?;

    // If file is empty, reject
    if total == 0 {
        let _ = tokio::fs::remove_file(temp_path).await;
        return Err(raise_error!(
            "Empty file is not allowed.".into(),
            ErrorCode::InvalidParameter
        ))?;
    }

    Ok((format_detected, total))
}
