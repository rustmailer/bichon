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

//! Flag-driven export for scripts and cron.
//!
//! The interactive wizard in [`super`] asks the same questions through
//! `dialoguer`; this module answers them from the command line instead. It
//! never prompts, writes human-readable progress to stderr, and leaves stdout
//! with a single machine-readable line: the artifact path, or the job id when
//! `--no-wait` is used.
//!
//! Exit codes are part of the contract:
//!
//! - `0` — the artifact is on disk (or the job id was printed)
//! - `1` — the export failed
//! - `2` — the invocation is invalid
//! - `3` — the server queued the request for approval and exported nothing

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bichon_core::export::{
    ExportCreateRequest, ExportFilter, ExportFormat, ExportJobView, ExportPreviewRequest,
};
use bichon_core::message::search::EmailSearchFilter;
use bichon_core::saved_search::{SavedSearchKind, SavedSearchModel};
use clap::{Args, ValueEnum};
use reqwest::Client;

use crate::api::export::{
    create_export_request, download_export_to_file_result, get_export_job_result,
    list_saved_searches_result, preview_export_request, verify_export_job_result, ApiError,
    CreateExportOutcome,
};
use crate::BichonCliConfig;

/// Exit codes shared by `main` and the tests.
pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_PENDING_APPROVAL: u8 = 3;

/// Files this module writes are named with this prefix. `--keep` only ever
/// deletes files matching it, so pointing an export at a populated directory
/// can never remove unrelated data.
const EXPORT_PREFIX: &str = "bichon-export-";

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    /// Whole messages, concatenated into an mbox file.
    Mbox,
    /// Attachments matched by an attachment saved search, packed into a zip.
    Attachments,
}

impl From<FormatArg> for ExportFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Mbox => ExportFormat::Mbox,
            FormatArg::Attachments => ExportFormat::Attachments,
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct ExportArgs {
    /// Saved search to export: its id, or its exact name
    #[arg(long, value_name = "ID|NAME", conflicts_with = "account_id")]
    pub saved_search: Option<String>,

    /// Export every message of these accounts (repeatable)
    #[arg(
        long = "account-id",
        value_name = "ID",
        action = clap::ArgAction::Append,
        conflicts_with = "saved_search"
    )]
    pub account_id: Vec<u64>,

    /// Artifact format
    #[arg(long, value_enum, default_value_t = FormatArg::Mbox)]
    pub format: FormatArg,

    /// Exact path of the artifact to write
    #[arg(long, value_name = "FILE", conflicts_with = "output_dir")]
    pub output: Option<PathBuf>,

    /// Directory to write a timestamped artifact into (created if missing)
    #[arg(long, value_name = "DIR", conflicts_with = "output")]
    pub output_dir: Option<PathBuf>,

    /// Return as soon as the job is created instead of waiting for the file
    #[arg(long)]
    pub no_wait: bool,

    /// Give up waiting for the job after this many seconds
    #[arg(long, value_name = "SECONDS", default_value_t = 86_400)]
    pub timeout_secs: u64,

    /// Run compliance verification after downloading (Bichon Pro server only)
    #[arg(long)]
    pub verify: bool,

    /// Keep only the N newest bichon-export-* files in --output-dir
    #[arg(long, value_name = "N", requires = "output_dir")]
    pub keep: Option<usize>,
}

/// Validated options, separated from `clap` so the flow can be exercised
/// directly by tests with a short poll interval.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub saved_search: Option<String>,
    pub account_ids: Vec<u64>,
    pub format: ExportFormat,
    pub output: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub wait: bool,
    pub timeout: Duration,
    pub verify: bool,
    pub keep: Option<usize>,
    pub poll_interval: Duration,
}

impl ExportOptions {
    pub fn from_args(args: &ExportArgs) -> Result<Self, ExportError> {
        let mut account_ids = args.account_id.clone();
        account_ids.sort_unstable();
        account_ids.dedup();

        let has_search = args.saved_search.is_some();
        let has_accounts = !account_ids.is_empty();
        if has_search == has_accounts {
            return Err(ExportError::usage(
                "choose exactly one source: --saved-search <id|name> or --account-id <id>",
            ));
        }

        let format: ExportFormat = args.format.into();
        if format == ExportFormat::Attachments && has_accounts {
            return Err(ExportError::usage(
                "--format attachments needs --saved-search: an attachment export is \
                 defined by an attachment saved search",
            ));
        }

        if args.output.is_some() == args.output_dir.is_some() {
            return Err(ExportError::usage(
                "choose exactly one output: --output <file> or --output-dir <dir>",
            ));
        }

        if args.timeout_secs == 0 {
            return Err(ExportError::usage("--timeout-secs must be greater than 0"));
        }

        if args.keep.is_some() && args.output_dir.is_none() {
            return Err(ExportError::usage(
                "--keep needs --output-dir: it prunes older bichon-export-* files \
                 written by previous runs",
            ));
        }

        Ok(Self {
            saved_search: args.saved_search.clone(),
            account_ids,
            format,
            output: args.output.clone(),
            output_dir: args.output_dir.clone(),
            wait: !args.no_wait,
            timeout: Duration::from_secs(args.timeout_secs),
            verify: args.verify,
            keep: args.keep,
            poll_interval: Duration::from_secs(2),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExportError {
    pub usage: bool,
    pub message: String,
}

impl ExportError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            usage: true,
            message: message.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self {
            usage: false,
            message: message.into(),
        }
    }

    fn api(error: ApiError) -> Self {
        Self::other(error.to_string())
    }
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ExportError {}

#[derive(Debug)]
pub enum ExportResult {
    /// The artifact was downloaded to `path`.
    Downloaded {
        job_id: String,
        path: PathBuf,
        artifact_hash: Option<String>,
        verified: bool,
    },
    /// `--no-wait`: the job exists but the artifact does not.
    Submitted { job_id: String },
    /// Dual control queued the request; nothing was exported.
    PendingApproval {
        request_id: String,
        expires_at: Option<i64>,
    },
}

impl ExportResult {
    pub fn exit_code(&self) -> u8 {
        match self {
            ExportResult::PendingApproval { .. } => EXIT_PENDING_APPROVAL,
            _ => EXIT_OK,
        }
    }
}

/// Entry point used by `main`: parse errors and failures become messages on
/// stderr and a documented exit code, never a panic or a hang on a prompt.
pub async fn run_export_command(config: &BichonCliConfig, args: &ExportArgs) -> u8 {
    let options = match ExportOptions::from_args(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("✘ {}", e.message);
            return EXIT_USAGE;
        }
    };

    match run_export(config, &options).await {
        Ok(ExportResult::Downloaded { path, .. }) => {
            // The one line a script needs.
            println!("{}", path.display());
            EXIT_OK
        }
        Ok(ExportResult::Submitted { job_id }) => {
            println!("{job_id}");
            EXIT_OK
        }
        Ok(result @ ExportResult::PendingApproval { .. }) => {
            if let ExportResult::PendingApproval {
                request_id,
                expires_at,
            } = &result
            {
                eprintln!(
                    "This server requires approval for batch exports. \
                     Request '{}' is queued and nothing was exported.",
                    request_id
                );
                if let Some(expires_at) = expires_at {
                    eprintln!("The request expires at unix time {expires_at}.");
                }
                eprintln!("Approve it in the web UI, then export again.");
            }
            EXIT_PENDING_APPROVAL
        }
        Err(e) => {
            eprintln!("✘ {}", e.message);
            if e.usage {
                EXIT_USAGE
            } else {
                EXIT_FAILURE
            }
        }
    }
}

/// Run one non-interactive export.
pub async fn run_export(
    config: &BichonCliConfig,
    options: &ExportOptions,
) -> Result<ExportResult, ExportError> {
    // `ExportOptions::from_args` validates the same thing, but the options are
    // public so a caller that builds them by hand gets the same protection.
    if options.saved_search.is_some() == !options.account_ids.is_empty() {
        return Err(ExportError::usage(
            "choose exactly one source: --saved-search <id|name> or --account-id <id>",
        ));
    }

    if let Some(path) = &options.output {
        if path.is_dir() {
            return Err(ExportError::usage(format!(
                "--output expects a file path but '{}' is a directory; use --output-dir",
                path.display()
            )));
        }
    }

    let client = Client::builder()
        .build()
        .map_err(|e| ExportError::other(format!("could not create the HTTP client: {e}")))?;

    let (create_request, label) = resolve_source(&client, config, options).await?;

    let preview_request = ExportPreviewRequest {
        saved_search_id: create_request.saved_search_id.clone(),
        filter: create_request.filter.clone(),
        label: create_request.label.clone(),
        format: create_request.format,
    };
    let preview = preview_export_request(&client, config, &preview_request)
        .await
        .map_err(ExportError::api)?;
    eprintln!(
        "Selection: {} message(s) across {} account(s), {} — '{}'",
        preview.total_emails,
        preview.accounts.len(),
        crate::export::format_bytes(preview.total_size),
        preview.saved_search_name
    );
    if preview.total_emails == 0 {
        eprintln!("Warning: the selection matches no messages; the artifact will be empty.");
    }

    let job = match create_export_request(&client, config, &create_request)
        .await
        .map_err(ExportError::api)?
    {
        CreateExportOutcome::PendingApproval {
            request_id,
            expires_at,
        } => {
            return Ok(ExportResult::PendingApproval {
                request_id,
                expires_at,
            })
        }
        CreateExportOutcome::Created(job) => job,
    };
    eprintln!("Export job: {}", job.job_id);

    if !options.wait {
        return Ok(ExportResult::Submitted {
            job_id: job.job_id,
        });
    }

    let job = wait_for_job(&client, config, job, options).await?;

    let extension = artifact_extension(&job, options.format);
    let target = match (&options.output, &options.output_dir) {
        (Some(path), _) => path.clone(),
        (_, Some(dir)) => dir.join(export_file_name(&label, chrono::Utc::now(), &extension)),
        _ => {
            return Err(ExportError::usage(
                "choose exactly one output: --output <file> or --output-dir <dir>",
            ))
        }
    };
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ExportError::other(format!("could not create '{}': {e}", parent.display()))
            })?;
        }
    }

    download_export_to_file_result(&client, config, &job.job_id, &target)
        .await
        .map_err(ExportError::api)?;
    eprintln!("Downloaded '{}'", target.display());

    if options.verify {
        let view = verify_export_job_result(&client, config, &job.job_id)
            .await
            .map_err(|e| {
                if e.status == Some(404) {
                    // The community server does not expose the verify route,
                    // so a 404 here is nearly always "wrong edition" rather
                    // than "wrong job id" — say so instead of echoing an
                    // opaque not-found.
                    ExportError::other(format!(
                        "compliance verification is not available on this server ({e}); \
                         it is provided by the Bichon Pro server"
                    ))
                } else {
                    ExportError::api(e)
                }
            })?;
        eprintln!(
            "Verification: hash match {}, {} checked, {} matched, {} mismatched",
            view.artifact_hash_match, view.checked, view.matched, view.mismatched
        );
        if !view.artifact_hash_match || view.mismatched > 0 {
            return Err(ExportError::other(
                "compliance verification failed; the artifact does not match the archive",
            ));
        }
    }

    if let (Some(keep), Some(dir)) = (options.keep, &options.output_dir) {
        match prune_exports(dir, keep) {
            Ok(removed) => {
                for path in removed {
                    eprintln!("Pruned '{}'", path.display());
                }
            }
            Err(e) => eprintln!("Warning: could not prune old exports: {e}"),
        }
    }

    Ok(ExportResult::Downloaded {
        job_id: job.job_id,
        path: target,
        artifact_hash: job.artifact_hash,
        verified: options.verify,
    })
}

async fn resolve_source(
    client: &Client,
    config: &BichonCliConfig,
    options: &ExportOptions,
) -> Result<(ExportCreateRequest, String), ExportError> {
    if let Some(input) = &options.saved_search {
        let kind = match options.format {
            ExportFormat::Mbox => SavedSearchKind::Email,
            ExportFormat::Attachments => SavedSearchKind::Attachment,
        };
        let searches = list_saved_searches_result(client, config, &kind)
            .await
            .map_err(ExportError::api)?;
        let search = resolve_saved_search(&searches, input)?;
        Ok((
            ExportCreateRequest {
                saved_search_id: Some(search.id.clone()),
                filter: None,
                label: None,
                format: Some(options.format),
            },
            search.name.clone(),
        ))
    } else {
        let accounts: HashSet<u64> = options.account_ids.iter().copied().collect();
        let label = format!(
            "accounts-{}",
            options
                .account_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join("-")
        );
        Ok((
            ExportCreateRequest {
                saved_search_id: None,
                filter: Some(ExportFilter {
                    email: Some(EmailSearchFilter {
                        account_ids: Some(accounts),
                        ..Default::default()
                    }),
                    attachment: None,
                }),
                label: Some(format!("cli {label}")),
                format: Some(options.format),
            },
            label,
        ))
    }
}

/// Resolve `--saved-search` against the caller's saved searches: an exact id
/// wins, otherwise the name must match exactly one search. Ambiguity is an
/// error rather than a guess — silently exporting the wrong search would be a
/// data-loss-shaped bug.
fn resolve_saved_search<'a>(
    searches: &'a [SavedSearchModel],
    input: &str,
) -> Result<&'a SavedSearchModel, ExportError> {
    if let Some(search) = searches.iter().find(|search| search.id == input) {
        return Ok(search);
    }

    let matches: Vec<&SavedSearchModel> = searches
        .iter()
        .filter(|search| search.name == input)
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => {
            let available = if searches.is_empty() {
                "none".to_string()
            } else {
                searches
                    .iter()
                    .map(|search| format!("'{}' ({})", search.name, search.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            Err(ExportError::other(format!(
                "no saved search with id or name '{input}'; available: {available}"
            )))
        }
        _ => Err(ExportError::other(format!(
            "'{input}' matches {} saved searches; pass the id instead: {}",
            matches.len(),
            matches
                .iter()
                .map(|search| format!("{} ({})", search.name, search.id))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

async fn wait_for_job(
    client: &Client,
    config: &BichonCliConfig,
    mut job: ExportJobView,
    options: &ExportOptions,
) -> Result<ExportJobView, ExportError> {
    let deadline = Instant::now() + options.timeout;
    let mut last_log = Instant::now();

    loop {
        match job.status.as_str() {
            "finished" => return Ok(job),
            "failed" | "cancelled" => {
                let detail = job
                    .error
                    .as_deref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default();
                return Err(ExportError::other(format!(
                    "export '{}' ended with status '{}'{}",
                    job.job_id, job.status, detail
                )));
            }
            _ => {}
        }

        if Instant::now() >= deadline {
            return Err(ExportError::other(format!(
                "timed out after {}s waiting for export '{}' (last status '{}')",
                options.timeout.as_secs(),
                job.job_id,
                job.status
            )));
        }

        tokio::time::sleep(options.poll_interval).await;
        job = get_export_job_result(client, config, &job.job_id)
            .await
            .map_err(ExportError::api)?;

        if last_log.elapsed() >= Duration::from_secs(10) {
            eprintln!(
                "… {}: {}/{} processed, {} exported, {} failed",
                job.status, job.processed, job.total_emails, job.exported, job.failed
            );
            last_log = Instant::now();
        }
    }
}

fn artifact_extension(job: &ExportJobView, format: ExportFormat) -> String {
    job.artifact_name
        .as_deref()
        .and_then(|name| Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_string())
        .unwrap_or_else(|| match format {
            ExportFormat::Mbox => "mbox".to_string(),
            ExportFormat::Attachments => "zip".to_string(),
        })
}

/// `bichon-export-<slug>-<utc timestamp>.<ext>`.
///
/// The timestamp is fixed-width and UTC, so a lexicographic sort of the
/// directory is also a chronological one — which is what `--keep` relies on.
fn export_file_name(label: &str, now: chrono::DateTime<chrono::Utc>, extension: &str) -> String {
    format!(
        "{}{}-{}.{}",
        EXPORT_PREFIX,
        slug(label),
        now.format("%Y%m%dT%H%M%SZ"),
        extension
    )
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
            if out.len() >= 40 {
                break;
            }
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "export".to_string()
    } else {
        out
    }
}

/// Delete the oldest `bichon-export-*` files, keeping the newest `keep`.
///
/// Only files this module would have written are considered, so `--keep` is
/// safe to point at a shared directory.
fn prune_exports(dir: &Path, keep: usize) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("mbox") | Some("zip")
            )
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with(EXPORT_PREFIX))
                .unwrap_or(false)
        })
        .collect();

    files.sort();
    let excess = files.len().saturating_sub(keep);
    let mut removed = Vec::with_capacity(excess);
    for path in files.into_iter().take(excess) {
        if std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ── a minimal HTTP/1.1 server ────────────────────────────────────────
    //
    // The CLI only talks to the Bichon REST API, so a canned router over a
    // local socket exercises the real reqwest client end to end without a
    // Bichon install or a new test dependency.

    type Handler = Arc<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>;

    struct MockServer {
        base_url: String,
        _handle: tokio::task::JoinHandle<()>,
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "OK",
        }
    }

    async fn spawn_mock(handler: Handler) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let header_end = loop {
                        let read = match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => read,
                        };
                        buffer.extend_from_slice(&chunk[..read]);
                        if let Some(position) =
                            buffer.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            break position + 4;
                        }
                    };

                    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                    let mut lines = head.lines();
                    let request_line = lines.next().unwrap_or_default();
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts
                        .next()
                        .unwrap_or_default()
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_string();

                    let content_length: usize = lines
                        .take_while(|line| !line.is_empty())
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if name.eq_ignore_ascii_case("content-length") {
                                value.trim().parse().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    let mut have = buffer.len().saturating_sub(header_end);
                    while have < content_length {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => have += read,
                        }
                    }

                    let (status, body) = handler(&method, &path);
                    let response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status,
                        reason(status),
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });

        MockServer {
            base_url: format!("http://{address}"),
            _handle: handle,
        }
    }

    // ── canned payloads ─────────────────────────────────────────────────

    fn saved_search_list() -> String {
        serde_json::json!([{
            "id": "ss_1",
            "user_id": 1,
            "name": "Nightly",
            "kind": "Email",
            "filter": {},
            "created_at": 0,
            "updated_at": 0,
        }])
        .to_string()
    }

    fn preview(total: u64) -> String {
        serde_json::json!({
            "saved_search_id": "ss_1",
            "saved_search_name": "Nightly",
            "format": "Mbox",
            "accounts": [{"id": 1, "email": "a@example.com", "name": null}],
            "total_emails": total,
            "total_size": 1024,
        })
        .to_string()
    }

    fn job(status: &str, artifact: Option<&str>) -> String {
        serde_json::json!({
            "job_id": "exp_1",
            "status": status,
            "saved_search_id": "ss_1",
            "saved_search_name": "Nightly",
            "format": "Mbox",
            "accounts": [],
            "total_emails": 1,
            "total_size": 10,
            "processed": 1,
            "exported": 1,
            "failed": 0,
            "error": null,
            "artifact_name": artifact,
            "artifact_size": 10,
            "artifact_hash": artifact.map(|_| "abc123"),
            "created_at": 0,
            "finished_at": null,
        })
        .to_string()
    }

    fn verify_progress(finished: bool, hash_match: bool) -> String {
        let result = serde_json::json!({
            "job_id": "exp_1",
            "status": "finished",
            "artifact_name": "exp_1.mbox",
            "expected_artifact_hash": "abc123",
            "actual_artifact_hash": if hash_match { "abc123" } else { "def456" },
            "artifact_hash_match": hash_match,
            "checked": 1,
            "matched": if hash_match { 1 } else { 0 },
            "mismatched": if hash_match { 0 } else { 1 },
            "verified_at": 0,
            "mismatches": [],
        });
        serde_json::json!({
            "job_id": "exp_1",
            "status": if finished { "finished" } else { "running" },
            "checked": 1,
            "total": 1,
            "matched": if hash_match { 1 } else { 0 },
            "mismatched": if hash_match { 0 } else { 1 },
            "error": null,
            "result": if finished { result } else { serde_json::Value::Null },
            "started_at": 0,
            "finished_at": null,
        })
        .to_string()
    }

    fn config(server: &MockServer) -> BichonCliConfig {
        BichonCliConfig {
            base_url: server.base_url.clone(),
            api_token: "tok".to_string(),
        }
    }

    fn options(output_dir: PathBuf) -> ExportOptions {
        ExportOptions {
            saved_search: Some("Nightly".to_string()),
            account_ids: Vec::new(),
            format: ExportFormat::Mbox,
            output: None,
            output_dir: Some(output_dir),
            wait: true,
            timeout: Duration::from_secs(5),
            verify: false,
            keep: None,
            poll_interval: Duration::from_millis(5),
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "bichon-cli-test-{}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst),
                label
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ARTIFACT: &str = "From: a@example.com\r\nSubject: hi\r\n\r\nhello\r\n";

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn downloads_the_finished_artifact() {
        let polls = Arc::new(AtomicUsize::new(0));
        let polls_in_handler = polls.clone();
        let server = spawn_mock(Arc::new(move |method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => {
                    if polls_in_handler.fetch_add(1, Ordering::SeqCst) == 0 {
                        (200, job("running", None))
                    } else {
                        (200, job("finished", Some("exp_1.mbox")))
                    }
                }
                ("GET", "/api/v1/exports/exp_1/download") => (200, ARTIFACT.to_string()),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("happy");
        let result = run_export(&config(&server), &options(dir.0.clone()))
            .await
            .expect("export should succeed");

        let ExportResult::Downloaded { path, .. } = result else {
            panic!("expected a downloaded artifact, got {result:?}");
        };
        assert!(path.starts_with(&dir.0));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(EXPORT_PREFIX));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ARTIFACT);
        assert_eq!(
            std::fs::read_dir(&dir.0).unwrap().count(),
            1,
            "the .part file must not survive a successful download"
        );
        assert!(polls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn reports_a_pending_approval_instead_of_success() {
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (
                    202,
                    r#"{"ok":false,"pending":true,"request_id":"req_1","expires_at":7}"#.to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("pending");
        let result = run_export(&config(&server), &options(dir.0.clone()))
            .await
            .expect("a queued request is not an error");

        match &result {
            ExportResult::PendingApproval { request_id, .. } => {
                assert_eq!(request_id, "req_1");
            }
            other => panic!("expected a pending approval, got {other:?}"),
        }
        assert_eq!(result.exit_code(), EXIT_PENDING_APPROVAL);
        assert_eq!(
            std::fs::read_dir(&dir.0).unwrap().count(),
            0,
            "nothing may be written when the export was only queued"
        );
    }

    #[tokio::test]
    async fn fails_when_the_job_fails() {
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => {
                    let mut value: serde_json::Value =
                        serde_json::from_str(&job("failed", None)).unwrap();
                    value["error"] = serde_json::json!("disk full");
                    (200, value.to_string())
                }
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("failed");
        let error = run_export(&config(&server), &options(dir.0.clone()))
            .await
            .expect_err("a failed job must not look successful");
        assert!(error.message.contains("disk full"), "{}", error.message);
        assert_eq!(error.usage, false);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn times_out_waiting_for_a_running_job() {
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => (200, job("running", None)),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("timeout");
        let mut opts = options(dir.0.clone());
        opts.timeout = Duration::from_millis(50);
        let error = run_export(&config(&server), &opts)
            .await
            .expect_err("a job that never finishes must time out");
        assert!(error.message.contains("timed out"), "{}", error.message);
    }

    #[tokio::test]
    async fn no_wait_returns_the_job_id_without_polling() {
        let status_reads = Arc::new(AtomicUsize::new(0));
        let status_reads_in_handler = status_reads.clone();
        let server = spawn_mock(Arc::new(move |method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => {
                    status_reads_in_handler.fetch_add(1, Ordering::SeqCst);
                    (500, "{}".to_string())
                }
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("no-wait");
        let mut opts = options(dir.0.clone());
        opts.wait = false;
        let result = run_export(&config(&server), &opts).await.unwrap();
        assert!(matches!(
            result,
            ExportResult::Submitted { ref job_id } if job_id == "exp_1"
        ));
        assert_eq!(status_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn verification_failure_fails_the_run() {
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => (200, job("finished", Some("exp_1.mbox"))),
                ("GET", "/api/v1/exports/exp_1/download") => (200, ARTIFACT.to_string()),
                ("POST", "/api/v1/exports/exp_1/verify") => (200, "{}".to_string()),
                ("GET", "/api/v1/exports/exp_1/verify") => (200, verify_progress(true, false)),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("verify-fails");
        let mut opts = options(dir.0.clone());
        opts.verify = true;
        let error = run_export(&config(&server), &opts)
            .await
            .expect_err("a hash mismatch must fail the run");
        assert!(
            error.message.contains("verification failed"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn a_missing_verify_route_is_explained() {
        // The community server has no verify endpoint; a script should learn
        // that it is an edition difference, not a broken job id.
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => (200, job("finished", Some("exp_1.mbox"))),
                ("GET", "/api/v1/exports/exp_1/download") => (200, ARTIFACT.to_string()),
                ("POST", "/api/v1/exports/exp_1/verify") => (
                    404,
                    r#"{"code":30000,"message":"not found"}"#.to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("verify-missing");
        let mut opts = options(dir.0.clone());
        opts.verify = true;
        let error = run_export(&config(&server), &opts).await.unwrap_err();
        assert!(
            error.message.contains("Bichon Pro server"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn verification_success_is_reported() {
        let server = spawn_mock(Arc::new(|method: &str, path: &str| -> (u16, String) {
            match (method, path) {
                ("GET", "/api/v1/saved-searches") => (200, saved_search_list()),
                ("POST", "/api/v1/exports/preview") => (200, preview(1)),
                ("POST", "/api/v1/exports") => (200, job("running", None)),
                ("GET", "/api/v1/exports/exp_1") => (200, job("finished", Some("exp_1.mbox"))),
                ("GET", "/api/v1/exports/exp_1/download") => (200, ARTIFACT.to_string()),
                ("POST", "/api/v1/exports/exp_1/verify") => (200, "{}".to_string()),
                ("GET", "/api/v1/exports/exp_1/verify") => (200, verify_progress(true, true)),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("verify-ok");
        let mut opts = options(dir.0.clone());
        opts.verify = true;
        let result = run_export(&config(&server), &opts).await.unwrap();
        assert!(matches!(
            result,
            ExportResult::Downloaded {
                verified: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn exports_accounts_without_a_saved_search() {
        let seen_create = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_create_in_handler = seen_create.clone();
        let server = spawn_mock(Arc::new(move |method: &str, path: &str| -> (u16, String) {
            if method == "POST" && path == "/api/v1/exports" {
                seen_create_in_handler
                    .lock()
                    .unwrap()
                    .push("create".to_string());
            }
            match (method, path) {
                ("POST", "/api/v1/exports/preview") => (200, preview(2)),
                ("POST", "/api/v1/exports") => (200, job("finished", Some("exp_1.mbox"))),
                ("GET", "/api/v1/exports/exp_1/download") => (200, ARTIFACT.to_string()),
                _ => (404, "{}".to_string()),
            }
        }))
        .await;

        let dir = TempDir::new("accounts");
        let opts = ExportOptions {
            saved_search: None,
            account_ids: vec![2, 1, 2],
            format: ExportFormat::Mbox,
            output: Some(dir.0.join("snapshot.mbox")),
            output_dir: None,
            wait: true,
            timeout: Duration::from_secs(5),
            verify: false,
            keep: None,
            poll_interval: Duration::from_millis(5),
        };
        let result = run_export(&config(&server), &opts).await.unwrap();
        let ExportResult::Downloaded { path, .. } = result else {
            panic!("expected a download");
        };
        assert_eq!(path, dir.0.join("snapshot.mbox"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), ARTIFACT);
        assert_eq!(seen_create.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_an_invalid_invocation_before_calling_the_server() {
        let server = spawn_mock(Arc::new(|_: &str, _: &str| -> (u16, String) {
            (500, "{}".to_string())
        }))
        .await;
        let mut opts = options(PathBuf::from("."));
        opts.saved_search = None;
        opts.account_ids = Vec::new();
        let error = run_export(&config(&server), &opts).await.unwrap_err();
        assert!(error.usage);
    }

    #[test]
    fn resolves_saved_searches_by_id_name_and_ambiguity() {
        let searches = vec![
            SavedSearchModel {
                id: "ss_1".into(),
                user_id: 1,
                name: "Nightly".into(),
                kind: SavedSearchKind::Email,
                filter: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            SavedSearchModel {
                id: "ss_2".into(),
                user_id: 1,
                name: "Twin".into(),
                kind: SavedSearchKind::Email,
                filter: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
            SavedSearchModel {
                id: "ss_3".into(),
                user_id: 1,
                name: "Twin".into(),
                kind: SavedSearchKind::Email,
                filter: serde_json::json!({}),
                created_at: 0,
                updated_at: 0,
            },
        ];

        assert_eq!(resolve_saved_search(&searches, "ss_2").unwrap().id, "ss_2");
        assert_eq!(resolve_saved_search(&searches, "Nightly").unwrap().id, "ss_1");

        let ambiguous = resolve_saved_search(&searches, "Twin").unwrap_err();
        assert!(ambiguous.message.contains("matches 2 saved searches"));

        let missing = resolve_saved_search(&searches, "nope").unwrap_err();
        assert!(missing.message.contains("no saved search"));
        assert!(missing.message.contains("ss_1"));
    }

    #[test]
    fn file_names_sort_chronologically_and_are_filesystem_safe() {
        let early = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let late = chrono::DateTime::parse_from_rfc3339("2026-11-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let early_name = export_file_name("Nightly Backup / All Mail!", early, "mbox");
        let late_name = export_file_name("Nightly Backup / All Mail!", late, "mbox");

        assert_eq!(early_name, "bichon-export-nightly-backup-all-mail-20260102T030405Z.mbox");
        assert!(early_name < late_name);
        assert!(!early_name.contains(' '));
        assert!(!early_name.contains('/'));
        assert_eq!(export_file_name("", early, "zip"), "bichon-export-export-20260102T030405Z.zip");
    }

    #[test]
    fn pruning_only_touches_its_own_files() {
        let dir = TempDir::new("prune");
        let ts = |second: u32| {
            chrono::DateTime::parse_from_rfc3339(&format!("2026-01-0{second}T00:00:00Z"))
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let oldest = dir.0.join(export_file_name("nightly", ts(1), "mbox"));
        let middle = dir.0.join(export_file_name("nightly", ts(2), "mbox"));
        let newest = dir.0.join(export_file_name("nightly", ts(3), "zip"));
        let foreign = dir.0.join("notes.mbox");
        let other = dir.0.join("bichon-export-keep-me.txt");
        for path in [&oldest, &middle, &newest, &foreign, &other] {
            std::fs::write(path, b"x").unwrap();
        }

        let removed = prune_exports(&dir.0, 2).unwrap();

        assert_eq!(removed, vec![oldest.clone()]);
        assert!(!oldest.exists());
        assert!(middle.exists());
        assert!(newest.exists());
        assert!(foreign.exists(), "files with another prefix must survive");
        assert!(other.exists(), "non-archive files must survive");
    }

    #[test]
    fn pruning_with_zero_removes_everything_it_owns() {
        let dir = TempDir::new("prune-zero");
        let file = dir.0.join(export_file_name("nightly", chrono::Utc::now(), "mbox"));
        std::fs::write(&file, b"x").unwrap();
        let removed = prune_exports(&dir.0, 0).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(!file.exists());
    }

    #[test]
    fn validates_argument_combinations() {
        let base = ExportArgs {
            saved_search: Some("Nightly".into()),
            account_id: Vec::new(),
            format: FormatArg::Mbox,
            output: None,
            output_dir: Some(PathBuf::from(".")),
            no_wait: false,
            timeout_secs: 10,
            verify: false,
            keep: None,
        };

        assert!(ExportOptions::from_args(&base).is_ok());

        let mut both_sources = base.clone();
        both_sources.account_id = vec![1];
        assert!(ExportOptions::from_args(&both_sources).unwrap_err().usage);

        let mut no_source = base.clone();
        no_source.saved_search = None;
        assert!(ExportOptions::from_args(&no_source).unwrap_err().usage);

        let mut no_output = base.clone();
        no_output.output_dir = None;
        assert!(ExportOptions::from_args(&no_output).unwrap_err().usage);

        let mut attachments_by_account = base.clone();
        attachments_by_account.saved_search = None;
        attachments_by_account.account_id = vec![1];
        attachments_by_account.format = FormatArg::Attachments;
        assert!(ExportOptions::from_args(&attachments_by_account)
            .unwrap_err()
            .message
            .contains("attachment saved search"));

        let mut no_timeout = base.clone();
        no_timeout.timeout_secs = 0;
        assert!(ExportOptions::from_args(&no_timeout).unwrap_err().usage);

        let mut keep_without_dir = base.clone();
        keep_without_dir.output_dir = None;
        keep_without_dir.output = Some(PathBuf::from("a.mbox"));
        keep_without_dir.keep = Some(3);
        assert!(ExportOptions::from_args(&keep_without_dir)
            .unwrap_err()
            .message
            .contains("--keep needs --output-dir"));

        let mut deduped = base.clone();
        deduped.saved_search = None;
        deduped.account_id = vec![3, 1, 3];
        assert_eq!(ExportOptions::from_args(&deduped).unwrap().account_ids, vec![1, 3]);
    }
}
