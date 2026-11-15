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

//! The backup state machine.
//!
//! [`BACKUP_MANAGER`] runs at most one backup at a time. A run walks through
//!
//! ```text
//! preflight -> capture -> upload -> finalize
//! ```
//!
//! * **preflight** — verify the backend target (S3 reachable and writable)
//!   *before* pausing anything, so a misconfigured target never stalls the
//!   server.
//! * **capture** — pause [`WRITE_GATE`], drain every in-flight / queued write,
//!   persist a `Running` record, then call each registered [`BackupPreparer`]
//!   in order. Each delivers its exact products as [`Artifact`]s; nothing on
//!   disk mutates for the rest of the window.
//! * **upload** — reopen the write gate *before* handing the artifacts to the
//!   backend (R2): the upload may take minutes and must not block archiving.
//!   The backend content-addresses each artifact, commits a manifest and
//!   applies retention. Blob GC stays paused until `finalize`.
//! * **finalize** — always: run every preparer's `finalize` (restores paused
//!   GC, cleans staging) and reopen the write gate, unconditionally, even on
//!   failure or panic. The gate-resume guarantee is implemented by
//!   [`RunScopedGuard`], which runs on every exit path.
//!
//! [`request_run`] fires a run in a background task and returns immediately;
//! [`BackupManager::run_with_backend`] runs the machine synchronously and is
//! what the tests drive with an injected backend.

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::backup::artifact::Artifact;
use crate::backup::backend::{AuditStamp, BackupBackend, BackupSummary};
use crate::backup::config;
use crate::backup::errors::{self, BackupErrorKind};
use crate::backup::gate::WRITE_GATE;
use crate::backup::model::{BackupRecord, BackupRunStatus};
use crate::backup::prepare::{self, PreparerContext};
use crate::database::manager::DB_MANAGER;
use crate::database::MemDbModel;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use crate::settings::dir::DATA_DIR_MANAGER;
use crate::store::blob::BLOB_MANAGER;
use crate::utc_now;
use tracing::{debug, error, info};

pub static BACKUP_MANAGER: LazyLock<BackupManager> = LazyLock::new(BackupManager::new);

/// Wall-clock cap for the preflight round-trip.
///
/// This is a **backstop, not the primary timeout**: `object_store` already
/// bounds every individual HTTP request (30 s) and gives up after ~3 minutes
/// of retries, so a working target never gets near this. It exists for the
/// case those bounds do not cover — a future that never resolves at all (a
/// stalled TLS/socket wait, a proxy that accepts the connection and then goes
/// silent) — which would otherwise park the run in `preflight` forever. When
/// it fires, the request future is dropped: the connection is closed and
/// nothing keeps running in the background.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Same backstop for reading the previous restore point (`meta/LATEST`) that
/// runs as part of the preflight phase.
const MANIFEST_READ_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Timeouts applied to the two bounded steps that run before the capture
/// window opens. Production always uses [`Default`] (the constants above);
/// tests pass much shorter values so a stuck future can be exercised in
/// milliseconds instead of minutes.
#[derive(Clone, Copy)]
struct RunTimeouts {
    preflight: Duration,
    manifest: Duration,
}

impl Default for RunTimeouts {
    fn default() -> Self {
        Self {
            preflight: PREFLIGHT_TIMEOUT,
            manifest: MANIFEST_READ_TIMEOUT,
        }
    }
}

/// Snapshot of the manager's public status, for the status API / UI.
#[derive(Clone, Debug, Default)]
pub struct ManagerState {
    pub running: bool,
    /// Current phase: `idle`, `preflight`, `capture[:<preparer>]`, `upload`,
    /// `finalize`.
    pub phase: String,
    /// UTC millis the current run started (`None` when idle).
    pub started_at: Option<i64>,
    pub current_record_id: Option<String>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_snapshot_id: Option<String>,
    /// Non-fatal warnings from the most recent run (e.g. a database growing
    /// too large). Surfaced in the notifications bell / backup status.
    pub last_warnings: Vec<String>,
}

pub struct BackupManager {
    state: Mutex<ManagerState>,
    /// Serializes runs. [`request_run`] takes it with `try_lock` (so a busy
    /// backup is reported synchronously) and holds it for the whole run.
    run_lock: tokio::sync::Mutex<()>,
}

impl BackupManager {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ManagerState::default()),
            run_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn state(&self) -> ManagerState {
        self.state
            .lock()
            .map(|st| st.clone())
            .unwrap_or_default()
    }

    /// Run the state machine synchronously, serialized by `run_lock`. Tests
    /// drive this directly with a fake [`BackupBackend`]; the public
    /// [`request_run`] wraps it in a background task.
    #[allow(dead_code)] // used only from #[cfg(test)]
    pub(crate) async fn run_with_backend(&self, trigger: &str, backend: Box<dyn BackupBackend>) {
        let _lock = self.run_lock.lock().await;
        self.run_locked(trigger, backend).await;
    }

    /// Test-only entry point with shortened timeouts, so a stuck backend
    /// future fails in milliseconds instead of minutes.
    #[cfg(test)]
    async fn run_with_timeouts(
        &self,
        trigger: &str,
        backend: Box<dyn BackupBackend>,
        timeouts: RunTimeouts,
    ) {
        let _lock = self.run_lock.lock().await;
        self.run_locked_with(trigger, backend, timeouts).await;
    }

    async fn run_locked(&self, trigger: &str, backend: Box<dyn BackupBackend>) {
        self.run_locked_with(trigger, backend, RunTimeouts::default())
            .await;
    }

    async fn run_locked_with(
        &self,
        trigger: &str,
        backend: Box<dyn BackupBackend>,
        timeouts: RunTimeouts,
    ) {
        let started_at = utc_now!();
        self.patch_state(|s| {
            s.running = true;
            s.phase = "preflight".to_string();
            s.started_at = Some(started_at);
            // Each run starts clean: warnings from the previous run must not
            // linger on the status view.
            s.last_warnings.clear();
        });
        // If the run task is killed (panic, task abort) before `finish()`,
        // this guard clears the in-memory status on the way out. Without it a
        // dead task leaves `running = true` forever: the status card keeps
        // reporting a run that no longer exists and the UI keeps "Run now"
        // disabled, with no way back except a restart. It is installed before
        // anything else can fail, so even the log line below is covered.
        // Resolved once here so nothing in the guard's Drop path has to read
        // the configuration while unwinding.
        let destination = config::destination();
        let _state_guard = RunStateGuard {
            manager: self,
            trigger,
            target: &destination,
            started_at,
        };
        info!("backup: run ({trigger}) started — target: {destination}");

        // ── Phase 0: preflight ─────────────────────────────────────────────
        // Verify the backend target before pausing writes so a broken target
        // never leaves the server quiesced; then load the previous restore
        // point for contributors that need chain state (the audit cursor).
        // Both steps are bounded by a wall-clock cap so a stalled connection
        // cannot park the run here indefinitely; dropping the timed-out
        // future also closes the underlying connection.
        info!("backup: preflight started");
        let preflight = match tokio::time::timeout(timeouts.preflight, backend.preflight()).await {
            Ok(result) => result,
            Err(_) => Err(raise_error!(
                format!(
                    "backup: preflight did not finish within {:?} — the target is \
                     unreachable or the connection stalled. No write window was opened.",
                    timeouts.preflight
                ),
                ErrorCode::InternalError
            )),
        };
        if let Err(e) = preflight {
            error!("backup: pre-flight failed: {e:?}");
            self.finish(trigger, started_at, "preflight", None, Some(e));
            return;
        }
        info!("backup: preflight succeeded");
        let previous_manifest =
            match tokio::time::timeout(timeouts.manifest, backend.previous_manifest()).await {
                Ok(result) => result,
                Err(_) => Err(raise_error!(
                    format!(
                        "backup: reading the previous restore point did not finish within \
                         {:?} — the target is unreachable or the connection stalled.",
                        timeouts.manifest
                    ),
                    ErrorCode::InternalError
                )),
            };
        let previous_manifest = match previous_manifest {
            Ok(pm) => pm,
            Err(e) => {
                error!("backup: cannot load the previous restore point: {e:?}");
                self.finish(trigger, started_at, "preflight", None, Some(e));
                return;
            }
        };

        let ctx = PreparerContext {
            root_dir: DATA_DIR_MANAGER.root_dir.clone(),
            staging_dir: DATA_DIR_MANAGER.backup_staging_dir.clone(),
            previous_manifest,
        };
        // The safety net. Runs on *every* exit path — normal return, early
        // error return, and panic — finalizing preparers and reopening the
        // write gate. This is what makes the "unconditional resume" invariant
        // hold.
        let _safety = RunScopedGuard {
            ctx: ctx.clone(),
        };

        // ── Phase 1: capture ───────────────────────────────────────────────
        self.set_phase("capture");
        info!("backup: capture started (writes will pause briefly)");
        WRITE_GATE.pause();
        // In-flight async writes (extractors, tantivy deletes) drain first;
        // then the blob queue that those writes fed.
        WRITE_GATE.drain().await;
        BLOB_MANAGER.drain().await;

        let record_id = uuid::Uuid::new_v4().to_string();
        self.patch_state(|s| s.current_record_id = Some(record_id.clone()));
        let running_record = BackupRecord {
            id: record_id.clone(),
            trigger: trigger.to_string(),
            started_at,
            finished_at: None,
            status: BackupRunStatus::Running,
            phase: "capture".to_string(),
            error: None,
            snapshot_id: None,
            summary: None,
            warnings: Vec::new(),
            target: Some(crate::backup::config::destination()),
        };
        // Written directly (bypassing the gated helpers — we are inside the
        // window). It lands in the memdb snapshot taken by the memdb preparer,
        // so a restored archive shows the run that produced it.
        write_record(&running_record);

        let mut failure: Option<crate::error::BichonError> = None;
        // Phase the failure happened in. Kept separately from `phase` so the
        // run record and the error log name where it went wrong instead of
        // the bookkeeping phase (`finalize`) the run ends in.
        let mut failure_phase: Option<String> = None;
        let mut artifacts: Vec<Artifact> = Vec::new();
        for p in prepare::preparers() {
            self.set_phase(&format!("capture:{}", p.name()));
            match p.prepare(&ctx).await {
                Ok(mut delivered) => artifacts.append(&mut delivered),
                Err(e) => {
                    error!("backup: preparer '{}' failed: {e:?}", p.name());
                    failure_phase = Some(format!("capture:{}", p.name()));
                    failure = Some(e);
                    break;
                }
            }
        }
        // Non-fatal warnings recorded by preparers ride along into the run
        // record and the status view (notifications bell / backup page).
        self.patch_state(|s| s.last_warnings = prepare::take_warnings());

        // ── Phase 2: upload ───────────────────────────────────────────────
        // The write gate reopens BEFORE anything uploads (R2): capture is
        // bounded by the 30 s hard cap, but the upload may take minutes and
        // must not block archiving. Blob GC stays paused until finalize, so
        // the segment artifacts handed over stay immutable on disk.
        let mut summary: Option<BackupSummary> = None;
        if failure.is_none() {
            self.set_phase("upload");
            info!("backup: upload started");
            WRITE_GATE.resume();
            // Upload-phase contributor delivery (gate already open, R2):
            // contributors may do O(data) work here — e.g. the Pro audit
            // contributor exporting `seq > cursor` through a WAL read
            // transaction. Their artifacts join the run's batch; audit facts
            // are forwarded to the backend verbatim.
            let mut audit_stamp = AuditStamp::default();
            for p in prepare::preparers() {
                self.set_phase(&format!("upload:{}", p.name()));
                match p.upload(&ctx).await {
                    Ok(delivery) => {
                        artifacts.extend(delivery.artifacts);
                        if let Some(c) = delivery.audit_cursor {
                            audit_stamp.cursor = Some(audit_stamp.cursor.map_or(c, |p| p.max(c)));
                        }
                        if let Some(b) = delivery.audit_cumulative_delta_bytes {
                            audit_stamp.cumulative_delta_bytes = Some(b);
                        }
                    }
                    Err(e) => {
                        error!("backup: upload-phase contributor '{}' failed: {e:?}", p.name());
                        failure_phase = Some(format!("upload:{}", p.name()));
                        failure = Some(e);
                        break;
                    }
                }
            }
            if failure.is_none() {
                match backend
                    .upload(&artifacts, trigger, &config::retention_policy_struct(), &audit_stamp)
                    .await
                {
                    Ok(s) => {
                        let snapshot_id = s.manifest_id.clone();
                        self.patch_state(|st| st.last_snapshot_id = Some(snapshot_id));
                        summary = Some(s);
                    }
                    Err(e) => {
                        error!("backup: upload failed: {e:?}");
                        failure_phase = Some("upload".to_string());
                        failure = Some(e);
                    }
                }
            }
        }

        // ── Phase 3: finalize ──────────────────────────────────────────────
        // Preparer finalize + gate resume run in the scoped guard on scope
        // exit; here we only persist the outcome.
        // A failure records the phase it happened in; a successful run is
        // finalizing.
        let finished_phase = failure_phase.as_deref().unwrap_or("finalize");
        self.set_phase("finalize");
        self.finish(trigger, started_at, finished_phase, summary, failure);
    }

    /// Persist the final record and update the public state. The gate is
    /// reopened by [`RunScopedGuard`] on the way out.
    fn finish(
        &self,
        trigger: &str,
        started_at: i64,
        phase: &str,
        summary: Option<BackupSummary>,
        failure: Option<crate::error::BichonError>,
    ) {
        let finished_at = utc_now!();
        let status = if failure.is_none() {
            BackupRunStatus::Success
        } else {
            BackupRunStatus::Failed
        };
        let error_text = failure.as_ref().map(|e| e.to_string());
        let snapshot_id = summary.as_ref().map(|s| s.manifest_id.clone());
        let warnings = self.state().last_warnings;

        let record = BackupRecord {
            id: self
                .state()
                .current_record_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            trigger: trigger.to_string(),
            started_at,
            finished_at: Some(finished_at),
            status,
            phase: phase.to_string(),
            error: error_text.clone(),
            snapshot_id: snapshot_id.clone(),
            summary,
            warnings,
            target: Some(crate::backup::config::destination()),
        };
        write_record(&record);
        // Failures land in the rolling error log as well as the run history:
        // the history rolls by *runs* (mostly successful ones), while the
        // error log guarantees the last N failures stay visible.
        if let Some(message) = &error_text {
            errors::record(
                BackupErrorKind::Failed,
                trigger,
                Some(&record.id),
                phase,
                message,
                record.target.as_deref(),
            );
        }

        self.patch_state(|s| {
            s.running = false;
            s.phase = "idle".to_string();
            s.started_at = None;
            s.current_record_id = None;
            if status == BackupRunStatus::Success {
                s.last_success_at = Some(finished_at);
                s.last_error = None;
            } else {
                s.last_error =
                    Some(error_text.unwrap_or_else(|| "backup failed".to_string()));
            }
            if let Some(sid) = snapshot_id {
                s.last_snapshot_id = Some(sid);
            }
        });

        let elapsed = finished_at - started_at;
        crate::metrics::record_backup_run(status == BackupRunStatus::Success, elapsed);
        match status {
            BackupRunStatus::Success => {
                info!("backup: run ({trigger}) succeeded in {elapsed}ms");
            }
            _ => {
                error!("backup: run ({trigger}) failed after {elapsed}ms");
            }
        }
    }

    fn set_phase(&self, phase: &str) {
        self.patch_state(|s| s.phase = phase.to_string());
    }

    fn patch_state(&self, f: impl FnOnce(&mut ManagerState)) {
        if let Ok(mut st) = self.state.lock() {
            f(&mut st);
        }
    }
}

/// Clears the in-memory run status if the run ends without calling
/// [`BackupManager::finish`] — i.e. the background task panicked or was
/// aborted. `finish()` clears `started_at`, which makes this guard a no-op on
/// every normal exit path (success and recorded failure alike); it only acts
/// while the state still describes the run it was created for.
///
/// Without it a single panicking task leaves `running = true` (and the phase
/// it died in) forever: the status card reports a run that no longer exists
/// and the WebUI keeps "Run now" disabled until the server restarts.
struct RunStateGuard<'a> {
    manager: &'a BackupManager,
    /// What started the run, recorded in the error log.
    trigger: &'a str,
    /// Resolved at run start so the Drop path never has to read the config.
    target: &'a str,
    started_at: i64,
}

/// Message recorded when a run task dies without reporting (`finish` never
/// ran). Shared by the status view and the rolling error log.
const ABNORMAL_EXIT: &str =
    "the run ended unexpectedly (panic or task abort) — check the server log";

impl Drop for RunStateGuard<'_> {
    fn drop(&mut self) {
        // Recover a poisoned lock rather than ignoring it: the panic we are
        // unwinding from must still leave a usable status view behind.
        let mut st = match self.manager.state.lock() {
            Ok(st) => st,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !st.running || st.started_at != Some(self.started_at) {
            return;
        }
        let phase = st.phase.clone();
        st.running = false;
        st.phase = "idle".to_string();
        st.started_at = None;
        st.current_record_id = None;
        st.last_error = Some(ABNORMAL_EXIT.to_string());
        // Release the state lock before logging (the panic we are unwinding
        // from may have poisoned or contended it).
        drop(st);

        // Persist the anomaly for the WebUI's error log. This runs while
        // unwinding a panic, so a failure here must never escalate into a
        // second panic (which would abort the process).
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            errors::record(
                BackupErrorKind::AbruptExit,
                self.trigger,
                None,
                &phase,
                ABNORMAL_EXIT,
                Some(self.target),
            );
        }));
        error!("backup: run ended unexpectedly in phase '{phase}' — status cleared");
    }
}

/// Try to begin a run. Succeeds only when no other run is in progress; the
/// returned guard *is* the run lock and must be held until the run finishes.
/// `request_run` moves it into a background task; tests use it directly.
pub fn begin_run() -> BichonResult<tokio::sync::MutexGuard<'static, ()>> {
    BACKUP_MANAGER
        .run_lock
        .try_lock()
        .map_err(|_| {
            raise_error!(
                "a backup is already in progress".to_string(),
                ErrorCode::TooManyRequest
            )
        })
}

/// What caused a backup run. Persisted verbatim on every run — written into
/// the [`BackupRecord`] and the manifest uploaded with the backup, then shown
/// back to the user by restore. The stored label is the stable string from
/// [`BackupTrigger::as_str`], which is free-form so old backups stay readable
/// and new trigger sources need no storage migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupTrigger {
    /// Fired by the cron scheduler tick.
    Schedule,
    /// Fired by the user via the `/backup/run` endpoint.
    Manual,
}

impl BackupTrigger {
    /// Stable label persisted in the record and manifest.
    pub fn as_str(&self) -> &'static str {
        match self {
            BackupTrigger::Schedule => "schedule",
            BackupTrigger::Manual => "manual",
        }
    }
}

impl std::fmt::Display for BackupTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Fire a backup run in a background task. Returns immediately; errors only
/// when backups are disabled, the target is not configured, or a run is
/// already in progress. The write window opens shortly after and lasts for
/// the (bounded) capture; the upload runs with the gate open (R2).
///
/// The backend is moved into the task, so the S3 client and its connection
/// pool are dropped when the run ends — including the timeout and panic
/// paths. Nothing outlives the run.
pub fn request_run(trigger: BackupTrigger) -> BichonResult<()> {
    if !config::enabled() {
        return Err(raise_error!(
            "backups are disabled (enable them in the WebUI Backup page)".to_string(),
            ErrorCode::MissingConfiguration
        ));
    }
    // Build the backend before taking the run lock so a missing/unreachable
    // target fails the request synchronously (R11) instead of in the
    // background task.
    let backend: Box<dyn BackupBackend> = Box::new(crate::backup::backend::EngineBackend::from_settings()?);
    let mgr: &'static BackupManager = &*BACKUP_MANAGER;
    let guard = begin_run()?;
    info!("backup: run requested (trigger: {trigger})");
    let trigger = trigger.as_str().to_string();
    tokio::spawn(async move {
        let _lock = guard;
        mgr.run_locked(&trigger, backend).await;
    });
    Ok(())
}

/// Finalizes preparers and reopens the write gate on every exit path of
/// [`BackupManager::run_locked`] — including panics — so the server can never
/// be left quiesced.
struct RunScopedGuard {
    ctx: PreparerContext,
}

impl Drop for RunScopedGuard {
    fn drop(&mut self) {
        // Finalize every preparer first (best-effort; a panicking preparer
        // must not prevent the gate from reopening)...
        for p in prepare::preparers() {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.finalize(&self.ctx)));
            if let Err(panic) = result {
                error!("backup: preparer '{}' finalize panicked: {panic:?}", p.name());
            }
        }
        // ...then unconditionally reopen the write gate (idempotent, so the
        // resume that already happened before upload is a no-op here).
        WRITE_GATE.resume();
    }
}

/// Persist a backup record directly on the memdb, bypassing the gated write
/// helpers: the `Running` record is written inside the capture window (where
/// those helpers reject writes by design) and the final record after it, from
/// the open-gate finalize phase.
fn write_record(record: &BackupRecord) {
    // `upsert`, not `insert`: the final record replaces the `Running` record
    // written at the start of the same run (same id), and `insert` would
    // reject the duplicate key.
    let result = DB_MANAGER
        .db()
        .collection(BackupRecord::collection())
        .upsert(record.key(), record);
    if let Err(e) = result {
        error!("backup: failed to persist run record: {e:?}");
    }
    // Roll the history: drop records beyond the cap so the collection cannot
    // grow unboundedly (the page shows 20, the API serves 200).
    let removed = BackupRecord::prune_beyond(BackupRecord::HISTORY_LIMIT);
    if removed > 0 {
        debug!("backup: pruned {removed} run record(s) beyond the history cap");
    }
}

/// Finalize the `Running` records a previous process left behind. The
/// `Running` record is persisted *before* capture and only `finish()` — same
/// process — transitions it; a crash or restart mid-run therefore leaves it
/// `Running` forever, since the in-memory machine died with the process. At
/// startup no run exists yet in this process, so *every* persisted `Running`
/// record is stale by definition. Called once from [`init`](super::init),
/// before the scheduler can start anything.
pub fn recover_interrupted_runs() {
    let Ok(stale) = DB_MANAGER
        .db()
        .collection(BackupRecord::collection())
        .list_all::<BackupRecord>()
    else {
        return;
    };
    for mut record in stale {
        if record.status != BackupRunStatus::Running {
            continue;
        }
        error!(
            "backup: run {} (started {}) was interrupted by a server restart; marking it failed",
            record.id, record.started_at
        );
        record.status = BackupRunStatus::Failed;
        record.finished_at = Some(utc_now!());
        record.error = Some("interrupted by a server restart".to_string());
        write_record(&record);
        // A restart mid-run is an anomaly an operator should find in the
        // error log, not only in the run history.
        errors::record(
            BackupErrorKind::AbruptExit,
            &record.trigger,
            Some(&record.id),
            &record.phase,
            "interrupted by a server restart",
            record.target.as_deref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::artifact::ArtifactKey;
    use crate::backup::backend::BackupSummary;
    use crate::backup::manifest::Manifest;
    use crate::backup::prepare::{BackupPreparer, UploadDelivery};
    use crate::backup::retention::RetentionPolicy;
    use crate::database::manager::DB_MANAGER;
    use crate::settings::cli::SETTINGS;
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Once};

    /// Shared temp-root env for the process globals, same convention as
    /// `retention.rs::tests`. Whichever module initializes the globals first
    /// wins, so tests must not depend on settings whose defaults change
    /// behavior (the machine tests drive `run_with_backend` directly rather
    /// than `request_run`, so the backup config document stays untouched).
    static TEST_ENV: Once = Once::new();
    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
            let _ = &*BLOB_MANAGER;
            prepare::register_base_preparers();
        });
    }

    /// The machine tests swap the global preparer registry and pause the
    /// write gate, and they read `config::retention_policy_struct()` mid-run —
    /// so they serialize against each other AND against the config tests via
    /// the shared `BACKUP_STATE_TESTS` lock (the per-module mutex below was
    /// unaware of the config tests, which write through the same gate).

    #[derive(Clone, Default)]
    struct CallLog {
        inner: Arc<Mutex<Vec<String>>>,
    }
    impl CallLog {
        fn push(&self, s: &str) {
            self.inner.lock().unwrap().push(s.to_string());
        }
        fn entries(&self) -> Vec<String> {
            self.inner.lock().unwrap().clone()
        }
    }

    /// A preparer that records `prepare:`/`upload:`/`finalize:` calls and can
    /// be told to fail prepare, panic in finalize, emit a non-fatal warning,
    /// or deliver an artifact (capture or upload phase).
    struct RecordingPreparer {
        name: &'static str,
        log: CallLog,
        fail_prepare: bool,
        panic_finalize: bool,
        warn_prepare: Option<String>,
        artifact: Option<Artifact>,
        upload_delivery: Option<UploadDelivery>,
    }
    impl RecordingPreparer {
        fn ok(name: &'static str, log: CallLog) -> Self {
            Self {
                name,
                log,
                fail_prepare: false,
                panic_finalize: false,
                warn_prepare: None,
                artifact: None,
                upload_delivery: None,
            }
        }
        fn failing(name: &'static str, log: CallLog) -> Self {
            Self {
                name,
                log,
                fail_prepare: true,
                panic_finalize: false,
                warn_prepare: None,
                artifact: None,
                upload_delivery: None,
            }
        }
        fn panicking_finalize(name: &'static str, log: CallLog) -> Self {
            Self {
                name,
                log,
                fail_prepare: false,
                panic_finalize: true,
                warn_prepare: None,
                artifact: None,
                upload_delivery: None,
            }
        }
        fn warning(name: &'static str, log: CallLog, warning: &str) -> Self {
            Self {
                name,
                log,
                fail_prepare: false,
                panic_finalize: false,
                warn_prepare: Some(warning.to_string()),
                artifact: None,
                upload_delivery: None,
            }
        }
        fn with_artifact(name: &'static str, log: CallLog, artifact: Artifact) -> Self {
            Self {
                name,
                log,
                fail_prepare: false,
                panic_finalize: false,
                warn_prepare: None,
                artifact: Some(artifact),
                upload_delivery: None,
            }
        }
        fn with_upload_delivery(name: &'static str, log: CallLog, delivery: UploadDelivery) -> Self {
            Self {
                name,
                log,
                fail_prepare: false,
                panic_finalize: false,
                warn_prepare: None,
                artifact: None,
                upload_delivery: Some(delivery),
            }
        }
    }
    impl BackupPreparer for RecordingPreparer {
        fn name(&self) -> &'static str {
            self.name
        }
        fn prepare<'a>(
            &'a self,
            _ctx: &'a PreparerContext,
        ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
            let log = self.log.clone();
            let name = self.name;
            let fail = self.fail_prepare;
            let warn = self.warn_prepare.clone();
            let artifact = self.artifact.clone();
            Box::pin(async move {
                log.push(&format!("prepare:{name}"));
                if let Some(w) = warn {
                    prepare::record_warning(w);
                }
                if fail {
                    Err(raise_error!(
                        "prepare failed (test)".to_string(),
                        ErrorCode::InternalError
                    ))
                } else {
                    Ok(artifact.map_or_else(Vec::new, |a| vec![a]))
                }
            })
        }
        fn upload<'a>(
            &'a self,
            _ctx: &'a PreparerContext,
        ) -> Pin<Box<dyn Future<Output = BichonResult<UploadDelivery>> + Send + 'a>> {
            let log = self.log.clone();
            let name = self.name;
            let delivery = self.upload_delivery.clone();
            Box::pin(async move {
                log.push(&format!("upload:{name}"));
                Ok(delivery.unwrap_or_default())
            })
        }
        fn finalize(&self, _ctx: &PreparerContext) {
            self.log.push(&format!("finalize:{}", self.name));
            if self.panic_finalize {
                panic!("finalize panic (test)");
            }
        }
    }

    /// Replaces the global preparer registry for the duration of a test and
    /// restores it on drop.
    struct TestPreparers(Vec<Arc<dyn BackupPreparer>>);
    impl Drop for TestPreparers {
        fn drop(&mut self) {
            prepare::set_preparers_for_test(std::mem::take(&mut self.0));
        }
    }
    fn with_test_preparers(preps: Vec<Arc<dyn BackupPreparer>>) -> TestPreparers {
        let old = prepare::take_preparers();
        prepare::set_preparers_for_test(preps);
        TestPreparers(old)
    }

    /// A fake backend with controllable failures. During `upload` it records
    /// whether the write gate was paused, how many artifacts it received and
    /// the audit stamp, so the tests can assert the two-phase invariant (R2)
    /// and the upload-phase contributor plumbing.
    struct FakeBackend {
        fail_preflight: bool,
        fail_upload: bool,
        manifest_id: &'static str,
        log: CallLog,
        gate_paused_during_upload: Arc<Mutex<Option<bool>>>,
        artifact_count: Arc<Mutex<usize>>,
        audit_stamp_received: Arc<Mutex<Option<AuditStamp>>>,
        previous_manifest: Option<Manifest>,
    }
    impl FakeBackend {
        fn ok(log: CallLog) -> Self {
            Self {
                fail_preflight: false,
                fail_upload: false,
                manifest_id: "fake-manifest-1",
                log,
                gate_paused_during_upload: Arc::new(Mutex::new(None)),
                artifact_count: Arc::new(Mutex::new(0)),
                audit_stamp_received: Arc::new(Mutex::new(None)),
                previous_manifest: None,
            }
        }
    }
    impl BackupBackend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn preflight<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<()>> + Send + 'a>> {
            let log = self.log.clone();
            let fail = self.fail_preflight;
            Box::pin(async move {
                log.push("preflight");
                if fail {
                    Err(raise_error!(
                        "preflight failed (test)".into(),
                        ErrorCode::InternalError
                    ))
                } else {
                    Ok(())
                }
            })
        }
        fn previous_manifest<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<Option<Manifest>>> + Send + 'a>> {
            let previous = self.previous_manifest.clone();
            Box::pin(async move { Ok(previous) })
        }
        fn upload<'a>(
            &'a self,
            artifacts: &'a [Artifact],
            _trigger: &'a str,
            _retention: &'a RetentionPolicy,
            audit: &'a AuditStamp,
        ) -> Pin<Box<dyn Future<Output = BichonResult<BackupSummary>> + Send + 'a>> {
            let log = self.log.clone();
            let fail = self.fail_upload;
            let manifest_id = self.manifest_id;
            let gate_paused = self.gate_paused_during_upload.clone();
            let artifact_count = self.artifact_count.clone();
            let stamp = *audit;
            let audit_stamp_received = self.audit_stamp_received.clone();
            Box::pin(async move {
                log.push("upload");
                *artifact_count.lock().unwrap() = artifacts.len();
                *gate_paused.lock().unwrap() = Some(WRITE_GATE.is_paused());
                *audit_stamp_received.lock().unwrap() = Some(stamp);
                if fail {
                    Err(raise_error!(
                        "upload failed (test)".into(),
                        ErrorCode::InternalError
                    ))
                } else {
                    Ok(BackupSummary {
                        manifest_id: manifest_id.to_string(),
                        uploaded_bytes: 42,
                        new_objects: 2,
                        skipped_objects: 1,
                        by_kind: Vec::new(),
                    })
                }
            })
        }
    }

    #[tokio::test]
    async fn scoped_guard_finalizes_and_resumes_gate_even_on_panic() {
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![
            Arc::new(RecordingPreparer::ok("a", log.clone())),
            Arc::new(RecordingPreparer::panicking_finalize("b", log.clone())),
        ]);
        let ctx = PreparerContext {
            root_dir: PathBuf::from("."),
            staging_dir: PathBuf::from("."),
            previous_manifest: None,
        };
        WRITE_GATE.pause();
        assert!(WRITE_GATE.is_paused());
        {
            let _guard = RunScopedGuard { ctx };
        }
        assert!(
            !WRITE_GATE.is_paused(),
            "gate must be reopened after the guard drops"
        );
        let entries = log.entries();
        assert!(
            entries.contains(&"finalize:a".to_string()),
            "non-panicking preparer must be finalized: {entries:?}"
        );
        assert!(
            entries.contains(&"finalize:b".to_string()),
            "a panicking finalizer must not stop the others: {entries:?}"
        );
        assert!(
            !entries.iter().any(|e| e.starts_with("prepare:")),
            "the guard must never run prepare: {entries:?}"
        );
    }

    #[tokio::test]
    async fn begin_run_rejects_while_a_run_holds_the_lock() {
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let guard = begin_run().expect("first begin_run should succeed");
        let e = begin_run().unwrap_err();
        assert_eq!(e.code(), ErrorCode::TooManyRequest, "busy run must be rejected");
        drop(guard);
        let _ = begin_run().expect("after release, begin_run succeeds again");
    }

    #[tokio::test]
    async fn full_run_success_writes_record_and_resumes_gate() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::ok("a", log.clone()))]);

        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(FakeBackend::ok(log.clone())))
            .await;

        let st = BACKUP_MANAGER.state();
        assert!(!st.running, "state must be idle after the run");
        assert_eq!(st.phase, "idle");
        assert!(st.last_success_at.is_some(), "success must be recorded");
        assert_eq!(
            st.last_snapshot_id.as_deref(),
            Some("fake-manifest-1"),
            "the manifest id must be surfaced as the restore point"
        );
        assert!(
            !WRITE_GATE.is_paused(),
            "gate must be reopened after a successful run"
        );
        let entries = log.entries();
        assert!(entries.contains(&"prepare:a".to_string()));
        assert!(entries.contains(&"finalize:a".to_string()));
        assert!(
            entries.iter().position(|e| e == "preflight")
                < entries.iter().position(|e| e == "upload"),
            "preflight must precede upload: {entries:?}"
        );

        let records = BackupRecord::list_recent(10);
        let mine = records
            .iter()
            .find(|r| {
                r.status == BackupRunStatus::Success
                    && r.snapshot_id.as_deref() == Some("fake-manifest-1")
                    && r.trigger == "manual"
            })
            .unwrap_or_else(|| panic!("a Success record must be persisted: {records:?}"));
        assert_eq!(
            mine.target.as_deref(),
            Some(&*crate::backup::config::destination()),
            "the record must name the destination it wrote to — the history is one \
             global list, so without this a target change makes it unreadable"
        );
    }

    /// Records are scoped by destination, so "the last successful backup" is
    /// always an answer about the target currently configured.
    #[tokio::test]
    async fn last_success_is_scoped_to_a_destination() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;

        let col = DB_MANAGER.db().collection(BackupRecord::collection());
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }

        let record = |id: &str, target: Option<&str>, at: i64| BackupRecord {
            id: id.to_string(),
            trigger: "manual".to_string(),
            started_at: at,
            finished_at: Some(at),
            status: BackupRunStatus::Success,
            phase: "finalize".to_string(),
            error: None,
            snapshot_id: Some(format!("m-{id}")),
            summary: None,
            warnings: vec![],
            target: target.map(str::to_string),
        };
        // Newest first: the newest run belongs to *another* destination.
        write_record(&record("old-here", Some("s3://aws/bucket"), 1_000));
        write_record(&record("new-there", Some("local:/mnt/backup"), 2_000));

        let here = BackupRecord::last_success_for("s3://aws/bucket").unwrap();
        assert_eq!(here.id, "old-here", "must skip the other destination's run");

        assert!(
            BackupRecord::last_success_for("local:/mnt/elsewhere").is_none(),
            "a destination with no history has no last success"
        );

        // A record written before `target` existed belongs to whatever is
        // configured now: those runs predate target switching.
        write_record(&record("legacy", None, 3_000));
        let legacy = BackupRecord::last_success_for("s3://aws/bucket").unwrap();
        assert_eq!(legacy.id, "legacy", "a legacy record must not be dropped");

        // Leave the shared collection as this test found it.
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }
    }

    /// The history rolls: writing beyond the cap prunes the oldest records,
    /// keeping the newest [`BackupRecord::HISTORY_LIMIT`].
    #[tokio::test]
    async fn run_record_history_rolls_beyond_the_cap() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;

        // Start from a clean collection.
        let col = DB_MANAGER.db().collection(BackupRecord::collection());
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }

        let base = 1_700_000_000_000i64;
        for i in 0..=BackupRecord::HISTORY_LIMIT + 10 {
            let record = BackupRecord {
                id: format!("hist-{i}"),
                trigger: "manual".to_string(),
                started_at: base + i as i64,
                finished_at: Some(base + i as i64 + 1),
                status: BackupRunStatus::Success,
                phase: "finalize".to_string(),
                error: None,
                snapshot_id: None,
                summary: None,
                warnings: vec![],
                target: None,
            };
            write_record(&record);
        }

        let kept = BackupRecord::list_recent(usize::MAX);
        assert_eq!(
            kept.len(),
            BackupRecord::HISTORY_LIMIT,
            "history must be capped at the limit"
        );
        // The newest records survive, the oldest are gone.
        assert!(
            kept.iter()
                .any(|r| r.id == format!("hist-{}", BackupRecord::HISTORY_LIMIT + 10)),
            "the newest record must survive"
        );
        assert!(
            !kept.iter().any(|r| r.id == "hist-0"),
            "the oldest records must be pruned"
        );

        // Clean up so other tests sharing the collection stay isolated.
        let col = DB_MANAGER.db().collection(BackupRecord::collection());
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }
    }

    /// A crash mid-run leaves the persisted `Running` record un-finalized —
    /// `finish()` lives in the dead process. Startup recovery must mark every
    /// stale `Running` record failed and leave terminal records untouched.
    #[tokio::test]
    async fn startup_recovery_finalizes_stale_running_records() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;

        // Start from a clean collection.
        let col = DB_MANAGER.db().collection(BackupRecord::collection());
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }

        let base = 1_700_000_000_000i64;
        let record = |id: &str, status: BackupRunStatus, error: Option<String>| BackupRecord {
            id: id.to_string(),
            trigger: "manual".to_string(),
            started_at: base,
            finished_at: None,
            status,
            phase: "capture".to_string(),
            error,
            snapshot_id: None,
            summary: None,
            warnings: vec![],
            target: None,
        };
        write_record(&record("stale-running", BackupRunStatus::Running, None));
        write_record(&record(
            "done-success",
            BackupRunStatus::Success,
            None,
        ));
        write_record(&record(
            "done-failed",
            BackupRunStatus::Failed,
            Some("boom".to_string()),
        ));

        recover_interrupted_runs();

        let records = BackupRecord::list_recent(usize::MAX);
        let recovered = records.iter().find(|r| r.id == "stale-running").unwrap();
        assert_eq!(
            recovered.status,
            BackupRunStatus::Failed,
            "the stale Running record must be finalized as failed"
        );
        assert!(
            recovered.finished_at.is_some(),
            "the recovered record needs a finish time"
        );
        assert!(
            recovered
                .error
                .as_deref()
                .unwrap()
                .contains("server restart"),
            "the error must say why the run ended: {:?}",
            recovered.error
        );

        // Terminal records are left exactly as they were.
        let success = records.iter().find(|r| r.id == "done-success").unwrap();
        assert_eq!(success.status, BackupRunStatus::Success);
        assert!(success.error.is_none());
        let failed = records.iter().find(|r| r.id == "done-failed").unwrap();
        assert_eq!(failed.status, BackupRunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("boom"));

        // The interruption is also in the rolling error log.
        let entries = error_entries_matching("interrupted by a server restart");
        assert!(
            entries
                .iter()
                .any(|e| e.run_id.as_deref() == Some("stale-running")),
            "the interrupted run must appear in the error log: {entries:?}"
        );

        // Clean up so other tests sharing the collection stay isolated.
        let col = DB_MANAGER.db().collection(BackupRecord::collection());
        for r in BackupRecord::list_recent(usize::MAX) {
            col.delete(r.key()).unwrap();
        }
    }

    #[tokio::test]
    async fn gate_is_open_during_upload_and_artifacts_flow_through() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();

        // One preparer delivers one artifact (a real file, like production).
        let local = DATA_DIR_MANAGER.backup_staging_dir.join("test-artifact.bin");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, b"hello backup").unwrap();
        let artifact = prepare::artifact_from_file(local.clone(), ArtifactKey::Memdb).unwrap();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::with_artifact(
            "a",
            log.clone(),
            artifact,
        ))]);

        let backend = FakeBackend::ok(log.clone());
        let gate_probe = backend.gate_paused_during_upload.clone();
        let artifact_probe = backend.artifact_count.clone();
        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(backend))
            .await;

        // R2: the write gate must be open while the backend uploads.
        let gate_was_paused = gate_probe
            .lock()
            .unwrap()
            .expect("upload must have run");
        assert!(
            !gate_was_paused,
            "the write gate must be reopened before upload (R2)"
        );
        assert_eq!(
            *artifact_probe.lock().unwrap(),
            1,
            "the captured artifact must reach the backend"
        );
        assert!(
            !WRITE_GATE.is_paused(),
            "gate must be open after the run"
        );

        let _ = std::fs::remove_file(&local);
    }

    #[tokio::test]
    async fn upload_phase_delivery_and_audit_stamp_reach_the_backend() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();

        // An upload-phase contributor delivers an artifact plus audit facts
        // (the Pro audit contributor's shape).
        let local = DATA_DIR_MANAGER.backup_staging_dir.join("delta.jsonl.gz");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, b"{}").unwrap();
        let delta = prepare::artifact_from_file(local.clone(), ArtifactKey::AuditDelta(5, 9)).unwrap();
        let delivery = UploadDelivery {
            artifacts: vec![delta],
            audit_cursor: Some(9),
            audit_cumulative_delta_bytes: Some(2048),
        };
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::with_upload_delivery(
            "a",
            log.clone(),
            delivery,
        ))]);

        let backend = FakeBackend::ok(log.clone());
        let artifact_probe = backend.artifact_count.clone();
        let stamp_probe = backend.audit_stamp_received.clone();
        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(backend))
            .await;

        // The upload-phase artifact joined the run's batch and reached the
        // backend, and the audit facts were forwarded verbatim.
        assert_eq!(
            *artifact_probe.lock().unwrap(),
            1,
            "the upload-phase artifact must reach the backend"
        );
        let stamp = stamp_probe.lock().unwrap().expect("upload must have run");
        assert_eq!(stamp.cursor, Some(9), "audit cursor must be forwarded");
        assert_eq!(
            stamp.cumulative_delta_bytes,
            Some(2048),
            "cumulative delta bytes must be forwarded"
        );
        let entries = log.entries();
        assert!(entries.contains(&"upload:a".to_string()), "{entries:?}");
        assert!(
            !WRITE_GATE.is_paused(),
            "upload-phase contributor runs with the gate open (R2)"
        );

        let _ = std::fs::remove_file(&local);
    }

    #[tokio::test]
    async fn preparer_failure_still_resumes_and_records_failed() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::failing(
            "bad",
            log.clone(),
        ))]);

        WRITE_GATE.resume(); // defensive: start from an open gate
        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(FakeBackend::ok(log.clone())))
            .await;

        let st = BACKUP_MANAGER.state();
        assert!(!st.running);
        assert!(
            !WRITE_GATE.is_paused(),
            "gate must be reopened even when a preparer fails"
        );
        assert!(st.last_error.is_some(), "the failure must be surfaced");
        assert!(
            log.entries().contains(&"finalize:bad".to_string()),
            "finalize must run even after a prepare failure"
        );
        assert!(
            !log.entries().contains(&"upload".to_string()),
            "upload must not run when capture failed"
        );

        let records = BackupRecord::list_recent(10);
        assert!(
            records.iter().any(|r| r.status == BackupRunStatus::Failed),
            "a Failed record must be persisted: {records:?}"
        );

        // The failure also lands in the rolling error log, with the run
        // context an operator needs to see it in the WebUI.
        let entries = error_entries_matching("prepare failed (test)");
        assert_eq!(entries.len(), 1, "one error entry: {entries:?}");
        assert_eq!(entries[0].kind, BackupErrorKind::Failed);
        assert_eq!(entries[0].trigger, "manual");
        assert_eq!(entries[0].phase, "capture:bad");
        assert!(
            entries[0].run_id.is_some(),
            "a reported failure must name its run record"
        );
    }

    #[tokio::test]
    async fn preparer_warnings_flow_into_state_and_record() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::warning(
            "w",
            log.clone(),
            "audit database is 2048 MB, backups will slow down",
        ))]);

        WRITE_GATE.resume(); // defensive: start from an open gate
        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(FakeBackend::ok(log.clone())))
            .await;

        let st = BACKUP_MANAGER.state();
        assert_eq!(
            st.last_warnings,
            vec!["audit database is 2048 MB, backups will slow down".to_string()],
            "warnings must be surfaced on the status view"
        );

        let records = BackupRecord::list_recent(10);
        assert!(
            records.iter().any(|r| {
                r.warnings
                    .iter()
                    .any(|w| w.contains("audit database is 2048 MB"))
            }),
            "warnings must be persisted on the run record"
        );
    }

    #[tokio::test]
    async fn preflight_failure_resumes_without_preparing() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::ok("a", log.clone()))]);

        let backend = FakeBackend {
            fail_preflight: true,
            fail_upload: false,
            manifest_id: "",
            log: log.clone(),
            gate_paused_during_upload: Arc::new(Mutex::new(None)),
            artifact_count: Arc::new(Mutex::new(0)),
            audit_stamp_received: Arc::new(Mutex::new(None)),
            previous_manifest: None,
        };
        BACKUP_MANAGER
            .run_with_backend("manual", Box::new(backend))
            .await;

        let st = BACKUP_MANAGER.state();
        assert!(!st.running);
        assert!(
            !WRITE_GATE.is_paused(),
            "pre-flight failure must not leave the gate paused"
        );
        assert!(
            !log.entries().iter().any(|e| e.starts_with("prepare:")),
            "preparers must not run when pre-flight fails"
        );
        let records = BackupRecord::list_recent(10);
        assert!(records.iter().any(|r| r.status == BackupRunStatus::Failed));
    }

    // ── abnormal-exit hardening ────────────────────────────────────────────

    /// Error-log entries whose message contains `fragment` — the rolling log
    /// is shared across tests, so assertions filter instead of counting.
    fn error_entries_matching(fragment: &str) -> Vec<crate::backup::errors::BackupErrorEntry> {
        errors::list_recent(errors::LIMIT)
            .into_iter()
            .filter(|e| e.message.contains(fragment))
            .collect()
    }

    /// Milliseconds instead of the production minutes, so a stuck future is
    /// exercised quickly.
    fn short_timeouts() -> RunTimeouts {
        RunTimeouts {
            preflight: Duration::from_millis(20),
            manifest: Duration::from_millis(20),
        }
    }

    /// Set on drop, proving a timed-out backend future was *cancelled* — not
    /// left running in the background holding a socket.
    struct DropProbe(Arc<AtomicBool>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[derive(Clone, Copy)]
    enum HangStep {
        Preflight,
        Manifest,
    }

    /// A backend whose preflight or restore-point read never resolves.
    struct HangingBackend {
        hang: HangStep,
        cancelled: Arc<AtomicBool>,
    }
    impl BackupBackend for HangingBackend {
        fn name(&self) -> &'static str {
            "hanging"
        }
        fn preflight<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<()>> + Send + 'a>> {
            let cancelled = self.cancelled.clone();
            let hang = matches!(self.hang, HangStep::Preflight);
            Box::pin(async move {
                if hang {
                    let _probe = DropProbe(cancelled);
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }
        fn previous_manifest<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<Option<Manifest>>> + Send + 'a>> {
            let cancelled = self.cancelled.clone();
            let hang = matches!(self.hang, HangStep::Manifest);
            Box::pin(async move {
                if hang {
                    let _probe = DropProbe(cancelled);
                    std::future::pending::<()>().await;
                }
                Ok(None)
            })
        }
        fn upload<'a>(
            &'a self,
            _artifacts: &'a [Artifact],
            _trigger: &'a str,
            _retention: &'a RetentionPolicy,
            _audit: &'a AuditStamp,
        ) -> Pin<Box<dyn Future<Output = BichonResult<BackupSummary>> + Send + 'a>> {
            Box::pin(async move {
                unreachable!("upload must not run when preflight never finished");
                #[allow(unreachable_code)]
                Ok(BackupSummary::default())
            })
        }
    }

    /// A preflight that panics, standing in for any panic inside a backend or
    /// contributor. The drop probe proves the backend — and with it the S3
    /// client and its connection pool — is released when the task dies.
    struct PanickingBackend {
        dropped: Arc<AtomicBool>,
    }
    impl Drop for PanickingBackend {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    impl BackupBackend for PanickingBackend {
        fn name(&self) -> &'static str {
            "panicking"
        }
        fn preflight<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<()>> + Send + 'a>> {
            Box::pin(async move {
                panic!("preflight panic (test)");
                #[allow(unreachable_code)]
                Ok(())
            })
        }
        fn previous_manifest<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = BichonResult<Option<Manifest>>> + Send + 'a>> {
            Box::pin(async move { Ok(None) })
        }
        fn upload<'a>(
            &'a self,
            _artifacts: &'a [Artifact],
            _trigger: &'a str,
            _retention: &'a RetentionPolicy,
            _audit: &'a AuditStamp,
        ) -> Pin<Box<dyn Future<Output = BichonResult<BackupSummary>> + Send + 'a>> {
            Box::pin(async move {
                unreachable!("upload must not run after a panicking preflight");
                #[allow(unreachable_code)]
                Ok(BackupSummary::default())
            })
        }
    }

    #[tokio::test]
    async fn preflight_timeout_fails_the_run_and_cancels_the_hung_future() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::ok("a", log.clone()))]);
        WRITE_GATE.resume();

        let cancelled = Arc::new(AtomicBool::new(false));
        BACKUP_MANAGER
            .run_with_timeouts(
                "manual",
                Box::new(HangingBackend {
                    hang: HangStep::Preflight,
                    cancelled: cancelled.clone(),
                }),
                short_timeouts(),
            )
            .await;

        assert!(
            cancelled.load(Ordering::Acquire),
            "the timed-out preflight future must be dropped — otherwise it keeps \
             running in the background, holding its connection"
        );
        let st = BACKUP_MANAGER.state();
        assert!(!st.running, "the run must end, not stay 'running': {st:?}");
        assert_eq!(st.phase, "idle");
        assert!(
            st.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("preflight"),
            "the timeout must be surfaced as the run's error: {st:?}"
        );
        assert!(!WRITE_GATE.is_paused(), "no write window may be left open");
        assert!(
            !log.entries().iter().any(|e| e.starts_with("prepare:")),
            "no preparer may run after a preflight that never succeeded"
        );

        let records = BackupRecord::list_recent(10);
        assert!(
            records.iter().any(|r| {
                r.status == BackupRunStatus::Failed
                    && r.error.as_deref().unwrap_or_default().contains("preflight")
            }),
            "a Failed record must name the preflight timeout: {records:?}"
        );

        let entries = error_entries_matching("preflight did not finish within");
        assert_eq!(entries.len(), 1, "one error entry: {entries:?}");
        assert_eq!(entries[0].kind, BackupErrorKind::Failed);
        assert_eq!(entries[0].phase, "preflight");
        assert_eq!(entries[0].trigger, "manual");
    }

    #[tokio::test]
    async fn manifest_read_timeout_fails_the_run_and_cancels_the_hung_future() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        let log = CallLog::default();
        let _restore = with_test_preparers(vec![Arc::new(RecordingPreparer::ok("a", log.clone()))]);
        WRITE_GATE.resume();

        let cancelled = Arc::new(AtomicBool::new(false));
        BACKUP_MANAGER
            .run_with_timeouts(
                "manual",
                Box::new(HangingBackend {
                    hang: HangStep::Manifest,
                    cancelled: cancelled.clone(),
                }),
                short_timeouts(),
            )
            .await;

        assert!(
            cancelled.load(Ordering::Acquire),
            "the timed-out restore-point read must be dropped"
        );
        let st = BACKUP_MANAGER.state();
        assert!(!st.running, "the run must end: {st:?}");
        assert_eq!(st.phase, "idle");
        assert!(
            st.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("previous restore point"),
            "the timeout must be surfaced: {st:?}"
        );
        assert!(!WRITE_GATE.is_paused());
        let records = BackupRecord::list_recent(10);
        assert!(
            records.iter().any(|r| r.status == BackupRunStatus::Failed),
            "a Failed record must be persisted"
        );
    }

    /// A panic inside the run must not leave `running = true` behind: the
    /// status card would otherwise report a live run forever and the WebUI
    /// would keep "Run now" disabled until a restart.
    #[tokio::test]
    async fn panicking_run_clears_the_running_status() {
        init_test_env();
        let _run_guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        WRITE_GATE.resume();

        let dropped = Arc::new(AtomicBool::new(false));
        let backend_probe = dropped.clone();
        let task = tokio::spawn(async move {
            BACKUP_MANAGER
                .run_with_timeouts(
                    "manual",
                    Box::new(PanickingBackend {
                        dropped: backend_probe,
                    }),
                    RunTimeouts::default(),
                )
                .await;
        });
        assert!(
            task.await.is_err(),
            "the spawned run task must surface the panic"
        );
        assert!(
            dropped.load(Ordering::Acquire),
            "the backend (its S3 client and connection pool) must be dropped \
             when the run task dies — nothing may outlive the run"
        );

        let st = BACKUP_MANAGER.state();
        assert!(
            !st.running,
            "a panicked run must clear the running flag: {st:?}"
        );
        assert_eq!(st.phase, "idle");
        assert!(
            st.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("ended unexpectedly"),
            "the abnormal exit must be surfaced: {st:?}"
        );
        assert!(
            !WRITE_GATE.is_paused(),
            "the write gate must be reopened after a panic"
        );

        // The abnormal exit is visible in the WebUI's error log too, not only
        // in the in-memory status.
        let entries = error_entries_matching("ended unexpectedly");
        assert_eq!(entries.len(), 1, "one abrupt-exit entry: {entries:?}");
        assert_eq!(entries[0].kind, BackupErrorKind::AbruptExit);
        assert_eq!(entries[0].phase, "preflight");
        assert_eq!(entries[0].trigger, "manual");
    }
}
