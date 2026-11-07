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

//! Server-side batch export engine.
//!
//! Exports are driven by an email saved search. The caller passes an
//! authorized account scope (computed from RBAC by the API layer); this
//! module intersects it with the saved search filter and streams matching
//! raw messages into an mbox artifact under the data-dir temp folder,
//! writing a per-message manifest for later compliance workflows.
//!
//! The scope is a set of account ids, **not** the search scope groups: a batch
//! export is also what the dual-control gate queues, and the approval record
//! carries those ids — the filter a view would add cannot be stored there yet,
//! so an export through a filtered view is refused rather than run against the
//! whole source account. See `rest::api::export::export_scope`.
//!
//! Jobs live in an in-memory registry; artifacts on disk. A TTL sweep
//! (`sweep_expired`) reclaims artifacts after `EXPORT_TTL_SECS`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bichon_core::account::migration::AccountModel;
use bichon_core::envelope::meta::BichonMetadata;
use bichon_core::error::{code::ErrorCode, BichonResult};
use bichon_core::export::{
    ExportAccount, ExportFormat, ExportJobView, ExportPreviewView, ExportSource,
    ExportVerifyMismatch, ExportVerifyProgressView, ExportVerifyView,
};
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::message::search::{
    search_attachment_impl, search_messages_impl, AttachmentSearchFilter,
    AttachmentSearchRequest, EmailSearchFilter, EmailSearchRequest, SortBy,
};
use bichon_core::raise_error;
use bichon_core::saved_search::{SavedSearchKind, SavedSearchModel};
use bichon_core::settings::dir::DATA_DIR_MANAGER;
use bichon_core::store::blob::{get_reader, BLOB_MANAGER};
use bichon_core::store::envelope::Envelope;
use bichon_core::{base64_encode, utc_now};
use bichon_core::utils::compute_content_hash;
use bichon_core::users::UserModel;
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};
use uuid::Uuid;

#[cfg(test)]
mod tests;

const EXPORT_PAGE_SIZE: u64 = 500;
/// Export artifacts are kept for this long after the job ends, then swept.
pub const EXPORT_TTL_SECS: i64 = 3600;
/// Safety buffer applied on top of the estimated export size for the disk
/// space check.
const DISK_SAFETY_FACTOR: f64 = 1.2;

const STATUS_PENDING: &str = "pending";
const STATUS_RUNNING: &str = "running";
const STATUS_FINISHED: &str = "finished";
const STATUS_FAILED: &str = "failed";
const STATUS_CANCELLED: &str = "cancelled";

const VERIFY_STATUS_IDLE: &str = "idle";
const VERIFY_STATUS_RUNNING: &str = "running";
const VERIFY_STATUS_FINISHED: &str = "finished";
const VERIFY_STATUS_FAILED: &str = "failed";

/// How long a one-time artifact download ticket stays valid.
const DOWNLOAD_TICKET_TTL_SECS: i64 = 60;

/// In-memory state of one export job. Mutated by both the control API and
/// the background runner.
struct ExportJob {
    job_id: String,
    user_id: u64,
    username: String,
    saved_search_id: String,
    saved_search_name: String,
    format: ExportFormat,
    status: String,
    accounts: Vec<ExportAccount>,
    total_emails: u64,
    total_size: u64,
    processed: u64,
    exported: u64,
    failed: u64,
    error: Option<String>,
    artifact_name: Option<String>,
    artifact_size: u64,
    artifact_hash: Option<String>,
    created_at: i64,
    finished_at: Option<i64>,
    cancel: Arc<AtomicBool>,
    /// Compliance verification state. The work runs in the background so the
    /// API can report live progress on large archives.
    verify_status: String,
    verify_checked: u64,
    verify_total: u64,
    verify_matched: u64,
    verify_mismatched: u64,
    verify_error: Option<String>,
    verify_result: Option<ExportVerifyView>,
    verify_started_at: Option<i64>,
    verify_finished_at: Option<i64>,
}

impl ExportJob {
    fn to_view(&self) -> ExportJobView {
        ExportJobView {
            job_id: self.job_id.clone(),
            status: self.status.clone(),
            saved_search_id: self.saved_search_id.clone(),
            saved_search_name: self.saved_search_name.clone(),
            format: self.format,
            accounts: self.accounts.clone(),
            total_emails: self.total_emails,
            total_size: self.total_size,
            processed: self.processed,
            exported: self.exported,
            failed: self.failed,
            error: self.error.clone(),
            artifact_name: self.artifact_name.clone(),
            artifact_size: self.artifact_size,
            artifact_hash: self.artifact_hash.clone(),
            created_at: self.created_at,
            finished_at: self.finished_at,
            verify_status: self.verify_status.clone(),
            verify_checked: self.verify_checked,
            verify_matched: self.verify_matched,
            verify_mismatched: self.verify_mismatched,
            verify_error: self.verify_error.clone(),
            verify_started_at: self.verify_started_at,
            verify_finished_at: self.verify_finished_at,
        }
    }
}

static EXPORT_JOBS: OnceLock<Mutex<HashMap<String, Arc<Mutex<ExportJob>>>>> =
    OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Arc<Mutex<ExportJob>>>> {
    EXPORT_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct DownloadTicket {
    user_id: u64,
    username: String,
    job_id: String,
    expires_at: i64,
}

static DOWNLOAD_TICKETS: OnceLock<Mutex<HashMap<String, DownloadTicket>>> = OnceLock::new();

fn download_tickets() -> &'static Mutex<HashMap<String, DownloadTicket>> {
    DOWNLOAD_TICKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Creates a short-lived, one-time ticket that lets a browser stream a
/// finished artifact without carrying the Bearer token (top-level navigation
/// cannot set headers). The ticket is single-use and expires after
/// [`DOWNLOAD_TICKET_TTL_SECS`].
pub fn create_download_ticket(user_id: u64, username: &str, job_id: &str) -> BichonResult<String> {
    let job = find_owned(user_id, job_id)?;
    let status = job.lock().unwrap().status.clone();
    if status != STATUS_FINISHED {
        return Err(raise_error!(
            format!("Export job '{job_id}' is not finished (status '{status}')."),
            ErrorCode::InvalidParameter
        ));
    }
    let ticket = Uuid::new_v4().to_string();
    let now = now_ms();
    let mut tickets = download_tickets().lock().unwrap();
    tickets.retain(|_, t| t.expires_at > now);
    tickets.insert(
        ticket.clone(),
        DownloadTicket {
            user_id,
            username: username.to_string(),
            job_id: job_id.to_string(),
            expires_at: now + DOWNLOAD_TICKET_TTL_SECS * 1000,
        },
    );
    Ok(ticket)
}

/// Resolves and consumes a one-time download ticket. Returns
/// `(user_id, username, job_id)`.
pub fn resolve_download_ticket(ticket: &str) -> Option<(u64, String, String)> {
    let now = now_ms();
    let mut tickets = download_tickets().lock().unwrap();
    tickets.retain(|_, t| t.expires_at > now);
    let entry = tickets.remove(ticket)?;
    Some((entry.user_id, entry.username, entry.job_id))
}

/// Global cap on how many export jobs may run at the same time.
const MAX_CONCURRENT_EXPORTS: usize = 2;

static EXPORT_SEMAPHORE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

fn export_semaphore() -> &'static tokio::sync::Semaphore {
    EXPORT_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_EXPORTS))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn export_root_dir() -> PathBuf {
    DATA_DIR_MANAGER.exports_dir.clone()
}

fn job_dir(job_id: &str) -> PathBuf {
    export_root_dir().join(job_id)
}

fn new_job_id() -> String {
    format!(
        "exp_{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

/// Inverse of [`ExportFormat::as_str`], for manifests written to disk. Keeping
/// the two directions next to each other means a new format cannot be added to
/// one side and forgotten in the other.
fn export_format_from_wire(value: &str) -> Option<ExportFormat> {
    [ExportFormat::Mbox, ExportFormat::Attachments]
        .into_iter()
        .find(|format| format.as_str() == value)
}

/// The saved-search kind an export format drives on: `Mbox` exports whole
/// messages from an `Email` search; `Attachments` exports the attachments an
/// `Attachment` search matches.
fn ensure_search_kind(format: ExportFormat, kind: &SavedSearchKind) -> BichonResult<()> {
    let ok = match format {
        ExportFormat::Mbox => kind == &SavedSearchKind::Email,
        ExportFormat::Attachments => kind == &SavedSearchKind::Attachment,
    };
    if ok {
        Ok(())
    } else {
        Err(raise_error!(
            format!(
                "Export format '{}' requires a saved search of kind '{}'.",
                format.as_str(),
                match format {
                    ExportFormat::Mbox => "Email",
                    ExportFormat::Attachments => "Attachment",
                }
            ),
            ErrorCode::InvalidParameter
        ))
    }
}

fn parse_filter(value: serde_json::Value) -> BichonResult<EmailSearchFilter> {
    serde_json::from_value(value).map_err(|e| {
        raise_error!(
            format!("Saved search filter is invalid: {e}"),
            ErrorCode::InvalidParameter
        )
    })
}

fn parse_attachment_filter(value: serde_json::Value) -> BichonResult<AttachmentSearchFilter> {
    serde_json::from_value(value).map_err(|e| {
        raise_error!(
            format!("Saved search filter is invalid: {e}"),
            ErrorCode::InvalidParameter
        )
    })
}

/// The search plan an export runs: which index drives the export and with
/// what filter. Owns the parsed saved-search conditions for both formats.
#[derive(Clone)]
enum ExportPlan {
    Mbox(EmailSearchFilter),
    Attachments(AttachmentSearchFilter),
}

fn find_owned(user_id: u64, job_id: &str) -> BichonResult<Arc<Mutex<ExportJob>>> {
    let jobs = registry().lock().unwrap();
    let job = jobs.get(job_id).cloned().ok_or_else(|| {
        raise_error!(
            format!("Export job '{job_id}' not found."),
            ErrorCode::ResourceNotFound
        )
    })?;
    let owner = job.lock().unwrap().user_id;
    if owner != user_id {
        return Err(raise_error!(
            "Permission denied: this export job belongs to another user.".into(),
            ErrorCode::Forbidden
        ));
    }
    Ok(job)
}

/// Runs a metadata-only pass over the search result to count matching emails
/// and sum their sizes, and resolves the accounts they belong to.
fn estimate(
    scope: &Option<HashSet<u64>>,
    filter: &EmailSearchFilter,
) -> BichonResult<(Vec<ExportAccount>, u64, u64)> {
    let mut account_ids: HashSet<u64> = HashSet::new();
    let mut total_size: u64 = 0;
    let mut page: u64 = 1;
    let mut total_emails: u64 = 0;

    loop {
        let request = EmailSearchRequest {
            filter: filter.clone(),
            page,
            page_size: EXPORT_PAGE_SIZE,
            sort_by: Some(SortBy::DATE),
            desc: Some(false),
        };
        let data = search_messages_impl(scope.clone(), request)?;
        if total_emails == 0 {
            total_emails = data.total_items;
        }
        for env in &data.items {
            total_size += env.size as u64;
            account_ids.insert(env.account_id);
        }
        let pages = data.total_pages.unwrap_or(1).max(1);
        if page >= pages {
            break;
        }
        page += 1;
    }

    let mut ids: Vec<u64> = account_ids.into_iter().collect();
    ids.sort_unstable();
    let mut accounts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(acc) = AccountModel::find(id)? {
            accounts.push(ExportAccount {
                id,
                email: acc.email,
                name: acc.account_name,
            });
        }
    }
    Ok((accounts, total_emails, total_size))
}

/// Attachment-export variant of [`estimate`]: pages the attachment search and
/// counts *unique* attachments (by content hash) plus their total size, so a
/// photo attached to ten emails counts once — the same dedup the export runs.
fn estimate_attachments(
    scope: &Option<HashSet<u64>>,
    filter: &AttachmentSearchFilter,
) -> BichonResult<(Vec<ExportAccount>, u64, u64)> {
    let mut account_ids: HashSet<u64> = HashSet::new();
    let mut unique: HashMap<String, u64> = HashMap::new();
    let mut page: u64 = 1;

    loop {
        let request = AttachmentSearchRequest {
            filter: filter.clone(),
            page,
            page_size: EXPORT_PAGE_SIZE,
            sort_by: Some(SortBy::DATE),
            desc: Some(false),
        };
        let data = search_attachment_impl(scope.clone(), request)?;
        for att in &data.items {
            account_ids.insert(att.account_id);
            unique.entry(att.content_hash.clone()).or_insert(att.size);
        }
        let pages = data.total_pages.unwrap_or(1).max(1);
        if page >= pages {
            break;
        }
        page += 1;
    }

    let total_size: u64 = unique.values().sum();
    let total_attachments = unique.len() as u64;

    let mut ids: Vec<u64> = account_ids.into_iter().collect();
    ids.sort_unstable();
    let mut accounts = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(acc) = AccountModel::find(id)? {
            accounts.push(ExportAccount {
                id,
                email: acc.email,
                name: acc.account_name,
            });
        }
    }
    Ok((accounts, total_attachments, total_size))
}

/// Verifies there is enough free disk space for the estimated export size.
fn check_disk_space(required: u64) -> BichonResult<()> {
    let dir = export_root_dir();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk = disks
        .list()
        .iter()
        .find(|d| dir.starts_with(d.mount_point()))
        .ok_or_else(|| {
            raise_error!(
                "Could not identify the disk for the export directory.".into(),
                ErrorCode::InternalError
            )
        })?;

    let free_space = disk.available_space();
    let required_with_buffer = (required as f64 * DISK_SAFETY_FACTOR) as u64;
    if free_space < required_with_buffer {
        return Err(raise_error!(
            format!(
                "Insufficient disk space for export: {} bytes required (with safety buffer), {} bytes available.",
                required_with_buffer, free_space
            ),
            ErrorCode::InternalError
        ));
    }
    Ok(())
}

/// A selection resolved against the archive: the display metadata a job or an
/// approval card shows, plus the search plan the export actually runs.
struct ResolvedSelection {
    /// Empty for ad-hoc selections — there is no saved search to point at.
    saved_search_id: String,
    saved_search_name: String,
    plan: ExportPlan,
}

/// Resolves "what to export": a saved search replayed, or an ad-hoc filter
/// built by the UI (e.g. "all images", "all PDFs"). Exactly one source is
/// accepted; two would be ambiguous and none is a caller bug, so both are
/// refused rather than guessed at.
fn resolve_selection(
    user_id: u64,
    source: &ExportSource,
    format: ExportFormat,
) -> BichonResult<ResolvedSelection> {
    // An empty id is the ledger's spelling of "no saved search"; treat it as
    // absent rather than as an id that could never resolve.
    let saved_search_id = source
        .saved_search_id
        .as_deref()
        .filter(|id| !id.is_empty());
    match (saved_search_id, source.filter.as_ref()) {
        (Some(_), Some(_)) => Err(raise_error!(
            "Provide either a saved search or a filter, not both.".into(),
            ErrorCode::InvalidParameter
        )),
        (None, None) => Err(raise_error!(
            "Provide a saved search or a filter.".into(),
            ErrorCode::InvalidParameter
        )),
        (Some(id), None) => {
            let saved = SavedSearchModel::get_owned(user_id, id)?;
            ensure_search_kind(format, &saved.kind)?;
            let plan = match format {
                ExportFormat::Mbox => ExportPlan::Mbox(parse_filter(saved.filter.clone())?),
                ExportFormat::Attachments => {
                    ExportPlan::Attachments(parse_attachment_filter(saved.filter.clone())?)
                }
            };
            Ok(ResolvedSelection {
                saved_search_id: saved.id,
                saved_search_name: saved.name,
                plan,
            })
        }
        (None, Some(filter)) => {
            // The filter half must match the format — an email filter with
            // `Attachments` (or the reverse) would silently export something
            // other than what the caller asked for.
            let plan = match format {
                ExportFormat::Mbox => {
                    let email = filter.email.clone().ok_or_else(|| {
                        raise_error!(
                            "Export format 'mbox' requires an email filter.".into(),
                            ErrorCode::InvalidParameter
                        )
                    })?;
                    if filter.attachment.is_some() {
                        return Err(raise_error!(
                            "Export format 'mbox' does not accept an attachment filter.".into(),
                            ErrorCode::InvalidParameter
                        ));
                    }
                    ExportPlan::Mbox(email)
                }
                ExportFormat::Attachments => {
                    let attachment = filter.attachment.clone().ok_or_else(|| {
                        raise_error!(
                            "Export format 'attachments' requires an attachment filter.".into(),
                            ErrorCode::InvalidParameter
                        )
                    })?;
                    if filter.email.is_some() {
                        return Err(raise_error!(
                            "Export format 'attachments' does not accept an email filter.".into(),
                            ErrorCode::InvalidParameter
                        ));
                    }
                    ExportPlan::Attachments(attachment)
                }
            };
            let name = source
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or("Ad-hoc export")
                .to_string();
            Ok(ResolvedSelection {
                saved_search_id: String::new(),
                saved_search_name: name,
                plan,
            })
        }
    }
}

fn estimate_for(
    scope: &Option<HashSet<u64>>,
    plan: &ExportPlan,
) -> BichonResult<(Vec<ExportAccount>, u64, u64)> {
    match plan {
        ExportPlan::Mbox(filter) => estimate(scope, filter),
        ExportPlan::Attachments(filter) => estimate_attachments(scope, filter),
    }
}

/// Computes what an export would contain, restricted to the caller's
/// authorized account scope. For `Attachments` the totals are unique
/// attachments and their summed size.
pub fn preview(
    user_id: u64,
    source: &ExportSource,
    format: ExportFormat,
    scope: Option<HashSet<u64>>,
) -> BichonResult<ExportPreviewView> {
    let resolved = resolve_selection(user_id, source, format)?;
    let (accounts, total_emails, total_size) = estimate_for(&scope, &resolved.plan)?;
    Ok(ExportPreviewView {
        saved_search_id: resolved.saved_search_id,
        saved_search_name: resolved.saved_search_name,
        format,
        accounts,
        total_emails,
        total_size,
    })
}

/// Validates the request, estimates the scope, checks disk space and starts
/// the background export. Returns the initial job view.
pub fn create_export(
    user_id: u64,
    username: String,
    source: &ExportSource,
    format: ExportFormat,
    scope: Option<HashSet<u64>>,
) -> BichonResult<ExportJobView> {
    sweep_expired();

    let permit: tokio::sync::SemaphorePermit<'static> = export_semaphore().try_acquire().map_err(|_| {
        raise_error!(
            format!(
                "Too many concurrent exports running (limit {MAX_CONCURRENT_EXPORTS}). Please retry after an existing export finishes."
            )
            .into(),
            ErrorCode::TooManyRequest
        )
    })?;

    let resolved = resolve_selection(user_id, source, format)?;
    let plan = resolved.plan;
    let (accounts, total_emails, total_size) = estimate_for(&scope, &plan)?;
    check_disk_space(total_size)?;

    let job_id = new_job_id();
    let dir = job_dir(&job_id);
    std::fs::create_dir_all(&dir).map_err(|e| {
        raise_error!(
            format!("Failed to create export directory: {e}"),
            ErrorCode::InternalError
        )
    })?;

    let job = Arc::new(Mutex::new(ExportJob {
        job_id: job_id.clone(),
        user_id,
        username: username.clone(),
        saved_search_id: resolved.saved_search_id.clone(),
        saved_search_name: resolved.saved_search_name.clone(),
        format,
        status: STATUS_RUNNING.to_string(),
        accounts,
        total_emails,
        total_size,
        processed: 0,
        exported: 0,
        failed: 0,
        error: None,
        artifact_name: None,
        artifact_size: 0,
        artifact_hash: None,
        created_at: now_ms(),
        finished_at: None,
        cancel: Arc::new(AtomicBool::new(false)),
        verify_status: VERIFY_STATUS_IDLE.to_string(),
        verify_checked: 0,
        verify_total: 0,
        verify_matched: 0,
        verify_mismatched: 0,
        verify_error: None,
        verify_result: None,
        verify_started_at: None,
        verify_finished_at: None,
    }));

    {
        let mut jobs = registry().lock().unwrap();
        jobs.insert(job_id.clone(), job.clone());
    }

    let view = job.lock().unwrap().to_view();
    tokio::spawn(run_export(job, scope, plan, permit));
    emit(Event::ExportStarted {
        user: username,
        export_id: job_id,
        saved_search_id: resolved.saved_search_id,
        saved_search_name: view.saved_search_name.clone(),
        format: view.format.as_str().to_string(),
        account_count: view.accounts.len() as u64,
        email_count: view.total_emails,
    });
    Ok(view)
}

/// Snapshot of a job owned by `user_id`.
pub fn get_export(user_id: u64, job_id: &str) -> BichonResult<ExportJobView> {
    let job = find_owned(user_id, job_id)?;
    let view = job.lock().unwrap().to_view();
    Ok(view)
}

/// Lists the caller's export jobs, newest first.
pub fn list_exports(user_id: u64) -> Vec<ExportJobView> {
    let jobs = registry().lock().unwrap();
    let mut views: Vec<ExportJobView> = jobs
        .values()
        .filter_map(|job| {
            let guard = job.lock().unwrap();
            (guard.user_id == user_id).then(|| guard.to_view())
        })
        .collect();
    views.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    views
}

/// Requests cancellation of a running job. The background runner stops at the
/// next page boundary.
pub fn cancel_export(user_id: u64, job_id: &str) -> BichonResult<ExportJobView> {
    let job = find_owned(user_id, job_id)?;
    {
        let mut guard = job.lock().unwrap();
        if matches!(guard.status.as_str(), STATUS_PENDING | STATUS_RUNNING) {
            guard.cancel.store(true, Ordering::SeqCst);
            guard.status = STATUS_CANCELLED.to_string();
            guard.finished_at = Some(now_ms());
        }
    }
    if job.lock().unwrap().status == STATUS_CANCELLED {
        let guard = job.lock().unwrap();
        emit(Event::ExportCancelled {
            user: guard.username.clone(),
            export_id: job_id.to_string(),
        });
    }
    let view = job.lock().unwrap().to_view();
    Ok(view)
}

/// Removes a job from the registry and deletes its artifact directory.
pub fn delete_export(user_id: u64, job_id: &str) -> BichonResult<()> {
    let _job = find_owned(user_id, job_id)?;
    remove_job_files(job_id);
    registry().lock().unwrap().remove(job_id);
    Ok(())
}

/// Path to the finished artifact plus its filename, ready for download.
pub fn download_artifact(user_id: u64, job_id: &str) -> BichonResult<(PathBuf, String)> {
    let job = find_owned(user_id, job_id)?;
    let (status, name) = {
        let guard = job.lock().unwrap();
        (guard.status.clone(), guard.artifact_name.clone())
    };
    if status != STATUS_FINISHED {
        return Err(raise_error!(
            "Export is not ready yet.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let name = name.ok_or_else(|| {
        raise_error!(
            "Export has no artifact.".into(),
            ErrorCode::InternalError
        )
    })?;
    let path = job_dir(job_id).join(&name);
    if !path.is_file() {
        return Err(raise_error!(
            "Export artifact is missing.".into(),
            ErrorCode::ResourceNotFound
        ));
    }
    Ok((path, name))
}

fn remove_job_files(job_id: &str) {
    let dir = job_dir(job_id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            warn!("Failed to remove export dir {}: {e}", dir.display());
        }
    }
}

/// Removes finished/stale jobs older than `EXPORT_TTL_SECS`, plus orphan
/// directories under the export root that no longer map to a live job.
pub fn sweep_expired() {
    let now = now_ms();
    let ttl_ms = EXPORT_TTL_SECS * 1000;
    let mut expired: Vec<String> = Vec::new();
    {
        let jobs = registry().lock().unwrap();
        for (id, job) in jobs.iter() {
            let guard = job.lock().unwrap();
            let reference = match guard.status.as_str() {
                STATUS_FINISHED | STATUS_FAILED | STATUS_CANCELLED => {
                    guard.finished_at.unwrap_or(guard.created_at)
                }
                _ => guard.created_at,
            };
            if now.saturating_sub(reference) > ttl_ms {
                expired.push(id.clone());
            }
        }
    }
    for id in &expired {
        info!("Sweeping expired export job {id}");
        remove_job_files(id);
        registry().lock().unwrap().remove(id);
    }

    let root = export_root_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if registry().lock().unwrap().contains_key(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|el| el.as_secs() > EXPORT_TTL_SECS as u64)
            .unwrap_or(false);
        if stale {
            info!("Sweeping orphan export dir {name}");
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Spawns a periodic TTL sweep. Called once at server startup.
pub fn spawn_export_cleanup() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(900));
        loop {
            interval.tick().await;
            sweep_expired();
        }
    });
}

async fn run_export(
    job: Arc<Mutex<ExportJob>>,
    scope: Option<HashSet<u64>>,
    plan: ExportPlan,
    _permit: tokio::sync::SemaphorePermit<'static>,
) {
    let dir = {
        let guard = job.lock().unwrap();
        job_dir(&guard.job_id)
    };

    let result = run_export_inner(&job, &dir, scope, plan).await;
    if let Err(e) = result {
        let message = e.to_string();
        let (username, export_id, saved_search_id, saved_search_name) = {
            let guard = job.lock().unwrap();
            (
                guard.username.clone(),
                guard.job_id.clone(),
                guard.saved_search_id.clone(),
                guard.saved_search_name.clone(),
            )
        };
        {
            let mut guard = job.lock().unwrap();
            if guard.status != STATUS_CANCELLED {
                guard.status = STATUS_FAILED.to_string();
                guard.error = Some(message.clone());
                guard.finished_at = Some(now_ms());
            }
        }
        emit(Event::ExportFailed {
            user: username,
            export_id: export_id.clone(),
            saved_search_id,
            saved_search_name,
            error: message.clone(),
        });
        error!("Export {export_id} failed: {message}");
    }
}

async fn run_export_inner(
    job: &Arc<Mutex<ExportJob>>,
    dir: &Path,
    scope: Option<HashSet<u64>>,
    plan: ExportPlan,
) -> BichonResult<()> {
    match plan {
        ExportPlan::Mbox(filter) => run_mbox_export(job, dir, scope, filter).await,
        ExportPlan::Attachments(filter) => run_attachments_export(job, dir, scope, filter).await,
    }
}

async fn run_mbox_export(
    job: &Arc<Mutex<ExportJob>>,
    dir: &Path,
    scope: Option<HashSet<u64>>,
    filter: EmailSearchFilter,
) -> BichonResult<()> {
    let job_id = job.lock().unwrap().job_id.clone();
    let artifact_name = format!("{job_id}.mbox");
    let mbox_path = dir.join(&artifact_name);
    let mut mbox = tokio::fs::File::create(&mbox_path).await.map_err(|e| {
        raise_error!(
            format!("Failed to create mbox file: {e}"),
            ErrorCode::InternalError
        )
    })?;

    let manifest_path = dir.join("manifest.jsonl");
    let mut manifest = tokio::fs::File::create(&manifest_path).await.map_err(|e| {
        raise_error!(
            format!("Failed to create manifest file: {e}"),
            ErrorCode::InternalError
        )
    })?;

    let mut page: u64 = 1;
    loop {
        if is_cancelled(job) {
            return Ok(());
        }
        let request = EmailSearchRequest {
            filter: filter.clone(),
            page,
            page_size: EXPORT_PAGE_SIZE,
            sort_by: Some(SortBy::DATE),
            desc: Some(false),
        };
        let data = search_messages_impl(scope.clone(), request)?;
        let pages = data.total_pages.unwrap_or(1).max(1);

        for envelope in data.items {
            if is_cancelled(job) {
                return Ok(());
            }
            let ok = export_one(&mut mbox, &mut manifest, &envelope).await;
            let mut guard = job.lock().unwrap();
            guard.processed += 1;
            if ok.is_ok() {
                guard.exported += 1;
            } else {
                guard.failed += 1;
                if let Err(e) = ok {
                    warn!(
                        "Failed to export message {} (account {}): {e}",
                        envelope.id, envelope.account_id
                    );
                }
            }
        }

        if page >= pages {
            break;
        }
        page += 1;
    }

    mbox.flush().await.ok();
    manifest.flush().await.ok();
    let artifact_size = tokio::fs::metadata(&mbox_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let artifact_hash = sha256_file(&mbox_path).await.ok();

    finish_export(job, dir, &artifact_name, artifact_size, artifact_hash).await
}

/// Shared completion tail for both export runners: writes the summary
/// manifest, flips the job to finished with its artifact, and emits the
/// completion event.
async fn finish_export(
    job: &Arc<Mutex<ExportJob>>,
    dir: &Path,
    artifact_name: &str,
    artifact_size: u64,
    artifact_hash: Option<String>,
) -> BichonResult<()> {
    let (
        username,
        export_id,
        saved_search_id,
        saved_search_name,
        format,
        accounts,
        exported,
        failed,
        created_at,
    ) = {
        let guard = job.lock().unwrap();
        (
            guard.username.clone(),
            guard.job_id.clone(),
            guard.saved_search_id.clone(),
            guard.saved_search_name.clone(),
            guard.format,
            guard.accounts.clone(),
            guard.exported,
            guard.failed,
            guard.created_at,
        )
    };

    write_summary_manifest(
        dir,
        &export_id,
        format,
        artifact_name,
        artifact_size,
        artifact_hash.as_deref(),
        &username,
        &saved_search_id,
        &saved_search_name,
        &accounts,
        exported,
        failed,
        created_at,
    )
    .await?;

    {
        let mut guard = job.lock().unwrap();
        guard.status = STATUS_FINISHED.to_string();
        guard.artifact_name = Some(artifact_name.to_string());
        guard.artifact_size = artifact_size;
        guard.artifact_hash = artifact_hash.clone();
        guard.finished_at = Some(now_ms());
    }

    emit(Event::ExportCompleted {
        user: username,
        export_id: export_id.clone(),
        saved_search_id,
        saved_search_name,
        exported,
        failed,
        artifact_size,
        artifact_hash,
    });
    info!("Export {export_id} finished: {exported} exported, {failed} failed");
    Ok(())
}

/// Attachment-export runner: pages the attachment search the saved search
/// defines and writes every matching attachment once — deduplicated by
/// content hash, so a photo attached to ten exports exactly one file — into
/// a zip artifact, plus one manifest line per exported file.
///
/// Unlike the mbox path this branch is synchronous I/O end to end: the
/// search and `retrieve_attachment_content` are plain blob reads, so there
/// is nothing to await inside the loop (the sync calls sit in this async
/// task exactly like the mbox runner's own sync `search_messages_impl`).
async fn run_attachments_export(
    job: &Arc<Mutex<ExportJob>>,
    dir: &Path,
    scope: Option<HashSet<u64>>,
    filter: AttachmentSearchFilter,
) -> BichonResult<()> {
    let job_id = job.lock().unwrap().job_id.clone();
    let artifact_name = format!("{job_id}.zip");
    let zip_path = dir.join(&artifact_name);
    let zip_file = std::fs::File::create(&zip_path).map_err(|e| {
        raise_error!(
            format!("Failed to create zip file: {e}"),
            ErrorCode::InternalError
        )
    })?;
    let mut zip = zip::ZipWriter::new(zip_file);

    let manifest_path = dir.join("manifest.jsonl");
    let mut manifest = std::fs::File::create(&manifest_path).map_err(|e| {
        raise_error!(
            format!("Failed to create manifest file: {e}"),
            ErrorCode::InternalError
        )
    })?;

    let mut seen: HashSet<String> = HashSet::new();
    let mut page: u64 = 1;
    loop {
        if is_cancelled(job) {
            return Ok(());
        }
        let request = AttachmentSearchRequest {
            filter: filter.clone(),
            page,
            page_size: EXPORT_PAGE_SIZE,
            sort_by: Some(SortBy::DATE),
            desc: Some(false),
        };
        let data = search_attachment_impl(scope.clone(), request)?;
        let pages = data.total_pages.unwrap_or(1).max(1);

        for att in &data.items {
            if is_cancelled(job) {
                return Ok(());
            }
            // The same content reached from several mails exports once; the
            // first hit (oldest first) decides the entry's date.
            if !seen.insert(att.content_hash.clone()) {
                continue;
            }
            let ok = export_attachment_one(&mut zip, &mut manifest, att);
            let mut guard = job.lock().unwrap();
            guard.processed += 1;
            match ok {
                Ok(()) => guard.exported += 1,
                Err(e) => {
                    guard.failed += 1;
                    warn!(
                        "Failed to export attachment {} (account {}): {e}",
                        att.content_hash, att.account_id
                    );
                }
            }
        }

        if page >= pages {
            break;
        }
        page += 1;
    }

    zip.finish().map_err(|e| {
        raise_error!(
            format!("Failed to finalize zip file: {e}"),
            ErrorCode::InternalError
        )
    })?;
    let artifact_size = std::fs::metadata(&zip_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let artifact_hash = sha256_file(&zip_path).await.ok();

    finish_export(job, dir, &artifact_name, artifact_size, artifact_hash).await
}

/// Writes one attachment into the zip plus its manifest line. Content comes
/// from the same retrieval the single-attachment download uses, so the bytes
/// in the zip are identical to what the UI serves.
fn export_attachment_one(
    zip: &mut zip::ZipWriter<std::fs::File>,
    manifest: &mut std::fs::File,
    att: &bichon_core::store::tantivy::model::AttachmentModel,
) -> BichonResult<()> {
    let mut cursor = bichon_core::message::attachment::retrieve_attachment_content(
        att.account_id,
        att.envelope_id.clone(),
        &att.content_hash,
    )?;
    let mut bytes = Vec::with_capacity(att.size.min(u32::MAX as u64) as usize);
    // Fully qualified: `AsyncReadExt` is also in scope module-wide and the
    // Cursor implements both.
    std::io::Read::read_to_end(&mut cursor, &mut bytes).map_err(|e| {
        raise_error!(
            format!("Failed to read attachment {}: {e}", att.content_hash),
            ErrorCode::InternalError
        )
    })?;

    let zip_name = attachment_zip_name(att);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(compression_for(&att.category));
    zip.start_file(zip_name.clone(), options).map_err(|e| {
        raise_error!(
            format!("Failed to add zip entry '{zip_name}': {e}"),
            ErrorCode::InternalError
        )
    })?;
    std::io::Write::write_all(zip, &bytes).map_err(|e| {
        raise_error!(
            format!("Failed to write zip entry '{zip_name}': {e}"),
            ErrorCode::InternalError
        )
    })?;

    let line = serde_json::json!({
        "account_id": att.account_id,
        "envelope_id": att.envelope_id,
        "content_hash": att.content_hash,
        "filename": att.name,
        "subject": att.subject,
        "size": att.size,
        "zip_name": zip_name,
    });
    let mut buf = serde_json::to_vec(&line).map_err(|e| {
        raise_error!(
            format!("Failed to serialize manifest line: {e}"),
            ErrorCode::InternalError
        )
    })?;
    buf.push(b'\n');
    std::io::Write::write_all(manifest, &buf).map_err(|e| {
        raise_error!(
            format!("Failed to write manifest line: {e}"),
            ErrorCode::InternalError
        )
    })?;

    Ok(())
}

/// Deterministic zip entry name: mail date, short content hash, extension.
/// The hash keeps it unique across same-named attachments from different
/// mails; the date prefix makes the set chronologically sortable for a photo
/// library import.
fn attachment_zip_name(att: &bichon_core::store::tantivy::model::AttachmentModel) -> String {
    let date = Utc
        .timestamp_opt(att.date / 1000, 0)
        .unwrap()
        .format("%Y%m%d-%H%M%S");
    let hash_prefix: String = att.content_hash.chars().take(8).collect();
    let ext = att
        .ext
        .clone()
        .unwrap_or_else(|| "bin".to_string());
    format!("{date}-{hash_prefix}.{ext}")
}

/// Media that is already compressed gains nothing from deflate and only
/// burns CPU on large exports; store those entries verbatim.
fn compression_for(category: &str) -> zip::CompressionMethod {
    match category {
        "image" | "video" | "audio" | "archive" => zip::CompressionMethod::Stored,
        _ => zip::CompressionMethod::Deflated,
    }
}

/// Streams a file and returns its lowercase hex SHA-256.
async fn sha256_file(path: &Path) -> BichonResult<String> {
    let mut file = tokio::fs::File::open(path).await.map_err(|e| {
        raise_error!(
            format!("Failed to open export artifact for hashing: {e}"),
            ErrorCode::InternalError
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await.map_err(|e| {
            raise_error!(
                format!("Failed to read export artifact for hashing: {e}"),
                ErrorCode::InternalError
            )
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Compliance verification of a finished export: recomputes the artifact's
/// SHA-256 and cross-checks every exported message's content hash against
/// the live archive. Results (including any mismatches) are emitted as an
/// `ExportVerified` audit event.
///
/// Runs synchronously and returns the final result; the progress and result
/// are also persisted on the job so they survive a page refresh.
pub async fn verify_export(user_id: u64, job_id: &str) -> BichonResult<ExportVerifyView> {
    let job = find_owned(user_id, job_id)?;
    verify_job(&job).await
}

/// Starts a background compliance verification and returns its live progress.
/// Idempotent: calling it again while a verification is running (or after it
/// finished) returns the current state instead of restarting the work.
pub fn start_export_verify(user_id: u64, job_id: &str) -> BichonResult<ExportVerifyProgressView> {
    let job = find_owned(user_id, job_id)?;
    let (status, verify_status) = {
        let guard = job.lock().unwrap();
        (guard.status.clone(), guard.verify_status.clone())
    };
    if status != STATUS_FINISHED {
        return Err(raise_error!(
            format!("Export job '{job_id}' is not finished (status '{status}')."),
            ErrorCode::InvalidParameter
        ));
    }
    if matches!(
        verify_status.as_str(),
        VERIFY_STATUS_RUNNING | VERIFY_STATUS_FINISHED | VERIFY_STATUS_FAILED
    ) {
        let view = {
            let guard = job.lock().unwrap();
            verify_progress_view(&*guard)
        };
        return Ok(view);
    }
    {
        let mut guard = job.lock().unwrap();
        guard.verify_status = VERIFY_STATUS_RUNNING.to_string();
        guard.verify_checked = 0;
        guard.verify_total = 0;
        guard.verify_matched = 0;
        guard.verify_mismatched = 0;
        guard.verify_error = None;
        guard.verify_result = None;
        guard.verify_started_at = Some(now_ms());
        guard.verify_finished_at = None;
    }
    let worker = job.clone();
    tokio::spawn(async move {
        if let Err(e) = verify_job(&worker).await {
            let message = e.to_string();
            {
                let mut guard = worker.lock().unwrap();
                if guard.verify_status == VERIFY_STATUS_RUNNING {
                    guard.verify_status = VERIFY_STATUS_FAILED.to_string();
                    guard.verify_error = Some(message.clone());
                    guard.verify_finished_at = Some(now_ms());
                    if let Err(e) = persist_verify_state(&*guard) {
                        error!("Failed to persist verify state for {}: {e}", guard.job_id);
                    }
                }
            }
            error!("Verify export {} failed: {message}", worker.lock().unwrap().job_id);
        }
    });
    let view = {
        let guard = job.lock().unwrap();
        verify_progress_view(&*guard)
    };
    Ok(view)
}

/// Current progress (and final result, when finished) of a compliance
/// verification.
pub fn export_verify_progress(user_id: u64, job_id: &str) -> BichonResult<ExportVerifyProgressView> {
    let job = find_owned(user_id, job_id)?;
    let guard = job.lock().unwrap();
    Ok(verify_progress_view(&*guard))
}

fn verify_progress_view(job: &ExportJob) -> ExportVerifyProgressView {
    ExportVerifyProgressView {
        job_id: job.job_id.clone(),
        status: job.verify_status.clone(),
        checked: job.verify_checked,
        total: job.verify_total,
        matched: job.verify_matched,
        mismatched: job.verify_mismatched,
        error: job.verify_error.clone(),
        result: job.verify_result.clone(),
        started_at: job.verify_started_at,
        finished_at: job.verify_finished_at,
    }
}

/// The verification worker: reads the manifest, cross-checks every message
/// against the archive and writes progress/result back onto the job.
async fn verify_job(job: &Arc<Mutex<ExportJob>>) -> BichonResult<ExportVerifyView> {
    let (job_id, status, artifact_name, format) = {
        let guard = job.lock().unwrap();
        (
            guard.job_id.clone(),
            guard.status.clone(),
            guard.artifact_name.clone(),
            guard.format,
        )
    };
    if status != STATUS_FINISHED {
        return Err(raise_error!(
            format!("Export job '{job_id}' is not finished (status '{status}')."),
            ErrorCode::InvalidParameter
        ));
    }
    let name = artifact_name.ok_or_else(|| {
        raise_error!(
            "Export has no artifact to verify.".into(),
            ErrorCode::InternalError
        )
    })?;
    let dir = job_dir(&job_id);
    let mbox_path = dir.join(&name);
    if !mbox_path.is_file() {
        return Err(raise_error!(
            "Export artifact is missing.".into(),
            ErrorCode::ResourceNotFound
        ));
    }

    let actual_artifact_hash = sha256_file(&mbox_path).await?;
    let expected_artifact_hash = read_manifest_artifact_hash(&dir).await;

    let manifest_path = dir.join("manifest.jsonl");
    let manifest_content = tokio::fs::read_to_string(&manifest_path).await.ok();
    let total = manifest_content
        .as_deref()
        .map(|content| content.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        .unwrap_or(0);
    {
        let mut guard = job.lock().unwrap();
        guard.verify_total = total;
    }

    let mut checked: u64 = 0;
    let mut matched: u64 = 0;
    let mut mismatched: u64 = 0;
    let mut mismatches: Vec<ExportVerifyMismatch> = Vec::new();

    if let Some(content) = manifest_content {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: Result<serde_json::Value, _> = serde_json::from_str(line);
            let Ok(entry) = entry else { continue };
            let Some(account_id) = entry.get("account_id").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(envelope_id) = entry.get("envelope_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let expected_hash = entry.get("content_hash").and_then(|v| v.as_str());
            checked += 1;

            // The mbox manifest lists one message per line and is checked
            // against the live message; the attachments manifest lists one
            // attachment per line and is checked against the live attachment.
            let actual = match format {
                ExportFormat::Mbox => read_blob_content_hash(account_id, envelope_id).await,
                ExportFormat::Attachments => {
                    read_attachment_content_hash(account_id, envelope_id, expected_hash).await
                }
            };
            match (expected_hash, actual.as_ref()) {
                // Matched when the reconstructed content is byte-identical, or
                // when every detached attachment blob is present. The blob
                // store is keyed by the hash of the *decoded* attachment
                // content while values hold the undecoded raw bytes; when the
                // same decoded content was stored under a different raw
                // encoding (e.g. base64 line folding) reattachment is
                // content-identical but not byte-identical. Only a genuinely
                // absent blob is a defect, mirroring the integrity check.
                (Some(expected), Some((actual_hash, missing)))
                    if expected == actual_hash || missing.is_empty() =>
                {
                    matched += 1
                }
                _ => {
                    mismatched += 1;
                    if mismatches.len() < 100 {
                        mismatches.push(ExportVerifyMismatch {
                            envelope_id: envelope_id.to_string(),
                            message_id: entry
                                .get("message_id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            subject: entry
                                .get("subject")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            expected_hash: expected_hash.map(|s| s.to_string()),
                            actual_hash: actual.as_ref().map(|(h, _)| h.clone()),
                            reason: classify_mismatch(expected_hash, actual.as_ref()).to_string(),
                        });
                    }
                }
            }
            {
                let mut guard = job.lock().unwrap();
                guard.verify_checked = checked;
                guard.verify_matched = matched;
                guard.verify_mismatched = mismatched;
            }
        }
    }

    let artifact_hash_match =
        expected_artifact_hash.as_deref() == Some(actual_artifact_hash.as_str());
    emit(Event::ExportVerified {
        user: job.lock().unwrap().username.clone(),
        export_id: job_id.clone(),
        artifact_hash: Some(actual_artifact_hash.clone()),
        checked,
        matched,
        mismatched,
    });

    let view = ExportVerifyView {
        job_id: job_id.clone(),
        status,
        artifact_name: Some(name),
        expected_artifact_hash,
        actual_artifact_hash: Some(actual_artifact_hash),
        artifact_hash_match,
        checked,
        matched,
        mismatched,
        verified_at: now_ms(),
        mismatches,
    };
    {
        let mut guard = job.lock().unwrap();
        guard.verify_status = VERIFY_STATUS_FINISHED.to_string();
        guard.verify_result = Some(view.clone());
        guard.verify_finished_at = Some(now_ms());
        if let Err(e) = persist_verify_state(&*guard) {
            error!("Failed to persist verify state for {job_id}: {e}");
        }
    }
    Ok(view)
}

/// Persists the current compliance verification state into the job
/// directory ("verify.json") so it survives a restart.
fn persist_verify_state(job: &ExportJob) -> BichonResult<()> {
    let dir = job_dir(&job.job_id);
    let data = serde_json::to_vec_pretty(&verify_progress_view(job)).map_err(|e| {
        raise_error!(
            format!("Failed to serialize verify state: {e}"),
            ErrorCode::InternalError
        )
    })?;
    std::fs::write(dir.join("verify.json"), data).map_err(|e| {
        raise_error!(
            format!("Failed to write verify state: {e}"),
            ErrorCode::InternalError
        )
    })
}

/// Reads the artifact SHA-256 recorded in `manifest.json` at export time.
async fn read_manifest_artifact_hash(dir: &Path) -> Option<String> {
    let path = dir.join("manifest.json");
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value
        .get("artifact_hash")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Recomputes the content hash of an archived message from the blob store.
/// Returns `(hash, missing_attachment_hashes)`: the reconstructed content's
/// hash plus the hashes of any `<<BICHON_DETACH_HASH:...>>` placeholders whose
/// blob is genuinely absent from the archive.
async fn read_blob_content_hash(
    account_id: u64,
    envelope_id: &str,
) -> Option<(String, Vec<String>)> {
    let mut reader = get_reader(account_id, envelope_id.to_string()).await.ok()?;
    let mut raw = Vec::new();
    reader.read_to_end(&mut raw).await.ok()?;
    Some((compute_content_hash(&raw), missing_attachment_hashes(&raw)))
}

/// Attachment-manifest verify unit: re-reads the exported attachment from
/// the archive and re-hashes its content. `None` means the attachment is no
/// longer retrievable — the same shape `read_blob_content_hash` returns for
/// a message whose blob is gone.
async fn read_attachment_content_hash(
    account_id: u64,
    envelope_id: &str,
    content_hash: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let content_hash = content_hash?;
    let mut cursor =
        bichon_core::message::attachment::retrieve_attachment_content(
            account_id,
            envelope_id.to_string(),
            content_hash,
        )
        .ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut cursor, &mut bytes).ok()?;
    Some((compute_content_hash(&bytes), Vec::new()))
}

/// True when `raw` still contains a detached-attachment placeholder.
fn contains_detach_placeholder(raw: &[u8]) -> bool {
    const PREFIX: &[u8] =
        bichon_core::envelope::extractor::DETACH_PLACEHOLDER_PREFIX.as_bytes();
    raw.windows(PREFIX.len()).any(|window| window == PREFIX)
}

/// Returns the hashes of any `<<BICHON_DETACH_HASH:...>>` placeholders still
/// present in `raw` whose blob is genuinely absent from the blob store.
///
/// Placeholders whose blob exists are intentionally not reported: legacy
/// records store raw bytes under the hash of the decoded attachment
/// content, so reattaching them can yield the same decoded content without
/// byte-identical output (e.g. base64 line folding). Only a placeholder
/// whose blob is truly missing is a real defect. Which hash domain a
/// placeholder belongs to is documented on
/// [`bichon_core::envelope::extractor::DETACH_PLACEHOLDER_PREFIX`].
fn missing_attachment_hashes(raw: &[u8]) -> Vec<String> {
    const PREFIX: &[u8] =
        bichon_core::envelope::extractor::DETACH_PLACEHOLDER_PREFIX.as_bytes();
    let mut missing = Vec::new();
    let mut search_cursor = 0;
    while let Some(pos) = raw[search_cursor..]
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)
    {
        let hash_start = search_cursor + pos + PREFIX.len();
        let hash_end = hash_start + 64;
        if hash_end + 2 <= raw.len() && &raw[hash_end..hash_end + 2] == b">>" {
            if let Ok(hash) = std::str::from_utf8(&raw[hash_start..hash_end]) {
                match BLOB_MANAGER.get_attachment(hash) {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => missing.push(hash.to_string()),
                }
            }
            search_cursor = hash_end + 2;
        } else {
            search_cursor = search_cursor + pos + 1;
        }
    }
    missing
}

/// Classifies why a message's recomputed content hash differs from the hash
/// recorded in the export manifest.
fn classify_mismatch(
    expected: Option<&str>,
    actual: Option<&(String, Vec<String>)>,
) -> &'static str {
    match actual {
        None => "blob_missing",
        Some((_, missing)) if !missing.is_empty() => "attachment_missing",
        Some((actual_hash, _)) if expected != Some(actual_hash.as_str()) => "content_changed",
        Some(_) => "content_changed",
    }
}

fn is_cancelled(job: &Arc<Mutex<ExportJob>>) -> bool {
    let guard = job.lock().unwrap();
    guard.cancel.load(Ordering::SeqCst) || guard.status == STATUS_CANCELLED
}

/// Writes one mbox entry (From_ line + metadata header + raw EML) and one
/// manifest line.
async fn export_one(
    mbox: &mut tokio::fs::File,
    manifest: &mut tokio::fs::File,
    envelope: &Envelope,
) -> BichonResult<()> {
    let mut reader = get_reader(envelope.account_id, envelope.id.clone()).await?;
    let mut raw = Vec::new();
    reader.read_to_end(&mut raw).await.map_err(|e| {
        raise_error!(
            format!("Failed to read message {}: {e}", envelope.id),
            ErrorCode::InternalError
        )
    })?;

    // A `<<BICHON_DETACH_HASH:...>>` placeholder still present in the
    // reconstructed content means an attachment blob is missing from the
    // archive. Writing that degraded copy into a compliance artifact would
    // silently corrupt it, so the message is counted as failed instead.
    if contains_detach_placeholder(&raw) {
        return Err(raise_error!(
            format!(
                "Message {} is missing attachment content in the archive; refusing to export a degraded copy.",
                envelope.id
            ),
            ErrorCode::InternalError
        ));
    }

    let date_dt = Utc.timestamp_opt(envelope.date / 1000, 0).unwrap();
    let date_str = date_dt.format("%a %b %e %H:%M:%S %Y").to_string();
    let from_line = format!("From {} {}\n", envelope.from.clone(), date_str);

    let metadata = BichonMetadata {
        account_email: envelope.account_email.clone(),
        mailbox_name: envelope.mailbox_name.clone(),
        tags: envelope.tags.clone(),
    };
    let encoded = base64_encode!(serde_json::to_string(&metadata).map_err(|e| {
        raise_error!(
            format!("Failed to serialize metadata: {e}"),
            ErrorCode::InternalError
        )
    })?);
    let header = format!("X-Bichon-Metadata: {}\r\n", encoded);

    mbox.write_all(from_line.as_bytes()).await.map_err(|e| {
        raise_error!(
            format!("Failed to write mbox entry: {e}"),
            ErrorCode::InternalError
        )
    })?;
    mbox.write_all(header.as_bytes()).await.map_err(|e| {
        raise_error!(
            format!("Failed to write mbox header: {e}"),
            ErrorCode::InternalError
        )
    })?;
    mbox.write_all(&raw).await.map_err(|e| {
        raise_error!(
            format!("Failed to write message body: {e}"),
            ErrorCode::InternalError
        )
    })?;
    mbox.write_all(b"\n\n").await.map_err(|e| {
        raise_error!(
            format!("Failed to finalize mbox entry: {e}"),
            ErrorCode::InternalError
        )
    })?;

    let line = serde_json::json!({
        "account_id": envelope.account_id,
        "envelope_id": envelope.id,
        "message_id": envelope.message_id,
        "subject": envelope.subject,
        "content_hash": envelope.content_hash,
        "size": envelope.size,
        "date": envelope.date,
        "mailbox_id": envelope.mailbox_id,
        "uid": envelope.uid,
    });
    let mut buf = serde_json::to_vec(&line).map_err(|e| {
        raise_error!(
            format!("Failed to serialize manifest line: {e}"),
            ErrorCode::InternalError
        )
    })?;
    buf.push(b'\n');
    manifest.write_all(&buf).await.map_err(|e| {
        raise_error!(
            format!("Failed to write manifest line: {e}"),
            ErrorCode::InternalError
        )
    })?;

    Ok(())
}

async fn write_summary_manifest(
    dir: &Path,
    job_id: &str,
    format: ExportFormat,
    artifact_name: &str,
    artifact_size: u64,
    artifact_hash: Option<&str>,
    username: &str,
    saved_search_id: &str,
    saved_search_name: &str,
    accounts: &[ExportAccount],
    exported: u64,
    failed: u64,
    created_at: i64,
) -> BichonResult<()> {
    let summary = serde_json::json!({
        "job_id": job_id,
        "status": "finished",
        "user": username,
        "saved_search_id": saved_search_id,
        "saved_search_name": saved_search_name,
        "format": format.as_str(),
        "created_at": created_at,
        "finished_at": now_ms(),
        "accounts": accounts,
        "total_emails": exported + failed,
        "total_size": artifact_size,
        "exported": exported,
        "failed": failed,
        "artifact": artifact_name,
        "artifact_size": artifact_size,
        "artifact_hash": artifact_hash,
        "hash_algorithm": "sha256",
    });
    let data = serde_json::to_vec_pretty(&summary).map_err(|e| {
        raise_error!(
            format!("Failed to serialize manifest: {e}"),
            ErrorCode::InternalError
        )
    })?;
    tokio::fs::write(dir.join("manifest.json"), data).await.map_err(|e| {
        raise_error!(
            format!("Failed to write manifest: {e}"),
            ErrorCode::InternalError
        )
    })
}

#[allow(dead_code)]
fn _utc_now_ms() -> i64 {
    utc_now!()
}

/// Persisted form of a finished export, written by `write_summary_manifest`
/// into each job directory. Used to reload export records at server startup.
#[derive(Deserialize)]
struct PersistedExportManifest {
    job_id: String,
    status: String,
    user: String,
    saved_search_id: String,
    saved_search_name: String,
    format: String,
    created_at: i64,
    finished_at: Option<i64>,
    accounts: Vec<ExportAccount>,
    total_emails: u64,
    total_size: u64,
    exported: u64,
    failed: u64,
    artifact: String,
    artifact_size: u64,
    artifact_hash: Option<String>,
}

/// Scans the export root directory at server startup and reloads finished
/// export jobs from their `manifest.json`, so records (and their artifacts)
/// survive a restart. Incomplete or malformed job directories are skipped;
/// the periodic TTL sweep reclaims them later.
pub fn load_persisted_exports() {
    let root = export_root_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let users = match UserModel::list_all() {
        Ok(users) => users,
        Err(e) => {
            warn!("Failed to list users while loading persisted exports: {e}");
            return;
        }
    };
    let mut registry_guard = registry().lock().unwrap();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if registry_guard.contains_key(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Ok(raw) = std::fs::read(dir.join("manifest.json")) else {
            continue;
        };
        let manifest: PersistedExportManifest = match serde_json::from_slice(&raw) {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    "Skipping export dir {}: invalid manifest ({e})",
                    dir.display()
                );
                continue;
            }
        };
        if manifest.status != STATUS_FINISHED {
            continue;
        }
        let Some(user_id) = users
            .iter()
            .find(|u| u.username == manifest.user)
            .map(|u| u.id)
        else {
            warn!(
                "Skipping export {}: user {} no longer exists",
                manifest.job_id, manifest.user
            );
            continue;
        };
        let format = match export_format_from_wire(&manifest.format) {
            Some(format) => format,
            None => {
                warn!(
                    "Skipping export {}: unknown format {}",
                    manifest.job_id, manifest.format
                );
                continue;
            }
        };
        // Restore any persisted compliance verification state so results
        // survive a restart. A `running` state cannot outlive the process, so
        // it is downgraded back to `idle`.
        let persisted_verify = std::fs::read(dir.join("verify.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<ExportVerifyProgressView>(&raw).ok())
            .filter(|v| v.status != VERIFY_STATUS_RUNNING);
        let job = ExportJob {
            job_id: manifest.job_id.clone(),
            user_id,
            username: manifest.user,
            saved_search_id: manifest.saved_search_id,
            saved_search_name: manifest.saved_search_name,
            format,
            status: STATUS_FINISHED.to_string(),
            accounts: manifest.accounts,
            total_emails: manifest.total_emails,
            total_size: manifest.total_size,
            processed: manifest.exported + manifest.failed,
            exported: manifest.exported,
            failed: manifest.failed,
            error: None,
            artifact_name: Some(manifest.artifact),
            artifact_size: manifest.artifact_size,
            artifact_hash: manifest.artifact_hash,
            created_at: manifest.created_at,
            finished_at: manifest.finished_at,
            cancel: Arc::new(AtomicBool::new(false)),
            verify_status: persisted_verify
                .as_ref()
                .map(|v| v.status.clone())
                .unwrap_or_else(|| VERIFY_STATUS_IDLE.to_string()),
            verify_checked: persisted_verify.as_ref().map(|v| v.checked).unwrap_or(0),
            verify_total: persisted_verify.as_ref().map(|v| v.total).unwrap_or(0),
            verify_matched: persisted_verify.as_ref().map(|v| v.matched).unwrap_or(0),
            verify_mismatched: persisted_verify.as_ref().map(|v| v.mismatched).unwrap_or(0),
            verify_error: persisted_verify.as_ref().and_then(|v| v.error.clone()),
            verify_result: persisted_verify.as_ref().and_then(|v| v.result.clone()),
            verify_started_at: persisted_verify.as_ref().and_then(|v| v.started_at),
            verify_finished_at: persisted_verify.as_ref().and_then(|v| v.finished_at),
        };
        info!(
            "Loaded persisted export job {} (user {}, {} exported, {} failed)",
            job.job_id, job.username, job.exported, job.failed
        );
        registry_guard.insert(name, Arc::new(Mutex::new(job)));
    }
}
