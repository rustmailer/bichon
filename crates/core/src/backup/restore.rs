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

//! Restore from a native restore point — the reverse of the engine. The
//! source is whichever target [`RestoreOptions::target_type`] selects: an
//! S3-compatible endpoint or a local directory.
//!
//! A manifest is fully self-describing ("each lists every object it
//! references", design doc §4), so a restore is a plain, idempotent fan-out:
//! read one manifest, stream every object to its on-disk home, verify
//! SHA-256 *while writing* (R11 — a corrupt object is never silently
//! restored), then stamp `STORAGE_VERSION` so `check_data_status` accepts the
//! result.
//!
//! # Safety rails (R7)
//!
//! A restore writes **only into an empty directory**: the target must not
//! exist, or be empty; it must not be the live data root; and it must not
//! contain or be contained by the live data root. There is no `--force` and
//! no in-place mode — a restore point is applied to a directory the server
//! has never seen, and the operator then points a fresh install at it.
//!
//! # Edition seam (R10)
//!
//! The engine's restore is edition-neutral. Pro/Enterprise data that the
//! community engine knows nothing about is carried by the generic slots
//! (`audit`, `integrity`, `timestamp`); the *replay* of those artifacts is
//! delegated to an [`AuditReplayer`] registered by the Pro binary through
//! [`register_audit_replayer`]. With no replayer registered the audit deltas
//! are still restored — copied verbatim under `audit/deltas/` — so nothing
//! is ever silently dropped (R11).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use object_store::ObjectStore;
use tracing::{info, warn};

use crate::backup::config::{BackupTargetType, RestoreFileConfig, DEFAULT_PREFIX};
use crate::backup::engine::{s3_store, BackupEngine, S3Options};
use crate::backup::manifest::{Manifest, ObjectRef, LATEST_NAME, MANIFEST_PREFIX};
use crate::backup::path::{check_free_space, path_contains, paths_equal};
use crate::backup::prepare::unpackage_dir;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::migrate::{write_storage_version, CURRENT_STORAGE_VERSION};
use crate::raise_error;
use crate::settings::cli::RestoreArgs;

/// Layout mapping: the on-disk home of each slot under the restore target.
/// Mirrors `settings::dir` / `feature_db_path` so a fresh install pointed at
/// the restored directory finds everything where it expects it.
pub(crate) const MEMDB_DIR: &str = "memdb";
pub(crate) const STORAGE_DIR: &str = "bichon-storage";
pub(crate) const BLOBS_DIR: &str = "blobs";
pub(crate) const SEGMENTS_DIR: &str = "segments";
pub(crate) const AUDIT_DIR: &str = "audit";
pub(crate) const AUDIT_DB_FILE: &str = "audit.db";
pub(crate) const AUDIT_DELTAS_DIR: &str = "deltas";
pub(crate) const INTEGRITY_DIR: &str = "integrity";
pub(crate) const INTEGRITY_DB_FILE: &str = "integrity.db";
pub(crate) const TIMESTAMP_DIR: &str = "timestamp";
pub(crate) const ANCHOR_DB_FILE: &str = "anchor.db";
pub(crate) const INDICES_DIR: &str = "bichon-indices";
pub(crate) const ENVELOPE_INDEX_DIR: &str = "mail_metadata";
pub(crate) const ATTACHMENT_INDEX_DIR: &str = "attachment_metadata";

/// CLI restore options for the one-shot disaster-recovery tools.
///
/// `Default` is derived so that adding a field does not have to touch every
/// construction site — this struct is built by literal in a dozen places
/// across the core tests and the admin binary.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Which backend the restore point is read from.
    pub target_type: BackupTargetType,
    /// `s3://<endpoint>/<bucket>/<prefix>` — the backup target. Only
    /// meaningful for an S3 target; use [`RestoreOptions::target_display`] for
    /// anything an operator reads.
    pub s3_uri: String,
    /// Absolute path of the local backup directory (when
    /// `target_type` is `local`).
    pub local_dir: Option<PathBuf>,
    /// Object-store prefix for a local target. An S3 target carries its
    /// prefix inside `s3_uri` instead.
    pub prefix: String,
    /// Restore a specific point; default = LATEST. [`normalize_point`]
    /// accepts the id, the pasted object name (`m-<id>` / `m-<id>.json.gz`),
    /// or the keyword `LATEST` (any case) for the newest point.
    pub point: Option<String>,
    /// Empty target directory (R7). Ignored in `verify_only` mode.
    pub into: PathBuf,
    /// Target index parent dir (mirrors the `bichon-index-dir` startup
    /// param). `None` = default layout, indices under
    /// `<into>/bichon-indices`. R7: must be empty and disjoint from the live
    /// data root.
    pub index_dir: Option<PathBuf>,
    /// Target blob data parent dir (mirrors the `bichon-data-dir` startup
    /// param). `None` = default layout, blob store under
    /// `<into>/bichon-storage`. R7: must be empty and disjoint from the live
    /// data root.
    pub data_dir: Option<PathBuf>,
    /// True = drill mode: verify every object of the point against the
    /// manifest, write nothing (design doc §8).
    pub verify_only: bool,
    /// Optional credential/region overrides; anything not passed explicitly
    /// is resolved by the restore CLI from the local config store (the
    /// WebUI-configured backup target) when one is present.
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    /// The live data root of an existing install, if the invoking binary
    /// knows one (flag or `BICHON_ROOT_DIR`). The R7 guards refuse restore
    /// targets that touch it. The engine deliberately does not read the
    /// global `SETTINGS`: restore runs from one-shot tools (`bichon-admin
    /// restore`) that parse no server settings. `None`/empty = fresh box,
    /// nothing to protect.
    pub live_root: Option<PathBuf>,
}

/// The resolved restore target directories, mirroring `DataDirManager::new`
/// (`settings/dir.rs`): an explicit index/data parent dir appends the
/// `bichon-indices` / `bichon-storage` subdir; otherwise both live under the
/// target root. R7 applies to each of the three.
#[derive(Debug, Clone)]
pub struct RestoreTargets {
    /// Target root — everything that lives under the data root.
    pub root: PathBuf,
    /// Target for the tantivy indices (`bichon-indices`).
    pub index: PathBuf,
    /// Target for the blob store (`bichon-storage`).
    pub data: PathBuf,
}

impl RestoreTargets {
    pub fn resolve(opts: &RestoreOptions) -> RestoreTargets {
        RestoreTargets {
            root: opts.into.clone(),
            index: match &opts.index_dir {
                Some(p) => p.join(INDICES_DIR),
                None => opts.into.join(INDICES_DIR),
            },
            data: match &opts.data_dir {
                Some(p) => p.join(STORAGE_DIR),
                None => opts.into.join(STORAGE_DIR),
            },
        }
    }
}

impl RestoreOptions {
    /// Where this restore reads from, for logs and operator-facing messages.
    pub fn target_display(&self) -> String {
        match self.target_type {
            BackupTargetType::S3 => self.s3_uri.clone(),
            BackupTargetType::Local => match &self.local_dir {
                Some(dir) => dir.display().to_string(),
                None => "<local directory not configured>".to_string(),
            },
        }
    }

    /// The object-store prefix, with the default applied. Only used by the
    /// local branch — the S3 branch takes its prefix from the URI, so the two
    /// can never disagree.
    fn effective_prefix(&self) -> String {
        let prefix = self.prefix.trim().trim_end_matches('/');
        if prefix.is_empty() {
            DEFAULT_PREFIX.to_string()
        } else {
            prefix.to_string()
        }
    }
}

/// The outcome of a restore run, for the CLI/WebUI report.
#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub manifest_id: String,
    pub created: String,
    pub trigger: String,
    /// Total objects referenced by the manifest.
    pub objects_total: usize,
    /// Total bytes referenced by the manifest.
    pub bytes_total: u64,
    /// Bytes downloaded and verified (verify-only: bytes streamed).
    pub bytes_checked: u64,
    pub memdb_restored: bool,
    /// Accounts whose IMAP UIDVALIDITY generation was bumped so clients drop
    /// their caches (see `archive::imap::stable_uid::bump_all_generations`).
    pub generations_bumped: usize,
    pub segments_restored: usize,
    pub audit_restored: bool,
    pub audit_deltas_replayed: usize,
    /// Deltas preserved verbatim because no replayer was registered (R11).
    pub audit_deltas_copied: usize,
    pub integrity_restored: bool,
    pub timestamp_restored: bool,
    /// `instance-id` was restored from the deployment identity package, so
    /// the existing license still binds to this data set.
    pub instance_id_restored: bool,
    /// `license.txt` was restored alongside it.
    pub license_restored: bool,
    /// `license-state.json` (trial start / installed ledger) was restored, so
    /// a restore cannot reset a trial.
    pub license_state_restored: bool,
    /// Number of index directories restored from the point (0 if the point
    /// carries no index copies).
    pub tantivy_restored: usize,
    /// Honest note when search cannot be restored directly (R11): either the
    /// point has no index copies, or the search index needs rebuilding.
    pub note: Option<String>,
    pub verify_only: bool,
    pub into: String,
}

/// Resolve the `restore` subcommand's arguments into engine options.
///
/// Shared by the `bichon-admin restore` entry point (community) and the Pro
/// admin binary — both build the same [`RestoreOptions`] from the
/// subcommand's flags. The backend connection details come from the JSON
/// config file named by `--config` ([`RestoreFileConfig`]) — an S3-compatible
/// endpoint or a local directory; restore deliberately does not read the local
/// install's metadata store, so it runs
/// unchanged on a fresh disaster-recovery box. `--into` is required unless
/// the run is a `verify_only` drill (R7).
pub fn restore_options_from_cli(args: &RestoreArgs, verify_only: bool) -> BichonResult<RestoreOptions> {
    let into = PathBuf::from(args.into.clone());
    if !verify_only && into.as_os_str().is_empty() {
        return Err(raise_error!(
            "restore: --into is required unless this is a verify-only drill".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let file = RestoreFileConfig::load(Path::new(&args.config))?;
    Ok(RestoreOptions {
        target_type: file.target_type,
        s3_uri: file.s3_uri(),
        local_dir: file.local_dir.as_deref().map(PathBuf::from),
        prefix: file.prefix.clone(),
        point: args.point.clone(),
        into,
        index_dir: args.index_dir.clone().map(PathBuf::from),
        data_dir: args.data_dir.clone().map(PathBuf::from),
        verify_only,
        access_key: file.s3_access_key,
        secret_key: file.s3_secret_key,
        region: file.s3_region,
        // The R7 guards protect an existing install's live data root; the
        // flag (and `BICHON_ROOT_DIR` via its env binding) names it when the
        // box still has one. Absent = fresh disaster-recovery box.
        live_root: args.root_dir.clone().map(PathBuf::from),
    })
}

/// A registered Pro/Enterprise auditor that can replay a backup's audit chain
/// into a restored audit database.
///
/// The community edition ships none — the restore module stays edition-
/// neutral (R10), and without a replayer deltas are copied verbatim instead
/// of dropped (R11).
pub trait AuditReplayer: Send + Sync {
    fn name(&self) -> &'static str;

    /// Replay `package` (gzip JSONL, seq ascending, rows `seq_from < seq <=
    /// seq_to`) into the audit database at `audit_db_path`, creating it if
    /// needed. Runs inside a write transaction; must be idempotent per row
    /// (INSERT OR IGNORE semantics on `seq`).
    fn apply_delta(
        &self,
        package: &Path,
        audit_db_path: &Path,
        seq_from: i64,
        seq_to: i64,
    ) -> BichonResult<()>;

    /// Post-restore retention sweep: purge audit rows older than the
    /// configured window (mirrors the live sweep so a restored audit store
    /// does not resurrect rows the live store would have purged).
    fn apply_retention(&self, audit_db_path: &Path) -> BichonResult<()>;
}

/// Monotonic counter that makes each restore's scratch dir unique within the
/// process (parallel restores / tests must never share one).
static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

static AUDIT_REPLAYERS: LazyLock<Mutex<Vec<Arc<dyn AuditReplayer>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Register an audit replayer (Pro binary, before restore runs). The first
/// registered replayer owns the audit replay.
pub fn register_audit_replayer(replayer: Arc<dyn AuditReplayer>) {
    AUDIT_REPLAYERS.lock().unwrap().push(replayer);
}

/// Normalize a `--point` value (CLI flag, interactive prompt, drill) to a
/// manifest id. `LATEST` (any case) and blank select the newest point — the
/// same as passing nothing — and a name pasted straight from the bucket
/// listing (`m-<id>` or `m-<id>.json.gz`) is accepted for the bare `<id>`,
/// so `load_by_id` does not look for `m-m-<id>.json.gz`.
pub(crate) fn normalize_point(point: Option<&str>) -> Option<String> {
    let raw = point?.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case(LATEST_NAME) {
        return None;
    }
    let id = raw
        .strip_prefix(MANIFEST_PREFIX)
        .unwrap_or(raw)
        .trim_end_matches(".json.gz");
    Some(id.to_string())
}

/// Parse `s3://<endpoint>/<bucket>/<prefix>` into `(endpoint, bucket, prefix)`.
/// `endpoint` is the bare host[:port] (`s3://localhost:9000/bichon-backups/...`);
/// the prefix is optional and defaults to [`DEFAULT_PREFIX`].
pub fn parse_s3_uri(uri: &str) -> BichonResult<(String, String, String)> {
    let rest = uri.strip_prefix("s3://").ok_or_else(|| {
        raise_error!(
            format!(
                "restore: {uri:?} is not an s3:// URI (expected s3://<endpoint>/<bucket>/<prefix>)"
            ),
            ErrorCode::InvalidParameter
        )
    })?;
    // The endpoint may carry a scheme (`http://host:port` for MinIO/R2/Wasabi
    // style, `https://` for TLS object stores, bare host for AWS) or be empty
    // (AWS — the SDK default endpoint). Peel a scheme off *before* splitting
    // at the first path separator, so `s3://http://h:9000/b/prefix` is not
    // misread as endpoint `http:`.
    let (endpoint, path) = if let Some(rest) = rest.strip_prefix("http://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("http://{host}"), path)
    } else if let Some(rest) = rest.strip_prefix("https://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("https://{host}"), path)
    } else {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (host.to_string(), path)
    };
    let (bucket, prefix) = match path.split_once('/') {
        Some((b, p)) => (b, p),
        None => (path, ""),
    };
    if bucket.is_empty() {
        return Err(raise_error!(
            "restore: bucket is required in {uri:?}".to_string(),
            ErrorCode::InvalidParameter
        ));
    }
    let prefix = if prefix.is_empty() {
        DEFAULT_PREFIX.to_string()
    } else {
        prefix.trim_end_matches('/').to_string()
    };
    Ok((endpoint, bucket.to_string(), prefix))
}

/// The CLI entry point: parse the URI, build the store and run the restore.
pub async fn run_restore(opts: &RestoreOptions) -> BichonResult<RestoreReport> {
    let engine = build_restore_engine(opts)?;
    restore_with_engine(opts, &engine).await
}

/// Build the engine for a restore run. Shared by [`run_restore`] and
/// [`restore_preflight`].
pub fn build_restore_engine(opts: &RestoreOptions) -> BichonResult<BackupEngine> {
    let (store, prefix): (Arc<dyn ObjectStore>, String) = match opts.target_type {
        BackupTargetType::S3 => {
            let (endpoint, bucket, prefix) = parse_s3_uri(&opts.s3_uri)?;
            let store_opts = S3Options {
                endpoint,
                bucket,
                region: opts
                    .region
                    .clone()
                    .unwrap_or_else(|| "us-east-1".to_string()),
                // Restore runs before the server starts (the DB is not open
                // yet), so there is no WebUI/system_config fallback here:
                // credentials come from the restore config file. Empty strings
                // are fine for public / anonymous-access buckets.
                access_key: opts.access_key.clone().unwrap_or_default(),
                secret_key: opts.secret_key.clone().unwrap_or_default(),
                force_path_style: true,
            };
            (s3_store(&store_opts)?, prefix)
        }
        BackupTargetType::Local => {
            let dir = opts.local_dir.clone().ok_or_else(|| {
                raise_error!(
                    "restore: a local target requires a directory".to_string(),
                    ErrorCode::InvalidParameter
                )
            })?;
            (BackupEngine::local_store(&dir)?, opts.effective_prefix())
        }
    };
    Ok(BackupEngine::new(store, &prefix))
}

/// The read-only first half of a restore: connect to the backup target and
/// load the requested manifest. Interactive tools run this first to test the
/// connection (endpoint / credentials / bucket / prefix are each diagnosable
/// from the error) and to report the point's size before touching any disk.
#[derive(Debug, Clone)]
pub struct RestorePreflight {
    pub manifest: Manifest,
    /// Objects referenced by the point.
    pub objects_total: usize,
    /// Total bytes the point will download.
    pub bytes_total: u64,
}

pub async fn restore_preflight(opts: &RestoreOptions) -> BichonResult<RestorePreflight> {
    let engine = build_restore_engine(opts)?;
    let manifest = match normalize_point(opts.point.as_deref()) {
        Some(id) => engine.load_by_id(&id).await?.ok_or_else(|| {
            raise_error!(
                format!("restore: restore point {id} not found"),
                ErrorCode::ResourceNotFound
            )
        })?,
        None => engine.load_latest().await?.ok_or_else(|| {
            raise_error!(
                format!(
                    "restore: no restore point found at {} (run the backup first)",
                    opts.target_display()
                ),
                ErrorCode::ResourceNotFound
            )
        })?,
    };
    let refs = manifest_refs(&manifest);
    let bytes_total: u64 = refs.iter().map(|r| r.2).sum();
    Ok(RestorePreflight {
        objects_total: refs.len(),
        bytes_total,
        manifest,
    })
}

/// R7 target validation, runnable before the restore starts so interactive
/// tools can surface problems early. Resolves the three target dirs, refuses
/// any that is non-empty or touches the live data root, refuses overlaps,
/// and creates the target root. [`restore_with_engine`] runs the same check.
pub fn check_restore_targets(opts: &RestoreOptions) -> BichonResult<RestoreTargets> {
    // ── R7: only empty, disjoint targets ──────────────────────────────────
    // A restore lands on three directories (root, index, blob data), each
    // mirroring the `bichon-root/index/data-dir` startup layout. Every one
    // must be empty and disjoint from the live data root.
    let targets = RestoreTargets::resolve(opts);
    let root = opts.live_root.clone().unwrap_or_default();
    for dir in [&targets.root, &targets.index, &targets.data] {
        assert_clean_restore_target(dir, &root)?;
    }
    if paths_equal(&targets.index, &targets.data)
        || paths_equal(&targets.index, &targets.root)
        || paths_equal(&targets.data, &targets.root)
    {
        return Err(raise_error!(
            "restore: target root, index and data dirs must be distinct (R7)".into(),
            ErrorCode::InvalidParameter
        ));
    }
    std::fs::create_dir_all(&targets.root).map_err(|e| {
        raise_error!(
            format!(
                "restore: cannot create target {}: {e}",
                targets.root.display()
            ),
            ErrorCode::InternalError
        )
    })?;
    Ok(targets)
}

/// Refuse before writing anything if any target's filesystem lacks room for
/// the objects that will land on it. Public for the interactive preflight;
/// [`restore_with_engine`] re-runs it right before the fan-out.
pub fn check_restore_space(targets: &RestoreTargets, manifest: &Manifest) -> BichonResult<()> {
    assert_sufficient_space(targets, manifest)
}

/// Run a verify-only drill against the **configured** backup target (the
/// Pro/Enterprise WebUI, design doc §8): every object of the restore
/// point is hashed and compared to the manifest, nothing is written. `point`
/// is the manifest id; `None` verifies the latest restore point.
///
/// Builds the engine from the page-configured target (same store + prefix as
/// the run-time backend), so a drill always exercises the real target —
/// whichever kind it is.
pub async fn run_drill(point: Option<&str>) -> BichonResult<RestoreReport> {
    let target = crate::backup::config::target()?;
    let store = target.build_store()?;
    let engine =
        crate::backup::engine::BackupEngine::new(store, &crate::backup::config::prefix());
    let ropts = RestoreOptions {
        target_type: crate::backup::config::target_type(),
        s3_uri: target.display(),
        local_dir: crate::backup::config::local_dir().map(PathBuf::from),
        prefix: crate::backup::config::prefix(),
        point: point.map(|s| s.to_string()),
        into: PathBuf::new(),
        index_dir: None,
        data_dir: None,
        verify_only: true,
        access_key: None,
        secret_key: None,
        region: None,
        // Drill mode writes nothing (returns before the R7 guards run).
        live_root: None,
    };
    restore_with_engine(&ropts, &engine).await
}

/// Core restore routine over an existing engine (tests inject a local or
/// in-memory store).
pub(crate) async fn restore_with_engine(
    opts: &RestoreOptions,
    engine: &BackupEngine,
) -> BichonResult<RestoreReport> {
    // ── pick the restore point ────────────────────────────────────────────
    let manifest = match normalize_point(opts.point.as_deref()) {
        Some(id) => engine.load_by_id(&id).await?.ok_or_else(|| {
            raise_error!(
                format!("restore: restore point {id} not found"),
                ErrorCode::ResourceNotFound
            )
        })?,
        None => engine.load_latest().await?.ok_or_else(|| {
            raise_error!(
                format!(
                    "restore: no restore point found at {} ({} run the backup first)",
                    opts.target_display(),
                    ""
                ),
                ErrorCode::ResourceNotFound
            )
        })?,
    };
    let refs = manifest_refs(&manifest);
    let bytes_total: u64 = refs.iter().map(|r| r.2).sum();

    let mut report = RestoreReport {
        manifest_id: manifest.id.clone(),
        created: manifest.created.clone(),
        trigger: manifest.trigger.clone(),
        objects_total: refs.len(),
        bytes_total,
        verify_only: opts.verify_only,
        into: if opts.verify_only {
            "<verify-only>".to_string()
        } else {
            opts.into.display().to_string()
        },
        ..Default::default()
    };

    // ── drill mode: verify every object, write nothing (design doc §8) ────
    if opts.verify_only {
        info!(
            "restore: verify-only — checking {} objects of {id}",
            refs.len(),
            id = manifest.id
        );
        for (key, sha256, bytes) in &refs {
            let got = engine.hash_object(key, *bytes).await?;
            let got_hex = hex::encode(got);
            if got_hex != *sha256 {
                return Err(raise_error!(
                    format!("restore: object {key} hash mismatch: got {got_hex}, manifest says {sha256}"),
                    ErrorCode::InternalError
                ));
            }
            report.bytes_checked += bytes;
        }
        report.note = Some(format!(
            "verify-only: all {} objects of restore point {} matched the manifest",
            report.objects_total, manifest.id
        ));
        return Ok(report);
    }

    // ── R7 targets + capacity (same checks the interactive preflight runs;
    // re-validated here so a direct `run_restore` is equally guarded) ──────
    let targets = check_restore_targets(opts)?;
    check_restore_space(&targets, &manifest)?;

    // ── fan out: stream every object to its home, hashing as we write ─────
    // Unique per invocation (pid + process-local counter + manifest id) so
    // concurrent restores — including parallel tests that reuse the same
    // manifest id — never share a scratch dir.
    let work = std::env::temp_dir().join(format!(
        "bichon-restore-{}-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed),
        manifest
            .id
            .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_")
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| {
        raise_error!(
            format!("restore: cannot create work dir {}: {e}", work.display()),
            ErrorCode::InternalError
        )
    })?;

    let result = restore_into(
        &manifest,
        &targets,
        &work,
        engine,
        opts.live_root.as_deref(),
        &mut report,
    )
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result?;

    // Invalidate every IMAP client cache for the restored accounts. A restore
    // point is internally consistent, so the restored counter/index pair looks
    // healthy while clients still hold UIDs from the generation they last saw —
    // bumping the UIDVALIDITY generation is the only way to make them resync
    // instead of trusting a cache that no longer matches. See
    // `archive::imap::stable_uid::bump_all_generations`.
    if report.memdb_restored {
        let memdb_dir = targets.root.join(MEMDB_DIR);
        match bichon_memdb::MemDb::open_with(&memdb_dir, bichon_memdb::Durability::Full)
            .map_err(|e| {
                raise_error!(
                    format!("restore: cannot open the restored memdb: {e}"),
                    ErrorCode::InternalError
                )
            })
            .and_then(|db| crate::archive::imap::stable_uid::bump_all_generations(&db))
        {
            Ok(bumped) => {
                report.generations_bumped = bumped;
                if bumped > 0 {
                    warn!(
                        accounts = bumped,
                        "restore: bumped the IMAP UIDVALIDITY generation of every account so \
                         clients resynchronize instead of trusting stale UID caches"
                    );
                }
            }
            Err(e) => {
                // Not fatal: the server's startup reconcile repairs a counter
                // that ends up behind the index, and a client that keeps a
                // stale cache is a correctness problem the operator can still
                // resolve by removing the account's cache. Refusing an
                // otherwise-complete restore would be worse.
                warn!("restore: could not invalidate IMAP client caches: {e:#?}");
            }
        }
    }

    // The commit marker: without STORAGE_VERSION, `check_data_status` treats
    // the directory as a fresh (empty) install and `initialize()` would
    // refuse to touch existing data.
    write_storage_version(&targets.root, CURRENT_STORAGE_VERSION).map_err(|e| {
        raise_error!(
            format!("restore: cannot stamp STORAGE_VERSION: {e}"),
            ErrorCode::InternalError
        )
    })?;

    info!(
        "restore: restore point {} applied to {} ({} objects, {} bytes)",
        manifest.id,
        targets.root.display(),
        report.objects_total,
        report.bytes_total
    );
    Ok(report)
}

/// Apply one manifest's objects to the resolved target directories. Separated
/// so failures propagate through the work-dir cleanup.
/// Deployment identity package: the `instance-id` licenses are bound to, the
/// license file when one exists, and the license-state ledger (trial start /
/// "a license was installed"). These are small files a disaster recovery must
/// preserve together — restoring the data without the id makes every license
/// fail with `MachineMismatch`, and dropping the ledger would restart a
/// trial.
#[derive(Debug, serde::Deserialize)]
struct DeploymentIdentityPackage {
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    license_state: Option<serde_json::Value>,
}

/// Pick the license to restore.
///
/// The restore point carries the license as it was at backup time. Restoring
/// an old point after a renewal would therefore regress `upd` and could fail
/// the update-coverage preflight on the current binary. When the restore
/// target belongs to the *same* deployment as the live install (same
/// `instance-id`), the live license is at least as current, so it wins.
/// Restoring onto a different deployment keeps the packaged license — it is
/// the only one bound to the restored `instance-id`.
/// A cheap shape check for "this is a license JWT, not an empty or garbage
/// file". Format validation belongs to `bichon-license`, which the community
/// core cannot depend on.
fn looks_like_jwt(raw: &str) -> bool {
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    compact.matches('.').count() == 2 && !compact.starts_with('.') && !compact.ends_with('.')
}

fn choose_license(
    packaged: Option<&str>,
    packaged_instance_id: Option<&str>,
    live_root: Option<&Path>,
) -> Option<String> {
    let packaged = packaged.map(str::trim).filter(|s| !s.is_empty());
    let Some(live_root) = live_root else {
        return packaged.map(str::to_string);
    };

    let live_id = std::fs::read_to_string(live_root.join("instance-id")).ok();
    let same_deployment = match (
        packaged_instance_id.map(str::trim),
        live_id.as_deref().map(str::trim),
    ) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    if same_deployment {
        if let Ok(live) = std::fs::read_to_string(live_root.join("license.txt")) {
            let live = live.trim();
            if looks_like_jwt(live) && Some(live) != packaged {
                info!(
                    "restore: keeping the deployment's current license instead of the one \
                     captured in the restore point"
                );
                return Some(live.to_string());
            }
        }
    }
    packaged.map(str::to_string)
}

async fn restore_deployment_identity(
    obj: &ObjectRef,
    targets: &RestoreTargets,
    work: &Path,
    engine: &BackupEngine,
    live_root: Option<&Path>,
    report: &mut RestoreReport,
) -> BichonResult<()> {
    let package = work.join("deployment.json");
    download(engine, obj, &package, report).await?;
    let raw = std::fs::read_to_string(&package).map_err(|e| {
        raise_error!(
            format!(
                "restore: cannot read deployment identity {}: {e}",
                package.display()
            ),
            ErrorCode::InternalError
        )
    })?;
    let identity: DeploymentIdentityPackage = serde_json::from_str(&raw).map_err(|e| {
        raise_error!(
            format!("restore: invalid deployment identity package: {e}"),
            ErrorCode::InternalError
        )
    })?;

    if let Some(instance_id) = identity.instance_id.as_deref() {
        let id = instance_id.trim();
        if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            std::fs::write(targets.root.join("instance-id"), id).map_err(|e| {
                raise_error!(
                    format!("restore: cannot write instance-id: {e}"),
                    ErrorCode::InternalError
                )
            })?;
            report.instance_id_restored = true;
        } else {
            // Never write a malformed id: a new one generated on first start
            // is at least a coherent deployment, while a broken file would
            // make every license check fail.
            warn!("restore: deployment identity contains a malformed instance-id; skipped");
        }
    }
    if let Some(license) = choose_license(
        identity.license.as_deref(),
        identity.instance_id.as_deref(),
        live_root,
    ) {
        std::fs::write(targets.root.join("license.txt"), license).map_err(|e| {
            raise_error!(
                format!("restore: cannot write license.txt: {e}"),
                ErrorCode::InternalError
            )
        })?;
        report.license_restored = true;
    }
    if let Some(state) = identity.license_state.as_ref().filter(|v| v.is_object()) {
        let raw = serde_json::to_vec_pretty(state).map_err(|e| {
            raise_error!(
                format!("restore: cannot encode license state: {e}"),
                ErrorCode::InternalError
            )
        })?;
        std::fs::write(targets.root.join("license-state.json"), raw).map_err(|e| {
            raise_error!(
                format!("restore: cannot write license-state.json: {e}"),
                ErrorCode::InternalError
            )
        })?;
        report.license_state_restored = true;
    } else if matches!(&identity.license_state, Some(v) if !v.is_null()) {
        warn!("restore: deployment identity contains a non-object license state; skipped");
    }
    Ok(())
}

async fn restore_into(
    manifest: &Manifest,
    targets: &RestoreTargets,
    work: &Path,
    engine: &BackupEngine,
    live_root: Option<&Path>,
    report: &mut RestoreReport,
) -> BichonResult<()> {
    // Deployment identity first: it is tiny and determines whether the
    // restored data set keeps its license.
    if let Some(obj) = &manifest.objects.deployment {
        restore_deployment_identity(obj, targets, work, engine, live_root, report).await?;
    }

    // memdb: one gzip snapshot streamed from memory at capture time. R7
    // guarantees the target is a fresh, empty dir, so there is no stale WAL
    // to clear.
    if let Some(obj) = &manifest.objects.memdb {
        let gz = work.join("snapshot.json.gz");
        download(engine, obj, &gz, report).await?;
        let memdb_dir = targets.root.join(MEMDB_DIR);
        std::fs::create_dir_all(&memdb_dir).map_err(|e| {
            raise_error!(
                format!("restore: cannot create {}: {e}", memdb_dir.display()),
                ErrorCode::InternalError
            )
        })?;
        let mut dec = flate2::read::GzDecoder::new(std::fs::File::open(&gz).map_err(|e| {
            raise_error!(
                format!("restore: cannot open {}: {e}", gz.display()),
                ErrorCode::InternalError
            )
        })?);
        let dest = memdb_dir.join("snapshot.json");
        let mut out = std::fs::File::create(&dest).map_err(|e| {
            raise_error!(
                format!("restore: cannot create {}: {e}", dest.display()),
                ErrorCode::InternalError
            )
        })?;
        std::io::copy(&mut dec, &mut out).map_err(|e| {
            raise_error!(
                format!(
                    "restore: cannot unpack memdb snapshot {}: {e}",
                    gz.display()
                ),
                ErrorCode::InternalError
            )
        })?;
        report.memdb_restored = true;
    }

    // blob: sealed segments, back in the layout the engine discovers on open.
    for seg in &manifest.objects.blob_segments {
        let dest = targets
            .data
            .join(BLOBS_DIR)
            .join(SEGMENTS_DIR)
            .join(format!("{:08}.seg", seg.seg_id));
        download(engine, &seg.obj, &dest, report).await?;
        report.segments_restored += 1;
    }

    // audit: baseline (if this manifest carries one) + ordered deltas.
    if let Some(audit) = &manifest.objects.audit {
        let audit_dir = targets.root.join(AUDIT_DIR);
        std::fs::create_dir_all(&audit_dir).map_err(|e| {
            raise_error!(
                format!("restore: cannot create {}: {e}", audit_dir.display()),
                ErrorCode::InternalError
            )
        })?;
        let db_path = audit_dir.join(AUDIT_DB_FILE);
        if !audit.base.key.is_empty() {
            download(engine, &audit.base, &db_path, report).await?;
            report.audit_restored = true;
        }
        let replayer = AUDIT_REPLAYERS.lock().unwrap().first().cloned();
        match replayer {
            Some(rp) => {
                for d in &audit.deltas {
                    let package = work.join(format!("audit-{}.jsonl.gz", d.seq_from));
                    download(engine, &d.obj, &package, report).await?;
                    rp.apply_delta(&package, &db_path, d.seq_from, d.seq_to)?;
                    report.audit_deltas_replayed += 1;
                }
                rp.apply_retention(&db_path)?;
                info!("restore: audit chain replayed via {}", rp.name());
            }
            None => {
                // No edition replayer: never drop the data (R11) — copy the
                // deltas verbatim so an operator can replay them by hand.
                for d in &audit.deltas {
                    let dest = audit_dir
                        .join(AUDIT_DELTAS_DIR)
                        .join(format!("delta-{}-{}.jsonl.gz", d.seq_from, d.seq_to));
                    download(engine, &d.obj, &dest, report).await?;
                    report.audit_deltas_copied += 1;
                }
                if !audit.deltas.is_empty() {
                    warn!(
                        "restore: no audit replayer registered — {} delta(s) copied to {}",
                        audit.deltas.len(),
                        audit_dir.join(AUDIT_DELTAS_DIR).display()
                    );
                }
            }
        }
    }

    // integrity + timestamp: single-file databases.
    if let Some(obj) = &manifest.objects.integrity {
        let dest = targets.root.join(INTEGRITY_DIR).join(INTEGRITY_DB_FILE);
        download(engine, obj, &dest, report).await?;
        report.integrity_restored = true;
    }
    if let Some(obj) = &manifest.objects.timestamp {
        let dest = targets.root.join(TIMESTAMP_DIR).join(ANCHOR_DB_FILE);
        download(engine, obj, &dest, report).await?;
        report.timestamp_restored = true;
    }

    // tantivy: index copies. When absent (older restore points, pre-index
    // backups) the
    // search index is not restored, and the report says so (R11).
    let mut restored_names = Vec::new();
    for t in &manifest.objects.tantivy {
        let sub = match t.name.as_str() {
            "envelope" => ENVELOPE_INDEX_DIR,
            "attachment" => ATTACHMENT_INDEX_DIR,
            other => {
                warn!("restore: unknown index copy {other:?} skipped");
                continue;
            }
        };
        // The index copy is packaged (gzip JSON map).
        let package = work.join(format!("index-{}.json.gz", t.name));
        download(engine, &t.obj, &package, report).await?;
        unpackage_dir(&package, &targets.index.join(sub))?;
        restored_names.push(t.name.clone());
        report.tantivy_restored += 1;
    }
    if restored_names.is_empty() {
        report.note = Some(
            "this restore point carries no tantivy index copies — the search index is not restored"
                .to_string(),
        );
    } else if !restored_names.contains(&"envelope".to_string())
        || !restored_names.contains(&"attachment".to_string())
    {
        report.note = Some(format!(
            "index copies restored: {}; a full search rebuild is recommended for consistency",
            restored_names.join(", ")
        ));
    }

    Ok(())
}

/// Stream one object to `dest` (verifying hash + size) and count it.
async fn download(
    engine: &BackupEngine,
    obj: &ObjectRef,
    dest: &Path,
    report: &mut RestoreReport,
) -> BichonResult<()> {
    engine
        .download_object(&obj.key, dest, &obj.sha256, obj.bytes)
        .await?;
    report.bytes_checked += obj.bytes;
    Ok(())
}

/// Every `(key, sha256, bytes)` reference of a manifest, in stable order.
fn manifest_refs(m: &Manifest) -> Vec<(&str, &str, u64)> {
    let mut out = Vec::new();
    for slot in [
        &m.objects.memdb,
        &m.objects.integrity,
        &m.objects.timestamp,
        &m.objects.deployment,
    ]
    .into_iter()
    .flatten()
    {
        out.push((slot.key.as_str(), slot.sha256.as_str(), slot.bytes));
    }
    for seg in &m.objects.blob_segments {
        out.push((seg.obj.key.as_str(), seg.obj.sha256.as_str(), seg.obj.bytes));
    }
    if let Some(audit) = &m.objects.audit {
        if !audit.base.key.is_empty() {
            out.push((
                audit.base.key.as_str(),
                audit.base.sha256.as_str(),
                audit.base.bytes,
            ));
        }
        for d in &audit.deltas {
            out.push((d.obj.key.as_str(), d.obj.sha256.as_str(), d.obj.bytes));
        }
    }
    for t in &m.objects.tantivy {
        out.push((t.obj.key.as_str(), t.obj.sha256.as_str(), t.obj.bytes));
    }
    out
}

// ── path safety helpers (Windows-tolerant) ─────────────────────────────────
//
// `paths_equal` / `path_contains` / `normalize` live in `backup::path` — the
// local backup target needs the same containment test, and a second copy
// would drift on the Windows verbatim-path handling.

/// R7: a restore writes only into a directory that is empty and neither
/// contains nor is contained by the live data root.
fn assert_clean_restore_target(dir: &Path, root: &Path) -> BichonResult<()> {
    // An empty root is the "no live data root configured" sentinel: restore
    // runs from `bichon-admin` on a fresh box that legitimately has nothing
    // to protect against. Only the live-root comparisons are skipped —
    // comparing against `""` would canonicalize to the current directory
    // (Windows) and either false-reject every target or, worse, treat cwd as
    // the protected root. The emptiness check below always applies.
    if !root.as_os_str().is_empty() {
        if paths_equal(dir, root) {
            return Err(raise_error!(
                format!(
                    "restore: refusing to restore into the live data root {} (R7)",
                    root.display()
                ),
                ErrorCode::InvalidParameter
            ));
        }
        if path_contains(dir, root) || path_contains(root, dir) {
            return Err(raise_error!(
                format!(
                    "restore: target {} and data root {} must be disjoint (R7)",
                    dir.display(),
                    root.display()
                ),
                ErrorCode::InvalidParameter
            ));
        }
    }
    if dir.exists() {
        let mut entries = std::fs::read_dir(dir).map_err(|e| {
            raise_error!(
                format!("restore: cannot read target {}: {e}", dir.display()),
                ErrorCode::InternalError
            )
        })?;
        if entries.next().is_some() {
            return Err(raise_error!(
                format!(
                    "restore: target {} is not empty (R7: a restore writes only into an empty directory)",
                    dir.display()
                ),
                ErrorCode::InvalidParameter
            ));
        }
    }
    Ok(())
}

/// Capacity check: each target's filesystem must have room for the objects
/// that will land on it (with a safety margin), or the restore is refused
/// before any bytes are written. Root gets everything except blob segments
/// (→ data target) and tantivy copies (→ index target).
fn assert_sufficient_space(targets: &RestoreTargets, m: &Manifest) -> BichonResult<()> {
    let root_needed = m.objects.memdb.as_ref().map_or(0, |o| o.bytes)
        + m.objects.integrity.as_ref().map_or(0, |o| o.bytes)
        + m.objects.timestamp.as_ref().map_or(0, |o| o.bytes)
        + m.objects.deployment.as_ref().map_or(0, |o| o.bytes)
        + m.objects.audit.as_ref().map_or(0, |a| {
            a.base.bytes + a.deltas.iter().map(|d| d.obj.bytes).sum::<u64>()
        });
    let data_needed: u64 = m.objects.blob_segments.iter().map(|s| s.obj.bytes).sum();
    let index_needed: u64 = m.objects.tantivy.iter().map(|t| t.obj.bytes).sum();
    for (name, dir, needed) in [
        ("root", &targets.root, root_needed),
        ("blob data", &targets.data, data_needed),
        ("index", &targets.index, index_needed),
    ] {
        check_free_space("restore", name, dir, needed)?;
    }
    Ok(())
}

/// Human-readable CLI report for a restore run. Shared by the community and
/// Pro binaries so both print the same honest picture (R11).
pub fn format_restore_report(report: &RestoreReport) -> String {
    fn yes(b: bool) -> &'static str {
        if b {
            "yes"
        } else {
            "no"
        }
    }
    let mut out = String::new();
    out.push_str("\nRestore report\n==============\n");
    out.push_str(&format!(
        "  manifest        : {} (trigger: {})\n",
        report.manifest_id, report.trigger
    ));
    out.push_str(&format!("  created         : {}\n", report.created));
    out.push_str(&format!(
        "  mode            : {}\n",
        if report.verify_only {
            "verify-only (drill, nothing written)"
        } else {
            "restore"
        }
    ));
    if report.verify_only {
        out.push_str(&format!(
            "  verified        : {} object(s), {} byte(s) hashed and matched\n",
            report.objects_total, report.bytes_checked
        ));
    } else {
        out.push_str(&format!("  target          : {}\n", report.into));
        out.push_str(&format!(
            "  objects         : {} ({} bytes) downloaded and verified\n",
            report.objects_total, report.bytes_total
        ));
        out.push_str(&format!(
            "  memdb           : {}\n",
            yes(report.memdb_restored)
        ));
        out.push_str(&format!(
            "  blob segments   : {}\n",
            report.segments_restored
        ));
        out.push_str(&format!(
            "  audit           : {}\n",
            yes(report.audit_restored)
        ));
        if report.audit_deltas_replayed > 0 {
            out.push_str(&format!(
                "  audit deltas    : {} replayed into audit.db\n",
                report.audit_deltas_replayed
            ));
        }
        if report.audit_deltas_copied > 0 {
            out.push_str(&format!(
                "  audit deltas    : {} copied verbatim (no replayer registered)\n",
                report.audit_deltas_copied
            ));
        }
        out.push_str(&format!(
            "  integrity       : {}\n",
            yes(report.integrity_restored)
        ));
        out.push_str(&format!(
            "  timestamp anchor: {}\n",
            yes(report.timestamp_restored)
        ));
        if report.instance_id_restored || report.license_restored || report.license_state_restored
        {
            out.push_str(&format!(
                "  deployment id   : instance-id {}, license {}, license state {}\n",
                yes(report.instance_id_restored),
                yes(report.license_restored),
                yes(report.license_state_restored)
            ));
        }
        out.push_str(&format!(
            "  search indices  : {} (from the point)\n",
            report.tantivy_restored
        ));
    }
    if let Some(note) = &report.note {
        out.push('\n');
        out.push_str(&format!("  NOTE: {note}\n"));
    }
    if !report.verify_only {
        // Switch-over guidance (design doc §8): restore always targets a
        // fresh directory, never the live data root (R7).
        out.push('\n');
        out.push_str("  To switch over:\n");
        out.push_str("    1. Stop the old server (it must not write while you swap).\n");
        out.push_str("    2. Point BICHON_ROOT_DIR at the restored directory:\n");
        out.push_str(&format!("       BICHON_ROOT_DIR=\"{}\"\n", report.into));
        out.push_str("    3. Start the server again; the blob engine rebuilds its\n");
        out.push_str("       internal index from the restored segments on open.\n");
        out.push_str("    4. Verify search.\n");
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::manifest::{
        AuditDelta, AuditObjects, ManifestObjects, SegmentRef, TantivyRef,
    };
    use crate::settings::cli::SETTINGS;
    use std::sync::Once;

    /// The restore path-safety guards read the data root via
    /// `SETTINGS.root_dir()`; on the test binary no subcommand is present so
    /// clap requires it. Set it once, to a directory that is disjoint from
    /// every test's `into` target (each test uses its own
    /// `bichon-restore-test-*` sibling).
    static TEST_ENV: Once = Once::new();
    /// The two tests that mutate the process-global AUDIT_REPLAYERS registry
    /// cannot run in parallel (same pattern as the config tests).
    static RUN_REPLAY_TESTS: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));
    fn init_test_env() {
        TEST_ENV.call_once(|| {
            std::env::set_var(
                "BICHON_ROOT_DIR",
                std::env::temp_dir().join(format!("bichon-core-test-{}", std::process::id())),
            );
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
        });
    }

    /// Per-test scratch dir (tests run in parallel, so no two may share one).
    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bichon-restore-test-{}-{name}", std::process::id()))
    }

    /// Upload a packaged directory (gzip JSON map) and return its key — the
    /// tantivy slot must be a real package or unpackage_dir fails.
    async fn package_object(
        engine: &BackupEngine,
        root: &Path,
        name: &str,
        files: &[(&str, &[u8])],
    ) -> String {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, data) in files {
            std::fs::write(dir.join(f), data).unwrap();
        }
        let pkg = root.join(format!("{name}.json.gz"));
        crate::backup::prepare::package_dir(&dir, &pkg).unwrap();
        engine
            .upload_bytes_for_test(&std::fs::read(&pkg).unwrap())
            .await
            .unwrap()
    }

    /// A manifest with one object per slot: the envelope index is a real
    /// package, memdb is a real gzip snapshot, everything else points at a
    /// single "hello" object so a round trip needs only a few uploads.
    async fn sample_manifest(engine: &BackupEngine, root: &Path) -> Manifest {
        let hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let index = package_object(engine, root, "index", &[("meta.json", b"{}")]).await;
        // memdb artifact = gzip of the raw snapshot bytes, so the restored
        // `memdb/snapshot.json` must come out exactly "hello".
        let memdb_gz = {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut enc, b"hello").unwrap();
            enc.finish().unwrap()
        };
        let memdb = engine.upload_bytes_for_test(&memdb_gz).await.unwrap();
        let obj = crate::backup::manifest::ObjectRef {
            key: hello.clone(),
            sha256: hex::encode(ring::digest::digest(&ring::digest::SHA256, b"hello").as_ref()),
            bytes: 5,
        };
        let mut m = Manifest::new("m-test-1".to_string(), "manual", None);
        m.objects = ManifestObjects {
            memdb: Some(crate::backup::manifest::ObjectRef {
                key: memdb.clone(),
                sha256: hex::encode(
                    ring::digest::digest(&ring::digest::SHA256, &memdb_gz).as_ref(),
                ),
                bytes: memdb_gz.len() as u64,
            }),
            blob_segments: vec![SegmentRef {
                seg_id: 3,
                obj: obj.clone(),
            }],
            audit: Some(AuditObjects {
                base: obj.clone(),
                deltas: vec![
                    AuditDelta {
                        seq_from: 0,
                        seq_to: 10,
                        obj: obj.clone(),
                    },
                    AuditDelta {
                        seq_from: 10,
                        seq_to: 20,
                        obj: obj.clone(),
                    },
                ],
                cursor: 20,
                cumulative_delta_bytes: 0,
            }),
            integrity: Some(obj.clone()),
            timestamp: Some(obj.clone()),
            deployment: None,
            tantivy: vec![TantivyRef {
                name: "envelope".to_string(),
                obj: crate::backup::manifest::ObjectRef {
                    key: index.clone(),
                    sha256: hex::encode(
                        ring::digest::digest(
                            &ring::digest::SHA256,
                            &std::fs::read(root.join("index.json.gz")).unwrap(),
                        )
                        .as_ref(),
                    ),
                    bytes: std::fs::metadata(root.join("index.json.gz")).unwrap().len(),
                },
            }],
        };
        m
    }

    /// A fake replayer: appends each package's raw bytes to a marker file and
    /// counts apply_retention calls.
    struct FakeReplayer {
        marker: PathBuf,
    }
    impl AuditReplayer for FakeReplayer {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn apply_delta(
            &self,
            package: &Path,
            _db: &Path,
            seq_from: i64,
            seq_to: i64,
        ) -> BichonResult<()> {
            let data = std::fs::read(package).unwrap();
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.marker)
                .unwrap();
            use std::io::Write;
            writeln!(f, "{seq_from}..{seq_to}:{}B", data.len()).unwrap();
            Ok(())
        }
        fn apply_retention(&self, _db: &Path) -> BichonResult<()> {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.marker)
                .unwrap();
            use std::io::Write;
            writeln!(f, "retention").unwrap();
            Ok(())
        }
    }

    #[test]
    fn parse_s3_uri_variants() {
        let (e, b, p) = parse_s3_uri("s3://localhost:9000/bichon-backups/prod").unwrap();
        assert_eq!(e, "localhost:9000");
        assert_eq!(b, "bichon-backups");
        assert_eq!(p, "prod");

        // No prefix → default.
        let (_, b, p) = parse_s3_uri("s3://minio.internal/bichon").unwrap();
        assert_eq!(b, "bichon");
        assert_eq!(p, DEFAULT_PREFIX);

        // Trailing slashes are trimmed.
        let (_, _, p) = parse_s3_uri("s3://h:9000/b/prod///").unwrap();
        assert_eq!(p, "prod");

        // MinIO/R2/Wasabi-style endpoints carry their scheme; it must not
        // confuse the endpoint/bucket split (and the engine needs the scheme
        // to permit plain http).
        let (e, b, p) = parse_s3_uri("s3://http://localhost:9000/bichon/bichon-backup").unwrap();
        assert_eq!(e, "http://localhost:9000");
        assert_eq!(b, "bichon");
        assert_eq!(p, "bichon-backup");
        let (e, b, _) = parse_s3_uri("s3://https://s3.example.com/bucket/prefix").unwrap();
        assert_eq!(e, "https://s3.example.com");
        assert_eq!(b, "bucket");

        // Empty endpoint = AWS (the SDK default endpoint, no override).
        let (e, b, p) = parse_s3_uri("s3:///bichon/bichon-backup").unwrap();
        assert_eq!(e, "");
        assert_eq!(b, "bichon");
        assert_eq!(p, "bichon-backup");

        assert!(parse_s3_uri("http://h/b").is_err());
        assert!(parse_s3_uri("s3://h").is_err());
    }

    #[test]
    fn normalize_point_variants() {
        // Keyword / blank / absent → the newest point (no explicit id).
        assert!(normalize_point(None).is_none());
        assert!(normalize_point(Some("")).is_none());
        assert!(normalize_point(Some("   ")).is_none());
        assert!(normalize_point(Some("LATEST")).is_none());
        assert!(normalize_point(Some("latest")).is_none());

        // Bare id passes through untouched.
        assert_eq!(
            normalize_point(Some("0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")).as_deref(),
            Some("0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")
        );

        // A name pasted from the bucket listing loses its `m-` / `.json.gz`,
        // so load_by_id does not look for `m-m-<id>.json.gz`.
        assert_eq!(
            normalize_point(Some("m-0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")).as_deref(),
            Some("0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")
        );
        assert_eq!(
            normalize_point(Some("m-0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.json.gz")).as_deref(),
            Some("0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")
        );
        // Surrounding whitespace is tolerated.
        assert_eq!(normalize_point(Some("  m-abc  ")).as_deref(), Some("abc"));
    }

    /// A restore into the same deployment keeps the current (possibly
    /// renewed) license instead of the older one captured in the point; a
    /// different deployment keeps the packaged license, the only one bound to
    /// the restored instance-id.
    #[test]
    fn choose_license_prefers_the_live_license_only_for_the_same_deployment() {
        let root = test_root("choose-license");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let id = "a".repeat(64);
        std::fs::write(root.join("instance-id"), &id).unwrap();
        std::fs::write(root.join("license.txt"), "live.header.sig").unwrap();

        // Same deployment: live wins.
        assert_eq!(
            choose_license(Some("packaged.header.sig"), Some(&id), Some(&root)).as_deref(),
            Some("live.header.sig")
        );
        // Different deployment: packaged wins.
        let other = "b".repeat(64);
        assert_eq!(
            choose_license(Some("packaged.header.sig"), Some(&other), Some(&root)).as_deref(),
            Some("packaged.header.sig")
        );
        // A malformed live file is ignored.
        std::fs::write(root.join("license.txt"), "not-a-jwt").unwrap();
        assert_eq!(
            choose_license(Some("packaged.header.sig"), Some(&id), Some(&root)).as_deref(),
            Some("packaged.header.sig")
        );
        // No live root (fresh DR box) or no packaged license: packaged stays.
        assert_eq!(
            choose_license(Some("packaged.header.sig"), Some(&id), None).as_deref(),
            Some("packaged.header.sig")
        );
        assert_eq!(choose_license(None, Some(&id), Some(&root)), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deployment identity restores `instance-id` and `license.txt` verbatim,
    /// so the existing license still binds to the restored data root.
    #[tokio::test]
    async fn deployment_identity_restores_instance_id_and_license() {
        init_test_env();
        let root = test_root("deployment");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = BackupEngine::local_store_for_test(&root.join("store")).unwrap();
        let engine = BackupEngine::new(store, "test");

        let instance_id = "a".repeat(64);
        let package = serde_json::json!({
            "instance_id": instance_id,
            "license": "-----BEGIN BICHON LICENSE-----\njwt.payload.sig\n-----END BICHON LICENSE-----",
            "license_state": {
                "license_installed": true,
                "trial_started_at": 1_700_000_000_i64,
                "last_seen_at": 1_700_086_400_i64,
            },
        });
        let raw = serde_json::to_vec(&package).unwrap();
        let key = engine.upload_bytes_for_test(&raw).await.unwrap();
        let obj = ObjectRef {
            key,
            sha256: hex::encode(ring::digest::digest(&ring::digest::SHA256, &raw).as_ref()),
            bytes: raw.len() as u64,
        };

        let target = root.join("restored");
        std::fs::create_dir_all(&target).unwrap();
        let targets = RestoreTargets {
            root: target.clone(),
            index: target.join(INDICES_DIR),
            data: target.join(STORAGE_DIR),
        };
        let mut report = RestoreReport::default();
        restore_deployment_identity(&obj, &targets, &root, &engine, None, &mut report)
            .await
            .unwrap();

        assert!(report.instance_id_restored);
        assert!(report.license_restored);
        assert!(report.license_state_restored);
        assert_eq!(
            std::fs::read_to_string(target.join("instance-id")).unwrap(),
            instance_id
        );
        assert!(std::fs::read_to_string(target.join("license.txt"))
            .unwrap()
            .contains("BICHON LICENSE"));
        let state = std::fs::read_to_string(target.join("license-state.json")).unwrap();
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state["license_installed"], true);
        assert_eq!(state["trial_started_at"], 1_700_000_000_i64);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn full_round_trip_with_fake_replayer() {
        init_test_env();
        let _run_guard = RUN_REPLAY_TESTS.lock().await;
        let root = test_root("roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored");
        let marker = root.join("replay.log");

        // Build an engine over a local store and commit a full manifest.
        let store_dir = root.join("store");
        let store = BackupEngine::local_store_for_test(&store_dir).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine
            .upload_bytes_for_test(b"hello")
            .await
            .expect("upload test object");
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        // Register the fake replayer for this test.
        {
            let mut guard = AUDIT_REPLAYERS.lock().unwrap();
            guard.clear();
            guard.push(Arc::new(FakeReplayer {
                marker: marker.clone(),
            }));
        }

        let opts = RestoreOptions {
            s3_uri: format!("s3://ignored/{}/test", store_dir.display()),
            point: None,
            into: into.clone(),
            index_dir: None,
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let report = restore_with_engine(&opts, &engine).await.unwrap();

        assert!(report.memdb_restored);
        assert_eq!(report.segments_restored, 1);
        assert!(report.audit_restored);
        assert_eq!(report.audit_deltas_replayed, 2);
        assert_eq!(report.audit_deltas_copied, 0);
        assert!(report.integrity_restored);
        assert!(report.timestamp_restored);
        assert_eq!(report.tantivy_restored, 1);

        // On-disk layout matches the production mapping.
        assert_eq!(
            std::fs::read(into.join("memdb/snapshot.json")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read(into.join("bichon-storage/blobs/segments/00000003.seg")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read(into.join("audit/audit.db")).unwrap(),
            b"hello"
        );
        // The replayer ran for both deltas + retention.
        let log = std::fs::read_to_string(&marker).unwrap();
        assert!(log.contains("0..10:5B"));
        assert!(log.contains("10..20:5B"));
        assert!(log.contains("retention"));
        // STORAGE_VERSION stamped (CURRENT = 2).
        assert_eq!(
            std::fs::read_to_string(into.join("STORAGE_VERSION"))
                .unwrap()
                .trim(),
            "2"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_targets_resolve_default_and_explicit() {
        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: PathBuf::from("R"),
            index_dir: None,
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        // Default layout: indices and blob store live under the target root.
        let t = RestoreTargets::resolve(&opts);
        assert_eq!(t.root, PathBuf::from("R"));
        assert_eq!(t.index, PathBuf::from("R").join(INDICES_DIR));
        assert_eq!(t.data, PathBuf::from("R").join(STORAGE_DIR));

        // Explicit parent dirs mirror the startup params (append the subdir).
        let opts = RestoreOptions {
            into: PathBuf::from("R"),
            index_dir: Some(PathBuf::from("I")),
            data_dir: Some(PathBuf::from("D")),
            ..opts
        };
        let t = RestoreTargets::resolve(&opts);
        assert_eq!(t.index, PathBuf::from("I").join(INDICES_DIR));
        assert_eq!(t.data, PathBuf::from("D").join(STORAGE_DIR));
    }

    #[tokio::test]
    async fn restore_respects_separate_index_and_data_dirs() {
        init_test_env();
        // This test restores an audit-bearing manifest, so it must serialize
        // with the replayer tests (RUN_REPLAY_TESTS) and pin the registry to
        // empty — otherwise it can run concurrently with
        // `full_round_trip_with_fake_replayer`, hijack its registered
        // FakeReplayer, and both tests append to the same marker file
        // (concurrent appends from two handles corrupt the file on Windows).
        let _run_guard = RUN_REPLAY_TESTS.lock().await;
        let root = test_root("separate-dirs");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut guard = AUDIT_REPLAYERS.lock().unwrap();
            guard.clear();
        }
        let into = root.join("restored");
        let index_parent = root.join("fresh-index");
        let data_parent = root.join("fresh-data");

        let store = BackupEngine::local_store_for_test(&root.join("store")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: Some(index_parent.clone()),
            data_dir: Some(data_parent.clone()),
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let report = restore_with_engine(&opts, &engine).await.unwrap();
        assert!(report.memdb_restored);
        assert_eq!(report.segments_restored, 1);
        assert_eq!(report.tantivy_restored, 1);

        // Root-owned data lands under `into`...
        assert_eq!(
            std::fs::read(into.join("memdb/snapshot.json")).unwrap(),
            b"hello"
        );
        // ...blob segments land under the data parent, indices under the index
        // parent, and neither leaks into `into`.
        assert_eq!(
            std::fs::read(data_parent.join("bichon-storage/blobs/segments/00000003.seg")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read(index_parent.join("bichon-indices/mail_metadata/meta.json")).unwrap(),
            b"{}"
        );
        assert!(!into.join("bichon-storage").exists());
        assert!(!into.join("bichon-indices").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn r7_refuses_index_target_inside_live_root() {
        init_test_env();
        let root = test_root("idx-inside-root");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored");

        let store = BackupEngine::local_store_for_test(&root.join("store")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        // index_dir pointing at the live data root → resolved index target is
        // inside the live root → refused (R7).
        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: Some(PathBuf::from(SETTINGS.root_dir())),
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: Some(PathBuf::from(SETTINGS.root_dir())),
            ..Default::default()
        };

        let err = restore_with_engine(&opts, &engine).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("disjoint"), "{msg}");
        assert!(msg.contains("R7"), "{msg}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn r7_refuses_non_empty_data_target() {
        init_test_env();
        let root = test_root("data-not-empty");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored");
        let data_parent = root.join("fresh-data");
        // The R7 check is on the *resolved* blob target — the `bichon-storage`
        // subdir under the parent — so seed the leftover there.
        std::fs::create_dir_all(data_parent.join(STORAGE_DIR)).unwrap();
        std::fs::write(data_parent.join(STORAGE_DIR).join("leftover"), "not-empty").unwrap();

        let store = BackupEngine::local_store_for_test(&root.join("store")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: None,
            data_dir: Some(data_parent.clone()),
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let err = restore_with_engine(&opts, &engine).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("not empty"), "{msg}");
        assert!(msg.contains("R7"), "{msg}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn verify_only_writes_nothing() {
        init_test_env();
        let root = test_root("verify");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored-verify");

        let store = BackupEngine::local_store_for_test(&root.join("store2")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: None,
            data_dir: None,
            verify_only: true,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let report = restore_with_engine(&opts, &engine).await.unwrap();
        assert!(report.verify_only);
        assert_eq!(report.objects_total, 8);
        assert!(
            report.bytes_checked > 35,
            "at least the 7 hello-sized slots"
        );
        assert!(!into.exists(), "verify-only must not create the target");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn r7_refuses_non_empty_target() {
        init_test_env();
        let root = test_root("nonempty");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored-full");
        std::fs::create_dir_all(into.join("something")).unwrap();

        let store = BackupEngine::local_store_for_test(&root.join("store3")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: None,
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let err = restore_with_engine(&opts, &engine).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("not empty"), "{msg}");
        assert!(msg.contains("R7"), "{msg}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn r7_refuses_live_data_root() {
        init_test_env();
        let root = test_root("liveroot");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = BackupEngine::local_store_for_test(&root.join("store4")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            // Equal to the live root (the BICHON_ROOT_DIR env value from init).
            into: PathBuf::from(SETTINGS.root_dir()),
            index_dir: None,
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: Some(PathBuf::from(SETTINGS.root_dir())),
            ..Default::default()
        };

        let err = restore_with_engine(&opts, &engine).await.unwrap_err();
        assert!(format!("{err:?}").contains("live data root"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn unpackage_rejects_path_traversal() {
        let root = test_root("traversal");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Craft a package whose entry tries to escape the destination.
        let package = root.join("evil.json.gz");
        let map = serde_json::json!({ "../escape.txt": "aGVsbG8=" });
        let json = serde_json::to_vec(&map).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &json).unwrap();
        let bytes = enc.finish().unwrap();
        std::fs::write(&package, bytes).unwrap();

        let dest = root.join("unpack-dest");
        let err = unpackage_dir(&package, &dest).unwrap_err();
        assert!(format!("{err:?}").contains("unsafe package entry"));
        assert!(!dest.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No replayer registered → deltas copied verbatim, nothing dropped.
    #[tokio::test]
    async fn without_replayer_deltas_are_copied() {
        init_test_env();
        let _run_guard = RUN_REPLAY_TESTS.lock().await;
        let root = test_root("noreplay");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let into = root.join("restored-noreplay");

        let store = BackupEngine::local_store_for_test(&root.join("store5")).unwrap();
        let engine = BackupEngine::new(store, "test");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let manifest = sample_manifest(&engine, &root).await;
        engine.commit(&manifest).await.unwrap();

        {
            let mut guard = AUDIT_REPLAYERS.lock().unwrap();
            guard.clear();
        }
        let opts = RestoreOptions {
            s3_uri: "s3://x/y/z".to_string(),
            point: None,
            into: into.clone(),
            index_dir: None,
            data_dir: None,
            verify_only: false,
            access_key: None,
            secret_key: None,
            region: None,
            live_root: None,
            ..Default::default()
        };

        let report = restore_with_engine(&opts, &engine).await.unwrap();
        assert_eq!(report.audit_deltas_copied, 2);
        assert_eq!(report.audit_deltas_replayed, 0);
        assert!(into.join("audit/deltas/delta-0-10.jsonl.gz").exists());
        assert!(into.join("audit/deltas/delta-10-20.jsonl.gz").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    // ── restore config file: target selection ─────────────────────────────

    /// Write `json` to a unique temp file and parse it as a restore config.
    fn load_config(json: &str) -> BichonResult<RestoreFileConfig> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "bichon-restore-config-{}-{seq}.json",
            std::process::id()
        ));
        std::fs::write(&path, json).unwrap();
        RestoreFileConfig::load(&path)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bichon-restore-target-{}-{tag}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restore_config_requires_s3_fields_only_for_an_s3_target() {
        // A file written before local targets existed has no `target_type` and
        // must still validate exactly as it used to.
        load_config(r#"{"s3_endpoint":"http://minio:9000","s3_bucket":"bichon"}"#).unwrap();
        // ...and the S3 branch still demands both fields.
        assert!(load_config(r#"{"s3_endpoint":"http://minio:9000"}"#).is_err());
        assert!(load_config(r#"{"s3_bucket":"bichon"}"#).is_err());

        // A local target does not need them, but does need a real directory.
        let dir = temp_dir("local-config");
        let json = format!(
            r#"{{"target_type":"local","local_dir":{}}}"#,
            serde_json::to_string(&dir.display().to_string()).unwrap()
        );
        let cfg = load_config(&json).unwrap();
        assert_eq!(cfg.target_type, BackupTargetType::Local);
        assert_eq!(cfg.target_display(), format!("{}/bichon-backup", dir.display()));

        // A local target without a directory is rejected.
        assert!(load_config(r#"{"target_type":"local"}"#).is_err());
        // ...and so is one pointing at a directory that is not there.
        let absent = dir.join("nope");
        let json = format!(
            r#"{{"target_type":"local","local_dir":{}}}"#,
            serde_json::to_string(&absent.display().to_string()).unwrap()
        );
        assert!(load_config(&json).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_config_target_display_is_honest_for_both_targets() {
        let s3 = load_config(r#"{"s3_endpoint":"http://minio:9000","s3_bucket":"bichon"}"#).unwrap();
        assert_eq!(s3.target_display(), "s3://http://minio:9000/bichon/bichon-backup");

        let dir = temp_dir("display");
        let json = format!(
            r#"{{"target_type":"local","local_dir":{},"prefix":"archive"}}"#,
            serde_json::to_string(&dir.display().to_string()).unwrap()
        );
        let local = load_config(&json).unwrap();
        // The local display must be a real path, never a fabricated `s3://`.
        assert_eq!(
            local.target_display(),
            format!("{}/archive", dir.display())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_restore_engine_dispatches_on_the_target_type() {
        let dir = temp_dir("dispatch");
        let local = RestoreOptions {
            target_type: BackupTargetType::Local,
            local_dir: Some(dir.clone()),
            prefix: "archive".to_string(),
            ..Default::default()
        };
        // A local target opens the directory itself.
        build_restore_engine(&local).unwrap();
        // ...and a missing directory is refused rather than created.
        let absent = RestoreOptions {
            target_type: BackupTargetType::Local,
            local_dir: Some(dir.join("nope")),
            ..Default::default()
        };
        assert!(build_restore_engine(&absent).is_err());
        // A local target with no directory at all is a configuration error.
        let unset = RestoreOptions {
            target_type: BackupTargetType::Local,
            ..Default::default()
        };
        assert!(build_restore_engine(&unset).is_err());
        // An S3 target still parses its URI (and fails on a malformed one).
        let s3 = RestoreOptions {
            target_type: BackupTargetType::S3,
            s3_uri: "not-a-uri".to_string(),
            ..Default::default()
        };
        assert!(build_restore_engine(&s3).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The end-to-end shape a local restore actually takes: a config file
    /// naming a directory, resolved into options, then into an engine that
    /// finds the restore point the engine wrote there.
    #[tokio::test]
    async fn restore_from_a_local_target_config_round_trips() {
        init_test_env();
        // The sample manifest carries audit objects, so serialize with the
        // replayer tests and pin the registry to empty (same reasoning as
        // `restore_respects_separate_index_and_data_dirs`).
        let _run_guard = RUN_REPLAY_TESTS.lock().await;
        {
            let mut guard = AUDIT_REPLAYERS.lock().unwrap();
            guard.clear();
        }

        let root = test_root("local-config-e2e");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store_dir = root.join("backup");
        let into = root.join("restored");

        let store = BackupEngine::local_store_for_test(&store_dir).unwrap();
        let engine = BackupEngine::new(store, "bichon-backup");
        let _hello = engine.upload_bytes_for_test(b"hello").await.unwrap();
        let committed = sample_manifest(&engine, &root).await;
        engine.commit(&committed).await.unwrap();

        // Everything from here mirrors `bichon-admin restore --config`: the
        // target comes from the file, not from anything in memory.
        let json = format!(
            r#"{{"target_type":"local","local_dir":{}}}"#,
            serde_json::to_string(&store_dir.display().to_string()).unwrap()
        );
        let file = load_config(&json).unwrap();
        let opts = RestoreOptions {
            target_type: file.target_type,
            s3_uri: file.s3_uri(),
            local_dir: file.local_dir.as_deref().map(PathBuf::from),
            prefix: file.prefix.clone(),
            into: into.clone(),
            live_root: None,
            ..Default::default()
        };

        let built = build_restore_engine(&opts).unwrap();
        let report = restore_with_engine(&opts, &built).await.unwrap();

        assert_eq!(
            report.manifest_id, committed.id,
            "the config file's directory must resolve to the store that wrote the point"
        );
        assert!(report.memdb_restored, "the memdb snapshot must land on disk");
        assert!(
            into.join("memdb/snapshot.json").exists(),
            "the restored tree must be populated: {:?}",
            std::fs::read_dir(&into).map(|d| d
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>())
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
