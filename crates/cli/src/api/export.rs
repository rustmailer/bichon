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
    ExportFormat, ExportJobView, ExportPreviewView, ExportVerifyProgressView, ExportVerifyView,
};
use bichon_core::saved_search::{SavedSearchKind, SavedSearchModel};
use reqwest::Client;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// Lists the current user's saved searches of one kind (newest first).
pub async fn list_saved_searches(
    client: &Client,
    config: &BichonCliConfig,
    kind: &SavedSearchKind,
) -> Vec<SavedSearchModel> {
    let kind = match kind {
        SavedSearchKind::Email => "Email",
        SavedSearchKind::Attachment => "Attachment",
    };
    let url = format!("{}/api/v1/saved-searches?kind={kind}", config.base_url);
    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res.json::<Vec<SavedSearchModel>>().await.unwrap_or_default(),
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                " ✘ Failed to list saved searches. Status: {}\n  Server error: {}",
                status, body
            );
            Vec::new()
        }
        Err(e) => {
            eprintln!(" ✘ Network error listing saved searches: {}", e);
            Vec::new()
        }
    }
}

/// Previews what an export from a saved search would contain.
pub async fn preview_export(
    client: &Client,
    config: &BichonCliConfig,
    saved_search_id: &str,
    format: ExportFormat,
) -> Option<ExportPreviewView> {
    let url = format!("{}/api/v1/exports/preview", config.base_url);
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&serde_json::json!({
            "saved_search_id": saved_search_id,
            "format": format,
        }))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res.json::<ExportPreviewView>().await.ok(),
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                " ✘ Failed to preview export. Status: {}\n  Server error: {}",
                status, body
            );
            None
        }
        Err(e) => {
            eprintln!(" ✘ Network error previewing export: {}", e);
            None
        }
    }
}

/// Starts a background export job from a saved search.
pub async fn create_export_job(
    client: &Client,
    config: &BichonCliConfig,
    saved_search_id: &str,
    format: ExportFormat,
) -> Option<ExportJobView> {
    let url = format!("{}/api/v1/exports", config.base_url);
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&serde_json::json!({
            "saved_search_id": saved_search_id,
            "format": format,
        }))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res.json::<ExportJobView>().await.ok(),
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                " ✘ Failed to start export. Status: {}\n  Server error: {}",
                status, body
            );
            None
        }
        Err(e) => {
            eprintln!(" ✘ Network error starting export: {}", e);
            None
        }
    }
}

/// Snapshot of an export job.
pub async fn get_export_job(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Option<ExportJobView> {
    let url = format!("{}/api/v1/exports/{}", config.base_url, job_id);
    match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res.json::<ExportJobView>().await.ok(),
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                " ✘ Failed to fetch export status. Status: {}\n  Server error: {}",
                status, body
            );
            None
        }
        Err(e) => {
            eprintln!(" ✘ Network error fetching export status: {}", e);
            None
        }
    }
}

/// Downloads the finished mbox artifact to `target`.
pub async fn download_export_to_file(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
    target: &Path,
) -> bool {
    let url = format!("{}/api/v1/exports/{}/download", config.base_url, job_id);
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res,
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                " ✘ Failed to download export. Status: {}\n  Server error: {}",
                status, body
            );
            return false;
        }
        Err(e) => {
            eprintln!(" ✘ Network error downloading export: {}", e);
            return false;
        }
    };

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!(" ✘ Failed to read export body: {}", e);
            return false;
        }
    };
    match tokio::fs::File::create(target).await {
        Ok(mut file) => {
            if file.write_all(&bytes).await.is_err() {
                eprintln!(" ✘ Failed to write '{}'", target.display());
                return false;
            }
            true
        }
        Err(e) => {
            eprintln!(" ✘ Failed to create '{}': {}", target.display(), e);
            false
        }
    }
}

/// The verification runs in the background on the server, so this starts it
/// and then polls the progress endpoint until it finishes.
pub async fn verify_export_job(
    client: &Client,
    config: &BichonCliConfig,
    job_id: &str,
) -> Option<ExportVerifyView> {
    let url = format!("{}/api/v1/exports/{}/verify", config.base_url, job_id);
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {}
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                "✘ Failed to start verification. Status: {}\n  Server error: {}",
                status, body
            );
            return None;
        }
        Err(e) => {
            eprintln!("✘ Network error verifying export: {}", e);
            return None;
        }
    }

    let mut polled: u32 = 0;
    loop {
        let progress: Option<ExportVerifyProgressView> = match client
            .get(&url)
            .header("Authorization", format!("Bearer {}", config.api_token))
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                res.json::<ExportVerifyProgressView>().await.ok()
            }
            Ok(_) => None,
            Err(e) => {
                eprintln!("✘ Network error polling verification: {}", e);
                return None;
            }
        };
        let Some(progress) = progress else {
            eprintln!("✘ Failed to read verification progress.");
            return None;
        };
        match progress.status.as_str() {
            "finished" => return progress.result,
            "failed" => {
                eprintln!(
                    "✘ Verification failed: {}",
                    progress.error.as_deref().unwrap_or("unknown error")
                );
                return None;
            }
            "running" => {
                if polled % 10 == 0 {
                    eprintln!(
                        "   Verifying... {} / {} messages checked ({} matched, {} mismatched)",
                        progress.checked, progress.total, progress.matched, progress.mismatched
                    );
                }
            }
            _ => {}
        }
        polled += 1;
        if polled > 7200 {
            eprintln!("✘ Verification timed out after 60 minutes.");
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
