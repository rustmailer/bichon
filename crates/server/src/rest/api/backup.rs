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

//! Backup REST API.
//!
//! Management (`run`, `status`, `records`) and the **manifest browser**
//! (`manifests`, detail, audit-delta download) are available with
//! `backup:manage`; browsing additionally requires the backup feature to be
//! enabled. Both are gated on `system:root` **or** `backup:manage` (admins
//! pass automatically), so an existing install needs no role migration.
//!
//! The browser shows restore points straight from the configured target
//! ([`BackupEngine`], design doc §9): every committed manifest with its
//! object composition, plus direct download of the audit delta JSONL.gz —
//! the timestamped compliance export artifact.

use bichon_core::backup::backend::EngineBackend;
use bichon_core::backup::config::{self, BackupConfig, BackupTargetType};
use bichon_core::backup::manager::{self, BackupTrigger, BACKUP_MANAGER};
use bichon_core::backup::manifest::Manifest;
use bichon_core::backup::model::BackupRecord;
use bichon_core::backup::retention::RetentionPolicy;
use bichon_core::backup::schedule;
use bichon_core::error::code::ErrorCode;
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::raise_error;
use bichon_core::users::permissions::Permission;
use poem::http::StatusCode;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Attachment, AttachmentType, Json, Response};
use poem_openapi::OpenApi;

use crate::common::auth::WrappedContext;
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;

pub struct BackupApi;

/// Current backup state + schedule, for the status card in the UI.
#[derive(serde::Serialize, poem_openapi::Object)]
struct BackupStatusView {
    /// Whether the backup feature is enabled.
    enabled: bool,
    /// Whether a run is executing right now.
    running: bool,
    /// Current phase: `idle`, `preflight`, `capture[:<preparer>]`, `upload`,
    /// `finalize`.
    phase: String,
    /// UTC millis the current run started.
    started_at: Option<i64>,
    current_record_id: Option<String>,
    /// UTC millis of the last successful run.
    last_success_at: Option<i64>,
    /// Error of the last failed run, if any.
    last_error: Option<String>,
    /// Manifest id produced by the last successful run.
    last_manifest_id: Option<String>,
    /// Bytes uploaded by the last successful run.
    last_uploaded_bytes: Option<u64>,
    /// Objects newly uploaded by the last successful run.
    last_new_objects: Option<u64>,
    /// Objects skipped (content-addressed dedup) by the last successful run.
    last_skipped_objects: Option<u64>,
    /// Cron expression (UTC) for scheduled backups.
    schedule: String,
    /// RFC 3339 of the next scheduled occurrence (or `None` if invalid).
    next_run_at: Option<String>,
    /// Which backend runs write to (`s3` / `local`), and where it points.
    /// After a target change the history below belongs to the *previous*
    /// target, so this is how the status card answers "where are my backups
    /// going right now?".
    target: String,
}

/// Effective backup configuration for the WebUI form. Secrets never
/// round-trip: credentials are exposed only as `*_set` flags.
///
/// The form covers both target types (design doc §9): an S3-compatible
/// endpoint (endpoint / bucket / region / access key / secret key) or a local
/// directory, plus enabled, schedule and the structured retention policy. The
/// search indices are always part of every backup point (no toggle).
#[derive(serde::Serialize, poem_openapi::Object)]
struct BackupConfigView {
    /// Whether the backup feature is enabled.
    enabled: bool,
    /// Cron expression (UTC) for scheduled backups.
    schedule: String,
    /// Object-store prefix — under the bucket for an S3 target, or under the
    /// local directory (`bichon-backup` by default).
    prefix: String,
    /// Structured retention policy (`keep_last` / `keep_daily` /
    /// `keep_weekly` / `keep_monthly`).
    retention: RetentionPolicy,
    /// Which target the fields below describe: `s3` or `local`.
    target_type: BackupTargetType,
    /// S3 endpoint (`http://…` for MinIO/R2/Wasabi style, empty for AWS).
    s3_endpoint: Option<String>,
    /// S3 region.
    s3_region: Option<String>,
    /// S3 bucket.
    s3_bucket: Option<String>,
    /// Whether an S3 access key is configured (page override or env).
    s3_access_key_set: bool,
    /// Whether an S3 secret key is configured (page override or env).
    s3_secret_key_set: bool,
    /// Absolute path of the local backup directory. Not a secret — the
    /// operator has to see and edit it. Retained while the active target is
    /// S3 so switching back and forth is lossless.
    local_dir: Option<String>,
}

/// WebUI backup-configuration update. `None` leaves a field unchanged; secrets
/// follow the SIEM convention: `""` clears, `"********"` keeps, anything else
/// replaces. `local_dir` is not a secret, so `""` clears it directly.
#[derive(serde::Deserialize, poem_openapi::Object)]
struct BackupConfigUpdate {
    enabled: Option<bool>,
    schedule: Option<String>,
    prefix: Option<String>,
    retention: Option<RetentionPolicy>,
    target_type: Option<BackupTargetType>,
    s3_endpoint: Option<String>,
    s3_region: Option<String>,
    s3_bucket: Option<String>,
    s3_access_key: Option<String>,
    s3_secret_key: Option<String>,
    local_dir: Option<String>,
}

/// One restore point in the manifest browser.
#[derive(serde::Serialize, poem_openapi::Object)]
struct ManifestView {
    id: String,
    /// RFC 3339 (UTC) creation time of the restore point.
    created: String,
    /// `"manual"` or `"schedule"`.
    trigger: String,
    /// Total bytes referenced by this restore point.
    bytes_total: u64,
    /// Total object count referenced by this restore point.
    object_count: usize,
    memdb: Option<ObjectView>,
    segments: Vec<ObjectView>,
    audit: Option<AuditView>,
    integrity: Option<ObjectView>,
    timestamp: Option<ObjectView>,
    /// `instance-id` + license + license-state package (Pro deployments).
    deployment: Option<ObjectView>,
    tantivy: Vec<ObjectView>,
}

/// Result of deleting one restore point.
#[derive(serde::Serialize, poem_openapi::Object)]
struct ManifestDeleteView {
    /// The deleted restore point's id.
    manifest_id: String,
    /// Objects removed from the bucket (only ones no surviving restore point
    /// referenced).
    objects_removed: u64,
    /// Bytes freed from the bucket.
    bytes_reclaimed: u64,
}

/// One referenced object with its content hash + size.
#[derive(serde::Serialize, poem_openapi::Object)]
struct ObjectView {
    /// Content-addressed key (`obj/sha256/<hex>`).
    key: String,
    bytes: u64,
    /// For index objects: which index this is (`envelope` / `attachment`).
    /// `None` for everything else.
    #[oai(default)]
    name: Option<String>,
    /// For blob segments: the segment id within the blob store. `None` for
    /// everything else.
    #[oai(default)]
    segment_id: Option<u32>,
}

/// The audit chain of a restore point: baseline + ordered deltas, with the
/// cursor / cumulative-bytes / base-ratio the Pro status page shows.
#[derive(serde::Serialize, poem_openapi::Object)]
struct AuditView {
    base: Option<ObjectView>,
    deltas: Vec<AuditDeltaView>,
    /// Highest audit seq covered by the chain.
    cursor: i64,
    /// Cumulative delta bytes since this baseline.
    cumulative_delta_bytes: u64,
    /// `cumulative_delta_bytes / base.bytes` — the auto-rebase trigger
    /// compares this against 0.5 (`None` when there is no baseline yet).
    base_ratio: Option<f64>,
}

/// One audit delta: `seq_from < seq <= seq_to`, downloadable as gzip JSONL.
#[derive(serde::Serialize, poem_openapi::Object)]
struct AuditDeltaView {
    seq_from: i64,
    seq_to: i64,
    #[serde(flatten)]
    object: ObjectView,
}

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Backup")]
impl BackupApi {
    /// Triggers a backup run immediately (as if the schedule had fired).
    /// Returns `202 Accepted` when a run was started, `429` when one is
    /// already in progress, and `400` when backups are disabled.
    #[oai(
        path = "/backup/run",
        method = "post",
        operation_id = "trigger_backup"
    )]
    async fn trigger_backup(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Response<Json<BackupStatusView>>> {
        require_backup_access(&context)?;
        let _ = manager::request_run(BackupTrigger::Manual)?;
        Ok(Response::new(Json(status_view())).status(StatusCode::ACCEPTED))
    }

    /// Current backup status (running/phase/last outcome + schedule).
    #[oai(
        path = "/backup/status",
        method = "get",
        operation_id = "get_backup_status"
    )]
    async fn get_backup_status(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<BackupStatusView>> {
        require_backup_access(&context)?;
        Ok(Json(status_view()))
    }

    /// The most recent backup run records, newest first.
    #[oai(
        path = "/backup/records",
        method = "get",
        operation_id = "list_backup_records"
    )]
    async fn list_backup_records(
        &self,
        context: WrappedContext,
        limit: Query<Option<usize>>,
    ) -> ApiResult<Json<Vec<BackupRecord>>> {
        require_backup_access(&context)?;
        let limit = limit.0.unwrap_or(20).min(200);
        Ok(Json(BackupRecord::list_recent(limit)))
    }

    /// Every committed restore point (manifest), newest first.
    #[oai(
        path = "/backup/manifests",
        method = "get",
        operation_id = "list_backup_manifests"
    )]
    async fn list_backup_manifests(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<ManifestView>>> {
        require_backup_access(&context)?;
        require_backup_enabled()?;
        let engine = EngineBackend::browser_engine()?;
        let manifests = engine.list_restore_points().await?;
        Ok(Json(manifests.into_iter().map(manifest_view).collect()))
    }

    /// One restore point's full object composition.
    #[oai(
        path = "/backup/manifests/:id",
        method = "get",
        operation_id = "get_backup_manifest"
    )]
    async fn get_backup_manifest(
        &self,
        context: WrappedContext,
        id: Path<String>,
    ) -> ApiResult<Json<ManifestView>> {
        require_backup_access(&context)?;
        require_backup_enabled()?;
        let engine = EngineBackend::browser_engine()?;
        let manifest = engine
            .load_by_id(&id.0)
            .await?
            .ok_or_else(|| {
                raise_error!(
                    format!("restore point {} not found", id.0),
                    ErrorCode::ResourceNotFound
                )
            })?;
        Ok(Json(manifest_view(manifest)))
    }

    /// Delete one restore point: its manifest is removed and every object
    /// only it referenced is reclaimed from the bucket (shared objects
    /// survive). Rejected with `429` while a backup run is in progress.
    /// Works even when backups are disabled — freeing the bucket is exactly
    /// what an admin does before reconfiguring or retiring the target.
    #[oai(
        path = "/backup/manifests/:id",
        method = "delete",
        operation_id = "delete_backup_manifest"
    )]
    async fn delete_backup_manifest(
        &self,
        context: WrappedContext,
        id: Path<String>,
    ) -> ApiResult<Json<ManifestDeleteView>> {
        require_backup_access(&context)?;
        // The run lock: a concurrent run uploads objects and commits its
        // manifest while the reclaim lists, and uncommitted objects would be
        // seen as garbage. `begin_run` fails fast with 429 instead.
        let _run_lock = manager::begin_run()?;
        let engine = EngineBackend::browser_engine()?;
        let stats = engine.delete_restore_point(&id.0).await?;
        Ok(Json(ManifestDeleteView {
            manifest_id: id.0,
            objects_removed: stats.objects_removed,
            bytes_reclaimed: stats.bytes_reclaimed,
        }))
    }

    /// Download one audit delta as gzip JSONL — the timestamped compliance
    /// export artifact. Covers rows `seq_from < seq <= seq_to`.
    #[oai(
        path = "/backup/manifests/:id/audit/:seq_from/:seq_to",
        method = "get",
        operation_id = "download_audit_delta"
    )]
    async fn download_audit_delta(
        &self,
        context: WrappedContext,
        id: Path<String>,
        seq_from: Path<i64>,
        seq_to: Path<i64>,
    ) -> ApiResult<Attachment<Body>> {
        require_backup_access(&context)?;
        require_backup_enabled()?;
        let engine = EngineBackend::browser_engine()?;
        let manifest = engine
            .load_by_id(&id.0)
            .await?
            .ok_or_else(|| {
                raise_error!(
                    format!("restore point {} not found", id.0),
                    ErrorCode::ResourceNotFound
                )
            })?;
        let audit = manifest.objects.audit.ok_or_else(|| {
            raise_error!(
                format!("restore point {} carries no audit chain", id.0),
                ErrorCode::ResourceNotFound
            )
        })?;
        let delta = audit
            .deltas
            .iter()
            .find(|d| d.seq_from == seq_from.0 && d.seq_to == seq_to.0)
            .ok_or_else(|| {
                raise_error!(
                    format!(
                        "restore point {} has no delta {}-{}",
                        id.0, seq_from.0, seq_to.0
                    ),
                    ErrorCode::ResourceNotFound
                )
            })?;
        let bytes = engine
            .read_object(&delta.obj.key, &delta.obj.sha256, delta.obj.bytes)
            .await?;
        let filename = format!("audit-{}-{}.jsonl.gz", seq_from.0, seq_to.0);
        let attachment = Attachment::new(Body::from(bytes))
            .attachment_type(AttachmentType::Attachment)
            .filename(filename);
        Ok(attachment)
    }

    /// Effective backup configuration (S3 target, schedule, retention;
    /// credentials as `*_set` flags only).
    #[oai(
        path = "/backup/config",
        method = "get",
        operation_id = "get_backup_config"
    )]
    async fn get_backup_config(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<BackupConfigView>> {
        require_backup_access(&context)?;
        Ok(Json(backup_config_view()))
    }

    /// Update backup configuration. `None` fields are left untouched; secrets
    /// use the `""`=clear / `"********"`=keep / other=replace convention.
    #[oai(
        path = "/backup/config",
        method = "post",
        operation_id = "update_backup_config"
    )]
    async fn update_backup_config(
        &self,
        context: WrappedContext,
        payload: Json<BackupConfigUpdate>,
    ) -> ApiResult<Json<BackupConfigView>> {
        require_backup_access(&context)?;
        apply_backup_config(payload.0)?;
        emit(Event::BackupConfigUpdated {
            user: context.user.username.clone(),
        });
        Ok(Json(backup_config_view()))
    }
}

/// Both `system:root` and `backup:manage` grant access, so existing installs
/// (whose admins hold `system:root`) need no role migration; a delegated
/// operator can be given just `backup:manage`.
fn require_backup_access(context: &WrappedContext) -> bichon_core::error::BichonResult<()> {
    if context.has_permission(None, Permission::ROOT)
        || context.has_permission(None, Permission::BACKUP_MANAGE)
    {
        Ok(())
    } else {
        Err(raise_error!(
            "Access Denied: Missing permission 'backup:manage'".into(),
            ErrorCode::Forbidden
        ))
    }
}

/// Browser endpoints are only meaningful when backups are on.
fn require_backup_enabled() -> bichon_core::error::BichonResult<()> {
    if config::enabled() {
        Ok(())
    } else {
        Err(raise_error!(
            "backups are disabled (enable them in the WebUI Backup page)"
                .into(),
            ErrorCode::MissingConfiguration
        ))
    }
}

fn status_view() -> BackupStatusView {
    let st = BACKUP_MANAGER.state();
    let (last_manifest_id, last_uploaded, last_new, last_skipped) =
        last_success_summary();
    BackupStatusView {
        enabled: config::enabled(),
        running: st.running,
        phase: st.phase,
        started_at: st.started_at,
        current_record_id: st.current_record_id,
        last_success_at: st.last_success_at,
        last_error: st.last_error,
        last_manifest_id,
        last_uploaded_bytes: last_uploaded,
        last_new_objects: last_new,
        last_skipped_objects: last_skipped,
        schedule: config::schedule(),
        next_run_at: schedule::next_run(&config::schedule()),
        target: config::destination(),
    }
}

/// Summary fields of the most recent successful run **against the currently
/// configured destination**.
///
/// Scoped on purpose: the run history is one global list, but a restore point
/// only exists on the target that wrote it. Without the scope, changing the
/// target would leave the status card reporting a manifest id that the
/// manifest browser — which reads the current target only — cannot find.
fn last_success_summary() -> (Option<String>, Option<u64>, Option<u64>, Option<u64>) {
    match BackupRecord::last_success_for(&config::destination())
        .and_then(|r| r.summary)
    {
        Some(s) => (
            Some(s.manifest_id),
            Some(s.uploaded_bytes),
            Some(s.new_objects),
            Some(s.skipped_objects),
        ),
        None => (None, None, None, None),
    }
}

/// How the WebUI signals "leave the stored secret unchanged".
const SECRET_KEEP: &str = "********";

fn backup_config_view() -> BackupConfigView {
    BackupConfigView {
        enabled: config::enabled(),
        schedule: config::schedule(),
        prefix: config::prefix(),
        retention: config::retention_policy_struct(),
        target_type: config::target_type(),
        s3_endpoint: config::s3_endpoint(),
        s3_region: config::s3_region(),
        s3_bucket: config::s3_bucket(),
        s3_access_key_set: config::s3_access_key_set(),
        s3_secret_key_set: config::s3_secret_key_set(),
        local_dir: config::local_dir(),
    }
}

/// Merge a WebUI update into the effective config and persist it as a single
/// document. `None` leaves a field unchanged; the two secrets follow the
/// `""`=clear / `"********"`=keep / other=replace convention.
fn apply_backup_config(update: BackupConfigUpdate) -> bichon_core::error::BichonResult<()> {
    let mut cfg = config::load();
    if let Some(v) = update.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = update.schedule {
        cfg.schedule = v;
    }
    if let Some(v) = update.prefix {
        cfg.prefix = v;
    }
    if let Some(v) = update.retention {
        cfg.retention = v;
    }
    if let Some(v) = update.target_type {
        cfg.target_type = v;
    }
    if let Some(v) = update.s3_endpoint {
        cfg.s3_endpoint = Some(v);
    }
    if let Some(v) = update.s3_region {
        cfg.s3_region = Some(v);
    }
    if let Some(v) = update.s3_bucket {
        cfg.s3_bucket = Some(v);
    }
    // Unlike the S3 fields, an empty `local_dir` must mean "unset" rather than
    // `Some("")`: `None` already means "unchanged", so without this a path
    // could never be cleared and switching s3 → local → s3 would leave the
    // stale path behind. `normalize` below performs that unset, along with
    // trimming the paths and the schedule expression.
    if let Some(v) = update.local_dir {
        cfg.local_dir = Some(v);
    }
    apply_secret_field(&mut cfg.s3_access_key, update.s3_access_key);
    apply_secret_field(&mut cfg.s3_secret_key, update.s3_secret_key);
    // Before the target guard rather than only before the save: the guard has
    // to evaluate the same path that ends up stored, or `local_dir =
    // " /data "` would be checked as `/data` and written as `" /data "`.
    config::normalize(&mut cfg);
    validate_effective_target(&cfg)?;
    config::save(&cfg)
}

/// Secret-field semantics: `None` untouched, `"********"` keep, `""` clear
/// (unset), anything else replaces.
fn apply_secret_field(target: &mut Option<String>, value: Option<String>) {
    match value {
        None => {}
        Some(s) if s == SECRET_KEEP => {}
        Some(s) if s.is_empty() => *target = None,
        Some(s) => *target = Some(s),
    }
}

/// Reject an obviously unusable target up front, so the form gives a clear
/// error instead of the run failing later.
///
/// The shape checks here mirror `config::validate` (which `config::save` runs
/// immediately afterwards) — they must agree, or the same input would produce
/// two different errors. What this adds is the data-root guard, which
/// `validate` deliberately leaves out because it needs server context.
fn validate_effective_target(cfg: &BackupConfig) -> bichon_core::error::BichonResult<()> {
    match cfg.target_type {
        BackupTargetType::S3 => {
            if cfg.s3_bucket.as_deref().unwrap_or("").is_empty() {
                return Err(raise_error!(
                    "an s3 target requires a bucket".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }
        BackupTargetType::Local => {
            config::validate_local_target(
                cfg,
                Some(&bichon_core::settings::dir::DATA_DIR_MANAGER.root_dir),
            )?;
        }
    }
    Ok(())
}

/// Flatten a manifest into the browser view.
fn manifest_view(m: Manifest) -> ManifestView {
    let objects = &m.objects;
    ManifestView {
        bytes_total: m.object_refs().iter().map(|(_, b)| b).sum(),
        object_count: m.object_refs().len(),
        id: m.id,
        created: m.created,
        trigger: m.trigger,
        memdb: objects.memdb.as_ref().map(object_view),
        segments: objects
            .blob_segments
            .iter()
            .map(|s| ObjectView {
                segment_id: Some(s.seg_id),
                ..object_view(&s.obj)
            })
            .collect(),
        audit: objects.audit.as_ref().map(audit_view),
        integrity: objects.integrity.as_ref().map(object_view),
        timestamp: objects.timestamp.as_ref().map(object_view),
        deployment: objects.deployment.as_ref().map(object_view),
        tantivy: objects
            .tantivy
            .iter()
            .map(|t| ObjectView {
                name: Some(t.name.clone()),
                ..object_view(&t.obj)
            })
            .collect(),
    }
}

fn object_view(o: &bichon_core::backup::manifest::ObjectRef) -> ObjectView {
    ObjectView {
        key: o.key.clone(),
        bytes: o.bytes,
        name: None,
        segment_id: None,
    }
}

fn audit_view(a: &bichon_core::backup::manifest::AuditObjects) -> AuditView {
    let base_bytes = a.base.bytes;
    AuditView {
        base: Some(object_view(&a.base)),
        deltas: a
            .deltas
            .iter()
            .map(|d| AuditDeltaView {
                seq_from: d.seq_from,
                seq_to: d.seq_to,
                object: object_view(&d.obj),
            })
            .collect(),
        cursor: a.cursor,
        cumulative_delta_bytes: a.cumulative_delta_bytes,
        base_ratio: if base_bytes > 0 {
            Some(a.cumulative_delta_bytes as f64 / base_bytes as f64)
        } else {
            None
        },
    }
}
