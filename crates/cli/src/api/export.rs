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

use crate::BichonCliConfig;
use bichon_core::export::{
    ExportCreateRequest, ExportFormat, ExportJobView, ExportPreviewRequest, ExportPreviewView,
    ExportVerifyProgressView, ExportVerifyView,
};
use bichon_core::saved_search::{SavedSearchKind, SavedSearchModel};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// A failed API call. `status` is `Some` when the server answered and `None`
/// when the request never reached it (connection refused, DNS failure, ...).
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl ApiError {
    fn network(context: &str, error: impl std::fmt::Display) -> Self {
        Self {
            status: None,
            message: format!("{context}: {error}"),
        }
    }

    fn http(status: u16, body: String) -> Self {
        let body = body.trim();
        Self {
            status: Some(status),
            message: if body.is_empty() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status}: {body}")
            },
        }
    }

    fn decode(context: &str, error: impl std::fmt::Display) -> Self {
        Self {
            status: None,
            message: format!("{context}: {error}"),
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

/// The result of asking the server to start an export.
///
/// A policy-gated server (dual control) answers `202 Accepted` with a pending
/// approval instead of creating a job. Scripts must be able to tell that apart
/// from a started export: reading it as success would let a cron job believe
/// an artifact exists when a second person still has to approve it.
#[derive(Debug, Clone)]
pub enum CreateExportOutcome {
    Created(ExportJobView),
    PendingApproval {
        request_id: String,
        expires_at: Option<i64>,
    },
}

fn endpoint(config: &BichonCliConfig, path: &str) -> String {
    format!("{}{}", config.base_url.trim_end_matches('/'), path)
}

async fn read_body(response: reqwest::Response) -> Result<(u16, String), ApiError> {
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| ApiError::network("could not read the server response", e))?;
    Ok((status, body))
}

/// Lists the current user's saved searches of one kind (newest first).
pub async fn list_saved_searches_result(
    client: &Client,
    config: &BichonCliConfig,
    kind: &SavedSearchKind,
) -> Result<Vec<SavedSearchModel>, ApiError> {
    let kind = match kind {
        SavedSearchKind::Email => "Email",
        SavedSearchKind::Attachment => "Attachment",
    };
    let url = endpoint(config, &format!("/api/v1/saved-searches?kind={kind}"));
    let response = client
        .get(&url)
        .bearer_auth(&config.api_token)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
    let (status, body) = read_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(ApiError::http(status, body));
    }
    serde_json::from_str(&body)
        .map_err(|e| ApiError::decode("unexpected saved-searches response", e))
}

/// Lists the current user's saved searches of one kind (newest first).
pub async fn list_saved_searches(
    client: &Client,
    config: &BichonCliConfig,
    kind: &SavedSearchKind,
) -> Vec<SavedSearchModel> {
    match list_saved_searches_result(client, config, kind).await {
        Ok(searches) => searches,
        Err(e) => {
            eprintln!(" ✘ Failed to list saved searches. {}", e);
            Vec::new()
        }
    }
}

/// Previews what an export would contain.
pub async fn preview_export_request(
    client: &Client,
    config: &BichonCliConfig,
    request: &ExportPreviewRequest,
) -> Result<ExportPreviewView, ApiError> {
    let url = endpoint(config, "/api/v1/exports/preview");
    let response = client
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(request)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
    let (status, body) = read_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(ApiError::http(status, body));
    }
    serde_json::from_str(&body).map_err(|e| ApiError::decode("unexpected export preview response", e))
}

/// Previews what an export from a saved search would contain.
pub async fn preview_export(
    client: &Client,
    config: &BichonCliConfig,
    saved_search_id: &str,
    format: ExportFormat,
) -> Option<ExportPreviewView> {
    let request = ExportPreviewRequest {
        saved_search_id: Some(saved_search_id.to_string()),
        filter: None,
        label: None,
        format: Some(format),
    };
    match preview_export_request(client, config, &request).await {
        Ok(view) => Some(view),
        Err(e) => {
            eprintln!(" ✘ Failed to preview export. {}", e);
            None
        }
    }
}

/// Starts a background export job.
pub async fn create_export_request(
    client: &Client,
    config: &BichonCliConfig,
    request: &ExportCreateRequest,
) -> Result<CreateExportOutcome, ApiError> {
    let url = endpoint(config, "/api/v1/exports");
    let response = client
        .post(&url)
        .bearer_auth(&config.api_token)
        .json(request)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
    let (status, body) = read_body(response).await?;

    if status == 202 {
        // Dual control: the request is queued, not exported. The body follows
        // the `ok: false, pending: true` contract.
        #[derive(serde::Deserialize)]
        struct Pending {
            request_id: String,
            #[serde(default)]
            expires_at: Option<i64>,
        }
        let pending: Pending = serde_json::from_str(&body)
            .map_err(|e| ApiError::decode("unexpected approval response", e))?;
        return Ok(CreateExportOutcome::PendingApproval {
            request_id: pending.request_id,
            expires_at: pending.expires_at,
        });
    }

    if !(200..300).contains(&status) {
        return Err(ApiError::http(status, body));
    }
    let job = serde_json::from_str(&body)
        .map_err(|e| ApiError::decode("unexpected export response", e))?;
    Ok(CreateExportOutcome::Created(job))
}

/// Starts a background export job from a saved search.
pub async fn create_export_job(
    client: &Client,
    config: &BichonCliConfig,
    saved_search_id: &str,
    format: ExportFormat,
) -> Option<ExportJobView> {
    let request = ExportCreateRequest {
        saved_search_id: Some(saved_search_id.to_string()),
        filter: None,
        label: None,
        format: Some(format),
    };
    match create_export_request(client, config, &request).await {
        Ok(CreateExportOutcome::Created(job)) => Some(job),
        Ok(CreateExportOutcome::PendingApproval { request_id, .. }) => {
            eprintln!(
                " ✘ This server requires approval for batch exports. \
                 The request was queued as '{}' and must be approved in the web UI.",
                request_id
            );
            None
        }
        Err(e) => {
            eprintln!(" ✘ Failed to start export. {}", e);
            None
        }
    }
}

/// Snapshot of an export job.
pub async fn get_export_job_result(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Result<ExportJobView, ApiError> {
    let url = endpoint(config, &format!("/api/v1/exports/{job_id}"));
    let response = client
        .get(&url)
        .bearer_auth(&config.api_token)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
    let (status, body) = read_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(ApiError::http(status, body));
    }
    serde_json::from_str(&body).map_err(|e| ApiError::decode("unexpected export status response", e))
}

/// Snapshot of an export job.
pub async fn get_export_job(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Option<ExportJobView> {
    match get_export_job_result(client, config, job_id).await {
        Ok(job) => Some(job),
        Err(e) => {
            eprintln!(" ✘ Failed to fetch export status. {}", e);
            None
        }
    }
}

fn part_path(target: &Path) -> PathBuf {
    PathBuf::from(format!("{}.part", target.display()))
}

/// Move the finished `.part` file over `target`, replacing an existing file.
///
/// `rename` replaces the destination on Unix but fails on Windows when the
/// destination exists, so the destination is removed first on that path.
async fn replace_file(part: &Path, target: &Path) -> Result<(), ApiError> {
    if tokio::fs::rename(part, target).await.is_ok() {
        return Ok(());
    }
    if target.exists() {
        if let Err(e) = tokio::fs::remove_file(target).await {
            return Err(ApiError::decode(
                &format!("could not replace '{}'", target.display()),
                e,
            ));
        }
        tokio::fs::rename(part, target)
            .await
            .map_err(|e| ApiError::decode(&format!("could not write '{}'", target.display()), e))
    } else {
        Err(ApiError::decode(
            &format!("could not write '{}'", target.display()),
            std::io::Error::last_os_error(),
        ))
    }
}

/// Downloads the finished artifact to `target`.
///
/// The bytes are written to a sibling `.part` file and renamed into place, so
/// a failed or interrupted download never leaves a truncated artifact under
/// the final name — cron jobs would otherwise have to guess whether the file
/// they find is complete.
pub async fn download_export_to_file_result(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
    target: &Path,
) -> Result<(), ApiError> {
    let url = endpoint(config, &format!("/api/v1/exports/{job_id}/download"));
    let response = client
        .get(&url)
        .bearer_auth(&config.api_token)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::http(status, body));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ApiError::network("could not read the export body", e))?;

    let part = part_path(target);
    if let Some(parent) = part.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ApiError::decode(&format!("could not create '{}'", parent.display()), e)
            })?;
        }
    }

    if let Err(e) = write_part(&part, &bytes).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    let replaced = replace_file(&part, target).await;
    if replaced.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    replaced
}

async fn write_part(part: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| ApiError::decode(&format!("could not create '{}'", part.display()), e))?;
    file.write_all(bytes)
        .await
        .map_err(|e| ApiError::decode(&format!("could not write '{}'", part.display()), e))?;
    file.flush()
        .await
        .map_err(|e| ApiError::decode(&format!("could not flush '{}'", part.display()), e))
}

/// Downloads the finished artifact to `target`.
pub async fn download_export_to_file(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
    target: &Path,
) -> bool {
    match download_export_to_file_result(client, config, job_id, target).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!(" ✘ Failed to download export. {}", e);
            false
        }
    }
}

/// Runs (and waits for) the server-side compliance verification of a finished
/// export. The verification runs in the background, so this polls its
/// progress endpoint until it finishes.
pub async fn verify_export_job_result(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Result<ExportVerifyView, ApiError> {
    let url = endpoint(config, &format!("/api/v1/exports/{job_id}/verify"));
    let response = client
        .post(&url)
        .bearer_auth(&config.api_token)
        .send()
        .await
        .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
    let (status, body) = read_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(ApiError::http(status, body));
    }

    let mut polled: u32 = 0;
    loop {
        let response = client
            .get(&url)
            .bearer_auth(&config.api_token)
            .send()
            .await
            .map_err(|e| ApiError::network("could not reach the Bichon server", e))?;
        let (status, body) = read_body(response).await?;
        if !(200..300).contains(&status) {
            return Err(ApiError::http(status, body));
        }
        let progress: ExportVerifyProgressView = serde_json::from_str(&body)
            .map_err(|e| ApiError::decode("unexpected verification response", e))?;
        match progress.status.as_str() {
            "finished" => {
                return progress
                    .result
                    .ok_or_else(|| ApiError::other("verification finished without a result"))
            }
            "failed" => {
                return Err(ApiError::other(format!(
                    "verification failed: {}",
                    progress.error.unwrap_or_else(|| "unknown error".into())
                )))
            }
            _ => {}
        }
        polled += 1;
        if polled > 7200 {
            return Err(ApiError::other("verification timed out after 60 minutes"));
        }
        if polled % 10 == 0 {
            eprintln!(
                "   Verifying... {} / {} messages checked ({} matched, {} mismatched)",
                progress.checked, progress.total, progress.matched, progress.mismatched
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Runs the compliance verification and waits for the result.
pub async fn verify_export_job(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Option<ExportVerifyView> {
    match verify_export_job_result(client, config, job_id).await {
        Ok(view) => Some(view),
        Err(e) => {
            eprintln!("✘ {}", e);
            None
        }
    }
}
