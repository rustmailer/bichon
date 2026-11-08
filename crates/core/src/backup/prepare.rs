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

//! The `BackupPreparer` extension point — the **contributor protocol**.
//!
//! The capture window is "fully quiesced": the write gate is paused and every
//! queue drained, so no storage layer mutates on disk. During it, each
//! registered contributor runs its [`BackupPreparer::prepare`] and *delivers*
//! its exact products as [`Artifact`]s. The manager collects the artifacts
//! and hands them to the backend, which uploads them *after* the gate
//! reopens (R2).
//!
//! # The edition contract
//!
//! The community edition registers exactly four preparers via
//! [`register_base_preparers`] (memdb, blob, envelope index, attachment
//! index). **Pro** and **enterprise** editions register their own additional
//! preparers at startup, *before* [`crate::backup::init`] runs:
//!
//! ```ignore
//! // in the Pro binary, before BichonContext::initialize():
//! backup::prepare::register_preparer(Arc::new(ProSqlitePreparer::new(...)));
//! ```
//!
//! A preparer's `prepare` runs inside the capture window with everything
//! quiesced. The artifacts it returns must be immutable files that survive
//! until the upload phase finishes; anything written to
//! `PreparerContext::staging_dir` is cleaned up by its
//! [`BackupPreparer::finalize`], which the manager guarantees to call
//! unconditionally (success or failure) to restore paused background work
//! (e.g. blob GC) and delete staging material.

use std::future::Future;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::{write::GzEncoder, Compression};

use crate::backup::artifact::{Artifact, ArtifactKey};
use crate::backup::manifest::Manifest;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use crate::settings::dir::DATA_DIR_MANAGER;

/// Paths handed to every preparer. `staging_dir` (inside the data root) is
/// scratch space for per-run preparation material; the manager deletes it via
/// each preparer's `finalize`.
#[derive(Clone, Debug)]
pub struct PreparerContext {
    /// The data root being backed up.
    pub root_dir: PathBuf,
    /// Scratch area for per-run preparation material
    /// (`<root>/backup-staging`, see `settings::dir`). Preparers write
    /// immutable artifacts here (e.g. the packaged memdb snapshot) and
    /// delete them in `finalize`.
    pub staging_dir: PathBuf,
    /// The most recently committed restore point, loaded from the backend
    /// during preflight. Contributors that need chain state (e.g. the Pro
    /// audit contributor reading the previous cursor) read it here; `None`
    /// before the first commit or when no backup has ever run.
    pub previous_manifest: Option<Manifest>,
}

/// What an upload-phase contributor delivers, plus optional manifest facts it
/// wants stamped on the restore point. Only the Pro audit contributor sets
/// the audit facts; the manager forwards them to the backend verbatim.
#[derive(Debug, Clone, Default)]
pub struct UploadDelivery {
    /// Artifacts produced during the upload phase (appended to the run's
    /// batch and uploaded with the captured ones).
    pub artifacts: Vec<Artifact>,
    /// Highest audit seq covered by this run's delivery (`None` = the audit
    /// chain did not advance).
    pub audit_cursor: Option<i64>,
    /// Cumulative audit delta bytes since the current baseline, after this
    /// run (`None` = leave the manifest's value untouched).
    pub audit_cumulative_delta_bytes: Option<u64>,
}

/// A storage contributor that prepares its data for a backup capture window.
///
/// `prepare` is called inside the quiesced window (write gate paused, queues
/// drained); it must flush its storage into a copy-safe state and return the
/// immutable files that represent it. `upload` runs *after* the gate reopens
/// (R2) for work that must not hold the window (e.g. exporting a long audit
/// delta through a WAL read transaction). `finalize` is called unconditionally
/// after the run — success or failure — to restore anything paused (GC) and
/// to clean up staging material.
pub trait BackupPreparer: Send + Sync {
    /// Short identifier used in logs and status (`"memdb"`, `"blob"`, ...).
    fn name(&self) -> &'static str;

    /// Flush this storage into a copy-safe state and return the artifacts to
    /// back up. The returned future is awaited by the manager while the
    /// window is open.
    fn prepare<'a>(
        &'a self,
        ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>>;

    /// Upload-phase delivery, called after the write gate reopens. Defaults to
    /// delivering nothing; contributors with O(data) capture work (the Pro
    /// audit delta export) override it. Its future is awaited by the manager
    /// before the backend upload.
    fn upload<'a>(
        &'a self,
        _ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<UploadDelivery>> + Send + 'a>> {
        Box::pin(async move { Ok(UploadDelivery::default()) })
    }

    /// Undo anything `prepare` paused and clean up staging material. Defaults
    /// to a no-op.
    fn finalize(&self, _ctx: &PreparerContext) {}
}

static PREPARERS: LazyLock<Mutex<Vec<Arc<dyn BackupPreparer>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Append a preparer. This is the seam Pro / enterprise editions use to add
/// their own preparation steps; call it at startup before the first backup.
/// Order matters: preparers run in registration order, so register base
/// ones first (`register_base_preparers`) then append extras.
pub fn register_preparer(preparer: Arc<dyn BackupPreparer>) {
    PREPARERS.lock().unwrap().push(preparer);
}

/// Register the community edition's four preparers. Idempotent: the Pro
/// binary can call this too (it is a no-op after the first call).
pub fn register_base_preparers() {
    static BASE_REGISTERED: AtomicBool = AtomicBool::new(false);
    if BASE_REGISTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    register_preparer(Arc::new(MemDbPreparer));
    register_preparer(Arc::new(BlobPreparer));
    register_preparer(Arc::new(EnvelopeIndexPreparer));
    register_preparer(Arc::new(AttachmentIndexPreparer));
    register_preparer(Arc::new(TombstonesPreparer));
}

/// Snapshot of the registered preparers, in registration order. Used by the
/// backup manager for the capture and finalize passes.
pub(crate) fn preparers() -> Vec<Arc<dyn BackupPreparer>> {
    PREPARERS.lock().unwrap().clone()
}

/// Non-fatal warnings reported by preparers during the current run (e.g. "the
/// audit database has grown large and is slowing backups"). Collected by the
/// manager after the capture pass, copied into the run record and status, and
/// cleared at the start of the next run. Surfaced to admins in the WebUI
/// notifications bell and the backup status page.
static PREPARER_WARNINGS: LazyLock<Mutex<Vec<String>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Record a non-fatal warning for the current run. Editions call this from
/// `prepare` when something is worth the admin's attention but does not fail
/// the backup (a database growing past its warn threshold, a skipped feature,
/// ...).
pub fn record_warning(warning: impl Into<String>) {
    let warning = warning.into();
    tracing::warn!("backup: {warning}");
    PREPARER_WARNINGS.lock().unwrap().push(warning);
}

/// Warnings collected during the just-finished capture pass, in order. Cleared
/// afterwards so each run starts clean.
pub(crate) fn take_warnings() -> Vec<String> {
    std::mem::take(&mut *PREPARER_WARNINGS.lock().unwrap())
}

#[cfg(test)]
pub(crate) fn set_preparers_for_test(preps: Vec<Arc<dyn BackupPreparer>>) {
    *PREPARERS.lock().unwrap() = preps;
}

#[cfg(test)]
pub(crate) fn take_preparers() -> Vec<Arc<dyn BackupPreparer>> {
    std::mem::take(&mut *PREPARERS.lock().unwrap())
}

// ── artifact helpers ───────────────────────────────────────────────────────

/// Build an [`Artifact`] from a local file: size + SHA-256.
pub fn artifact_from_file(local: PathBuf, logical: ArtifactKey) -> BichonResult<Artifact> {
    let bytes = std::fs::metadata(&local)
        .map_err(|e| {
            raise_error!(
                format!("backup: cannot stat artifact {}: {e}", local.display()),
                ErrorCode::InternalError
            )
        })?
        .len();
    let sha256 = sha256_file(&local)?;
    Ok(Artifact {
        local,
        logical,
        sha256,
        bytes,
    })
}

/// SHA-256 of a file, streamed in 1 MiB chunks (a multi-hundred-MB segment
/// never sits in memory).
pub(crate) fn sha256_file(path: &Path) -> BichonResult<[u8; 32]> {
    use ring::digest::{Context, SHA256};
    let mut file = std::fs::File::open(path).map_err(|e| {
        raise_error!(
            format!("backup: cannot open {} for hashing: {e}", path.display()),
            ErrorCode::InternalError
        )
    })?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            raise_error!(
                format!("backup: cannot read {} for hashing: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let digest = ctx.finish();
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    Ok(out)
}

/// Magic header of the streaming index package format. It is followed by one
/// record per packaged file, `[u32 BE name_len][name][u64 BE content_len]
/// [content]`, with the whole stream gzip-wrapped. Fixed header + sorted file
/// names keep it deterministic (R8). This replaced the old base64-JSON-map
/// layout, which read every file into memory — untenable for multi-GB tantivy
/// indexes.
const IDX_PACKAGE_MAGIC: &[u8] = b"BICHON.IDX1\n";

/// Package header for the recursive tree format (tombstones). The index
/// format only accepts flat names; a tree package carries `a/b` relative
/// paths and validates every component separately on restore.
const TREE_PACKAGE_MAGIC: &[u8] = b"BICHON.TREE1\n";

/// Maximum accepted package entry name length (a safety cap so a crafted
/// stream cannot force a huge allocation; real index file names are short).
const MAX_PACKAGE_NAME_LEN: usize = 4096;

/// Files to leave out of an index package. Tantivy holds
/// `.tantivy-meta.lock` and `.tantivy-writer.lock` with an exclusive
/// byte-range lock for the whole process lifetime, so reading them (on
/// Windows) fails with a lock violation. They are re-created on open — pure
/// transient state, never part of a backup.
fn skip_index_file(name: &str) -> bool {
    name.ends_with(".lock")
}

/// Package every regular file in `dir` (sorted by name, tantivy lock files
/// skipped) into a single gzip stream: `IDX_PACKAGE_MAGIC`, then for each
/// file a `[name_len][name][content_len][content]` record. File contents are
/// copied chunk-by-chunk, so peak memory is independent of the directory size
/// — a multi-GB tantivy index is never read into memory. Deterministic
/// (fixed header + sorted names) so identical directories produce identical
/// bytes (R8).
pub(crate) fn package_dir(dir: &Path, out: &Path) -> BichonResult<()> {
    let mut names: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| {
        raise_error!(
            format!("backup: cannot read {} for packaging: {e}", dir.display()),
            ErrorCode::InternalError
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            raise_error!(
                format!("backup: cannot read {} for packaging: {e}", dir.display()),
                ErrorCode::InternalError
            )
        })?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip_index_file(&name) {
            continue;
        }
        names.push(name);
    }
    names.sort_unstable();

    let file = std::fs::File::create(out).map_err(|e| {
        raise_error!(
            format!("backup: cannot create packaged artifact {}: {e}", out.display()),
            ErrorCode::InternalError
        )
    })?;
    let mut enc = GzEncoder::new(std::io::BufWriter::new(file), Compression::default());
    enc.write_all(IDX_PACKAGE_MAGIC).map_err(|e| {
        raise_error!(
            format!("backup: cannot write index package header to {}: {e}", out.display()),
            ErrorCode::InternalError
        )
    })?;
    for name in names {
        if name.len() > MAX_PACKAGE_NAME_LEN {
            return Err(raise_error!(
                format!("backup: index file name too long in {}: {name}", dir.display()),
                ErrorCode::InternalError
            ));
        }
        let path = dir.join(&name);
        let meta = std::fs::metadata(&path).map_err(|e| {
            raise_error!(
                format!("backup: cannot stat {} for packaging: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
        enc.write_all(&(name.len() as u32).to_be_bytes()).map_err(|e| {
            raise_error!(
                format!("backup: index package write failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        enc.write_all(name.as_bytes()).map_err(|e| {
            raise_error!(
                format!("backup: index package write failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        enc.write_all(&meta.len().to_be_bytes()).map_err(|e| {
            raise_error!(
                format!("backup: index package write failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        let mut src = std::fs::File::open(&path).map_err(|e| {
            raise_error!(
                format!("backup: cannot read {} for packaging: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
        std::io::copy(&mut src, &mut enc).map_err(|e| {
            raise_error!(
                format!("backup: cannot stream {} into package: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
    }
    let mut inner = enc.finish().map_err(|e| {
        raise_error!(
            format!("backup: index package gzip failed: {e}"),
            ErrorCode::InternalError
        )
    })?;
    inner.flush().map_err(|e| {
        raise_error!(
            format!("backup: index package flush failed: {e}"),
            ErrorCode::InternalError
        )
    })?;
    Ok(())
}

/// Inverse of [`package_dir`]: unpack an index package back into `dest_dir`,
/// creating it. Accepts both the current streaming record format and the
/// legacy gzip JSON map (file name → base64). Restore-side (design doc §8),
/// so a crafted package must never write outside `dest_dir`: every entry must
/// be a plain file name with no path separators and no `.`/`..` tricks.
pub(crate) fn unpackage_dir(package: &Path, dest_dir: &Path) -> BichonResult<()> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        raise_error!(
            format!("restore: cannot create {}: {e}", dest_dir.display()),
            ErrorCode::InternalError
        )
    })?;
    let file = std::fs::File::open(package).map_err(|e| {
        raise_error!(
            format!("restore: cannot read package {}: {e}", package.display()),
            ErrorCode::InternalError
        )
    })?;
    let mut dec = std::io::BufReader::new(flate2::read::GzDecoder::new(
        std::io::BufReader::new(file),
    ));
    // Peek without consuming: the streaming format opens with the magic
    // header, the legacy format is a gzip JSON object starting with `{`.
    let is_stream = {
        let head = dec.fill_buf().map_err(|e| {
            raise_error!(
                format!("restore: corrupt package {}: {e}", package.display()),
                ErrorCode::InternalError
            )
        })?;
        head.len() >= IDX_PACKAGE_MAGIC.len() && head[..IDX_PACKAGE_MAGIC.len()] == *IDX_PACKAGE_MAGIC
    };
    if is_stream {
        unpackage_stream(&mut dec, dest_dir, package)
    } else {
        unpackage_legacy_json(&mut dec, dest_dir, package)
    }
}

/// Path-traversal guard shared by both package formats: only a bare file name
/// may be written.
fn check_unsafe_name(name: &str) -> BichonResult<()> {
    let unsafe_name = name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with("..")
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':');
    if unsafe_name {
        return Err(raise_error!(
            format!("restore: refusing unsafe package entry {name:?}"),
            ErrorCode::InvalidParameter
        ));
    }
    Ok(())
}

/// Unify "corrupt package" errors across the io/serde failure modes.
fn corrupt_package(package: &Path, e: impl std::fmt::Display) -> crate::error::BichonError {
    raise_error!(
        format!("restore: corrupt package {}: {e}", package.display()),
        ErrorCode::InternalError
    )
}

/// Unpack the streaming record format: `[u32 name_len][name][u64
/// content_len][content]` per file, contents written chunk-by-chunk so a
/// multi-GB package is never held in memory. The magic header was peeked but
/// not consumed — read it here, then records until a clean EOF.
fn unpackage_stream(
    dec: &mut impl Read,
    dest_dir: &Path,
    package: &Path,
) -> BichonResult<()> {
    let mut magic = [0u8; IDX_PACKAGE_MAGIC.len()];
    dec.read_exact(&mut magic)
        .map_err(|e| corrupt_package(package, e))?;
    loop {
        let mut name_len = [0u8; 4];
        match dec.read_exact(&mut name_len) {
            Ok(()) => {}
            // Clean EOF right after the header: a package with no more files.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(corrupt_package(package, e)),
        }
        let name_len = u32::from_be_bytes(name_len) as usize;
        if name_len > MAX_PACKAGE_NAME_LEN {
            return Err(corrupt_package(
                package,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "entry name too long"),
            ));
        }
        let mut name = vec![0u8; name_len];
        dec.read_exact(&mut name)
            .map_err(|e| corrupt_package(package, e))?;
        let name = String::from_utf8(name).map_err(|_| {
            corrupt_package(
                package,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "non-utf8 name"),
            )
        })?;
        check_unsafe_name(&name)?;
        let mut content_len = [0u8; 8];
        dec.read_exact(&mut content_len)
            .map_err(|e| corrupt_package(package, e))?;
        let content_len = u64::from_be_bytes(content_len);
        let mut out = std::fs::File::create(dest_dir.join(&name)).map_err(|e| {
            raise_error!(
                format!("restore: cannot write {}: {e}", dest_dir.join(&name).display()),
                ErrorCode::InternalError
            )
        })?;
        let copied = std::io::copy(&mut dec.by_ref().take(content_len), &mut out)
            .map_err(|e| corrupt_package(package, e))?;
        if copied != content_len {
            return Err(corrupt_package(
                package,
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "entry content truncated",
                ),
            ));
        }
    }
}

/// Legacy format (pre-streaming backups): a gzip JSON object mapping file
/// name → base64 content.
fn unpackage_legacy_json(
    dec: &mut impl Read,
    dest_dir: &Path,
    package: &Path,
) -> BichonResult<()> {
    let mut json = Vec::new();
    dec.read_to_end(&mut json)
        .map_err(|e| corrupt_package(package, e))?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&json)
        .map_err(|e| corrupt_package(package, e))?;
    for (name, value) in map {
        check_unsafe_name(&name)?;
        let base64 = value.as_str().ok_or_else(|| {
            raise_error!(
                format!("restore: package entry {name:?} is not a string"),
                ErrorCode::InvalidParameter
            )
        })?;
        let data = STANDARD.decode(base64).map_err(|e| {
            raise_error!(
                format!("restore: package entry {name:?} is not valid base64: {e}"),
                ErrorCode::InvalidParameter
            )
        })?;
        std::fs::write(dest_dir.join(&name), data).map_err(|e| {
            raise_error!(
                format!("restore: cannot write {}: {e}", dest_dir.join(&name).display()),
                ErrorCode::InternalError
            )
        })?;
    }
    Ok(())
}

// ── memdb ──────────────────────────────────────────────────────────────────

/// memdb: stream the in-memory state (the `(last_seq, data)` pair, which is
/// already a complete, self-consistent snapshot) straight to a gzip artifact
/// in the staging dir — one pass, no read-back off disk, no re-packaging. The
/// live memdb dir is left untouched; the dump is uploaded after the gate
/// reopens.
pub struct MemDbPreparer;

impl BackupPreparer for MemDbPreparer {
    fn name(&self) -> &'static str {
        "memdb"
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
        Box::pin(async move {
            let out = ctx.staging_dir.join("snapshot.json.gz");
            let file = std::fs::File::create(&out).map_err(|e| {
                raise_error!(
                    format!("backup: cannot create {}: {e}", out.display()),
                    ErrorCode::InternalError
                )
            })?;
            let mut gz = GzEncoder::new(file, Compression::default());
            crate::database::manager::DB_MANAGER.dump_to(&mut gz)?;
            // An explicit finish flushes the compression stream and writes the
            // gzip trailer, surfacing errors here while we can still fail the
            // capture (Drop would swallow them).
            gz.finish().map_err(|e| {
                raise_error!(
                    format!("backup: memdb package gzip finish failed: {e}"),
                    ErrorCode::InternalError
                )
            })?;
            artifact_from_file(out, ArtifactKey::Memdb).map(|a| vec![a])
        })
    }
}

// ── blob ───────────────────────────────────────────────────────────────────

/// bichon-blob content store: pause GC, seal the active segment, fsync every
/// segment and flush the redb index, then deliver every sealed segment as an
/// artifact (hashes computed at seal time, page-cache warm). GC is restored
/// in `finalize`, which the manager guarantees to call even on failure.
pub struct BlobPreparer;

impl BackupPreparer for BlobPreparer {
    fn name(&self) -> &'static str {
        "blob"
    }

    fn prepare<'a>(
        &'a self,
        _ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
        Box::pin(async move {
            crate::store::blob::BLOB_MANAGER.prepare_for_backup()?;
            let segments = crate::store::blob::BLOB_MANAGER.backup_segments()?;
            Ok(segments
                .into_iter()
                .map(|s| Artifact {
                    local: s.path,
                    logical: ArtifactKey::BlobSegment(s.segment_id),
                    sha256: s.sha256,
                    bytes: s.bytes,
                })
                .collect())
        })
    }

    fn finalize(&self, _ctx: &PreparerContext) {
        crate::store::blob::BLOB_MANAGER.resume_gc();
    }
}

// ── tantivy indices (always included) ──────────────────────────────────────

/// Tantivy envelope index. The search index is always part of every backup
/// point: restore-time readiness beats the derived-data argument, and a
/// "rebuild after restore" path is a much worse DR story than a bigger
/// object.
pub struct EnvelopeIndexPreparer;

impl BackupPreparer for EnvelopeIndexPreparer {
    fn name(&self) -> &'static str {
        "envelope-index"
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
        Box::pin(async move {
            // Drains the writer queue, commits and deterministically waits
            // for merges — after this the index directory is copy-safe
            // (nothing is mid-write, and the writer cannot be holding files
            // open while the gate stays paused).
            crate::store::tantivy::envelope::ENVELOPE_MANAGER
                .prepare_for_backup()
                .await?;
            let dir = DATA_DIR_MANAGER.envelope_dir.clone();
            let out = ctx.staging_dir.join("envelope-index.idx.gz");
            package_dir(&dir, &out)?;
            artifact_from_file(out, ArtifactKey::TantivyEnvelope).map(|a| vec![a])
        })
    }
}

/// Tantivy attachment index: same treatment as the envelope index.
pub struct AttachmentIndexPreparer;

impl BackupPreparer for AttachmentIndexPreparer {
    fn name(&self) -> &'static str {
        "attachment-index"
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
        Box::pin(async move {
            crate::store::tantivy::attachment::ATTACHMENT_MANAGER
                .prepare_for_backup()
                .await?;
            let dir = DATA_DIR_MANAGER.attachment_dir.clone();
            let out = ctx.staging_dir.join("attachment-index.idx.gz");
            package_dir(&dir, &out)?;
            artifact_from_file(out, ArtifactKey::TantivyAttachment).map(|a| vec![a])
        })
    }
}

/// Source-UID tombstones: one directory of compressed bitmaps. Packaged as a
/// single artifact — the total is small, and one object keeps the manifest and
/// restore path simple.
pub struct TombstonesPreparer;

impl BackupPreparer for TombstonesPreparer {
    fn name(&self) -> &'static str {
        "tombstones"
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a PreparerContext,
    ) -> Pin<Box<dyn Future<Output = BichonResult<Vec<Artifact>>> + Send + 'a>> {
        Box::pin(async move {
            let dir = DATA_DIR_MANAGER.tombstones_dir.clone();
            std::fs::create_dir_all(&dir).map_err(|e| {
                raise_error!(
                    format!("backup: cannot create {}: {e}", dir.display()),
                    ErrorCode::InternalError
                )
            })?;
            remove_stale_temp_files(&dir)?;
            let out = ctx.staging_dir.join("tombstones.tomb.gz");
            package_tree(&dir, &out)?;
            artifact_from_file(out, ArtifactKey::Tombstones).map(|a| vec![a])
        })
    }
}

/// A crash can leave `.<mailbox>.<pid>.tmp` files behind; they are never
/// meaningful state and must not be carried into a backup.
fn remove_stale_temp_files(dir: &Path) -> BichonResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(raise_error!(
                format!("backup: cannot read {}: {e}", dir.display()),
                ErrorCode::InternalError
            ))
        }
    };
    for account in entries.flatten() {
        let path = account.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&path) {
            for file in files.flatten() {
                let name = file.file_name();
                if !name.to_string_lossy().ends_with(".tmp") {
                    continue;
                }
                // Only sweep leftovers from a crash. A write in flight holds
                // the backup gate, so in production none can be running here —
                // but tests and a stuck writer must not lose their temp file.
                let stale = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > std::time::Duration::from_secs(3600));
                if stale {
                    let _ = std::fs::remove_file(file.path());
                }
            }
        }
    }
    Ok(())
}


/// Recursively package a directory tree. Each record's name is the file's
/// relative path with `/` separators (e.g. `42/7`). The tombstone store is
/// laid out by account and mailbox, so the flat index format cannot express
/// it.
pub fn package_tree(dir: &Path, out: &Path) -> BichonResult<()> {
    fn collect(dir: &Path, base: &Path, out: &mut Vec<String>) -> BichonResult<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(raise_error!(
                    format!("backup: cannot read {} for packaging: {e}", dir.display()),
                    ErrorCode::InternalError
                ))
            }
        };
        for entry in entries {
            let entry = entry.map_err(|e| {
                raise_error!(
                    format!("backup: cannot read {} for packaging: {e}", dir.display()),
                    ErrorCode::InternalError
                )
            })?;
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                collect(&path, base, out)?;
            } else if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let relative = path.strip_prefix(base).map_err(|_| {
                    raise_error!(
                        format!("backup: {} is outside {}", path.display(), base.display()),
                        ErrorCode::InternalError
                    )
                })?;
                let mut parts = Vec::new();
                let mut skip = false;
                for component in relative.components() {
                    let Some(text) = component.as_os_str().to_str() else {
                        return Err(raise_error!(
                            format!("backup: non-UTF-8 file name in {}", path.display()),
                            ErrorCode::InternalError
                        ));
                    };
                    if text.starts_with('.') {
                        // In-progress temp files are never part of the state.
                        skip = true;
                        break;
                    }
                    parts.push(text);
                }
                if !skip {
                    out.push(parts.join("/"));
                }
            }
        }
        Ok(())
    }

    let mut names = Vec::new();
    collect(dir, dir, &mut names)?;
    names.sort_unstable();

    let file = std::fs::File::create(out).map_err(|e| {
        raise_error!(
            format!("backup: cannot create packaged tree {}: {e}", out.display()),
            ErrorCode::InternalError
        )
    })?;
    let mut enc = GzEncoder::new(std::io::BufWriter::new(file), Compression::default());
    enc.write_all(TREE_PACKAGE_MAGIC)
        .map_err(|e| raise_error!(format!("backup: tree header: {e}"), ErrorCode::InternalError))?;
    for name in names {
        if name.len() > MAX_PACKAGE_NAME_LEN {
            return Err(raise_error!(
                format!("backup: tree entry name too long: {name}"),
                ErrorCode::InternalError
            ));
        }
        let path = dir.join(&name);
        let meta = std::fs::metadata(&path).map_err(|e| {
            raise_error!(
                format!("backup: cannot stat {}: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
        enc.write_all(&(name.len() as u32).to_be_bytes())
            .map_err(|e| raise_error!(format!("backup: tree write: {e}"), ErrorCode::InternalError))?;
        enc.write_all(name.as_bytes())
            .map_err(|e| raise_error!(format!("backup: tree write: {e}"), ErrorCode::InternalError))?;
        enc.write_all(&meta.len().to_be_bytes())
            .map_err(|e| raise_error!(format!("backup: tree write: {e}"), ErrorCode::InternalError))?;
        let mut src = std::fs::File::open(&path).map_err(|e| {
            raise_error!(
                format!("backup: cannot read {}: {e}", path.display()),
                ErrorCode::InternalError
            )
        })?;
        std::io::copy(&mut src, &mut enc)
            .map_err(|e| raise_error!(format!("backup: tree stream: {e}"), ErrorCode::InternalError))?;
    }
    let mut inner = enc
        .finish()
        .map_err(|e| raise_error!(format!("backup: tree gzip: {e}"), ErrorCode::InternalError))?;
    inner
        .flush()
        .map_err(|e| raise_error!(format!("backup: tree flush: {e}"), ErrorCode::InternalError))?;
    Ok(())
}

/// Reverse of [`package_tree`]. Every entry must be a relative path whose
/// components are plain names: no `.`/`..`, separators inside a component,
/// drive letters or absolute paths. A crafted backup must never write outside
/// `dest_dir`.
pub(crate) fn unpackage_tree(package: &Path, dest_dir: &Path) -> BichonResult<()> {
    fn check_relative(name: &str) -> BichonResult<()> {
        if name.is_empty() || name.starts_with('/') {
            return Err(raise_error!(
                format!("restore: refusing unsafe tree entry {name:?}"),
                ErrorCode::InvalidParameter
            ));
        }
        for component in name.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.contains('\\')
                || component.contains(':')
            {
                return Err(raise_error!(
                    format!("restore: refusing unsafe tree entry {name:?}"),
                    ErrorCode::InvalidParameter
                ));
            }
        }
        Ok(())
    }

    std::fs::create_dir_all(dest_dir).map_err(|e| {
        raise_error!(
            format!("restore: cannot create {}: {e}", dest_dir.display()),
            ErrorCode::InternalError
        )
    })?;
    let file = std::fs::File::open(package).map_err(|e| {
        raise_error!(
            format!("restore: cannot read tree package {}: {e}", package.display()),
            ErrorCode::InternalError
        )
    })?;
    let mut dec = std::io::BufReader::new(flate2::read::GzDecoder::new(
        std::io::BufReader::new(file),
    ));
    let mut magic = [0u8; TREE_PACKAGE_MAGIC.len()];
    dec.read_exact(&mut magic)
        .map_err(|e| corrupt_package(package, e))?;
    if magic != *TREE_PACKAGE_MAGIC {
        return Err(corrupt_package(package, "bad tree magic"));
    }
    loop {
        let mut name_len = [0u8; 4];
        match dec.read_exact(&mut name_len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(corrupt_package(package, e)),
        }
        let name_len = u32::from_be_bytes(name_len) as usize;
        if name_len > MAX_PACKAGE_NAME_LEN {
            return Err(corrupt_package(package, "tree entry name too long"));
        }
        let mut name = vec![0u8; name_len];
        dec.read_exact(&mut name)
            .map_err(|e| corrupt_package(package, e))?;
        let name = String::from_utf8(name)
            .map_err(|_| corrupt_package(package, "non-utf8 tree name"))?;
        check_relative(&name)?;
        let mut content_len = [0u8; 8];
        dec.read_exact(&mut content_len)
            .map_err(|e| corrupt_package(package, e))?;
        let content_len = u64::from_be_bytes(content_len);
        let dest = dest_dir.join(&name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                raise_error!(
                    format!("restore: cannot create {}: {e}", parent.display()),
                    ErrorCode::InternalError
                )
            })?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| {
            raise_error!(
                format!("restore: cannot write {}: {e}", dest.display()),
                ErrorCode::InternalError
            )
        })?;
        let copied = std::io::copy(&mut dec.by_ref().take(content_len), &mut out)
            .map_err(|e| corrupt_package(package, e))?;
        if copied != content_len {
            return Err(corrupt_package(package, "tree entry content truncated"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-test scratch dir (tests run in parallel, so no two may share one).
    /// Fresh: removed if left over from a prior crash, then recreated.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bichon-prepare-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The tombstone directory is packaged as one artifact and unpacks back
    /// into the layout the delete path and gap-fill expect.
    #[tokio::test]
    async fn tombstones_preparer_round_trips_through_the_package() {
        use crate::settings::cli::SETTINGS;
        use crate::settings::dir::DATA_DIR_MANAGER;

        static ENV: std::sync::Once = std::sync::Once::new();
        ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
        });

        // Shares the process-global tombstone root with the tombstone unit
        // tests; serialize so their cleanup cannot race this walk.
        let _global = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;

        let account_id = 96_001;
        let mailbox_id = 96_002;
        crate::archive::imap::tombstones::record_uids(account_id, mailbox_id, 3, &[1, 2, 3])
            .await
            .unwrap();

        let staging = scratch("tombstones-staging");
        let ctx = PreparerContext {
            root_dir: DATA_DIR_MANAGER.root_dir.clone(),
            staging_dir: staging.clone(),
            previous_manifest: None,
        };
        let artifacts = TombstonesPreparer.prepare(&ctx).await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert!(matches!(artifacts[0].logical, ArtifactKey::Tombstones));
        assert!(artifacts[0].bytes > 0);

        let dest = scratch("tombstones-unpack");
        unpackage_tree(&artifacts[0].local, &dest).unwrap();
        let restored = dest.join(account_id.to_string()).join(mailbox_id.to_string());
        assert!(restored.is_file(), "tombstone file restored");

        crate::archive::imap::tombstones::remove_account(account_id)
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn tree_package_rejects_path_traversal() {
        use std::io::Write as _;
        let dir = scratch("tree-traversal");
        let package = dir.join("evil.gz");
        let file = std::fs::File::create(&package).unwrap();
        let mut enc = GzEncoder::new(std::io::BufWriter::new(file), Compression::default());
        enc.write_all(TREE_PACKAGE_MAGIC).unwrap();
        let name = b"../escape";
        enc.write_all(&(name.len() as u32).to_be_bytes()).unwrap();
        enc.write_all(name).unwrap();
        enc.write_all(&4u64.to_be_bytes()).unwrap();
        enc.write_all(b"evil").unwrap();
        enc.finish().unwrap();

        let dest = dir.join("out");
        let err = unpackage_tree(&package, &dest).unwrap_err();
        assert!(format!("{err}").contains("unsafe tree entry"));
        assert!(!dir.join("escape").exists());
        assert!(!dest.join("escape").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_package_round_trip_skips_lock_files() {
        let dir = scratch("roundtrip");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("meta.json"), br#"{"segments":[]}"#).unwrap();
        // A larger file exercises the chunked streaming copy.
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(src.join("0_1.store"), &big).unwrap();
        std::fs::write(src.join("0_1.term"), b"term-data").unwrap();
        // Tantivy lock files must be skipped, not read (they are exclusively
        // locked by the live index — reading them fails on Windows).
        std::fs::write(src.join(".tantivy-meta.lock"), b"LOCK").unwrap();
        std::fs::write(src.join(".tantivy-writer.lock"), b"LOCK").unwrap();

        let pkg = dir.join("index.idx.gz");
        package_dir(&src, &pkg).unwrap();

        let dest = dir.join("dest");
        unpackage_dir(&pkg, &dest).unwrap();

        assert_eq!(
            std::fs::read(dest.join("meta.json")).unwrap(),
            br#"{"segments":[]}"#
        );
        assert_eq!(std::fs::read(dest.join("0_1.store")).unwrap(), big);
        assert_eq!(std::fs::read(dest.join("0_1.term")).unwrap(), b"term-data");
        assert!(
            !dest.join(".tantivy-meta.lock").exists(),
            "lock file must not be packaged"
        );
        assert!(
            !dest.join(".tantivy-writer.lock").exists(),
            "lock file must not be packaged"
        );
    }

    #[test]
    fn index_package_is_deterministic() {
        let dir = scratch("deterministic");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("b.txt"), b"beta").unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        let p1 = dir.join("p1.gz");
        let p2 = dir.join("p2.gz");
        package_dir(&src, &p1).unwrap();
        package_dir(&src, &p2).unwrap();
        assert_eq!(
            std::fs::read(&p1).unwrap(),
            std::fs::read(&p2).unwrap(),
            "identical dirs must produce identical package bytes (R8)"
        );
    }

    /// Old backups used a gzip JSON object (file name → base64). Restore must
    /// keep reading them.
    #[test]
    fn unpackage_dir_accepts_legacy_json_map() {
        use base64::Engine as _;
        let dir = scratch("legacy");
        let mut map = serde_json::Map::new();
        map.insert(
            "meta.json".to_string(),
            serde_json::Value::String(
                base64::engine::general_purpose::STANDARD.encode(b"{}"),
            ),
        );
        let json = serde_json::to_vec(&map).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &json).unwrap();
        let bytes = enc.finish().unwrap();
        let pkg = dir.join("legacy.json.gz");
        std::fs::write(&pkg, &bytes).unwrap();

        let dest = dir.join("dest");
        unpackage_dir(&pkg, &dest).unwrap();
        assert_eq!(std::fs::read(dest.join("meta.json")).unwrap(), b"{}");
    }

    /// The streaming format must apply the same path-traversal guard as the
    /// legacy JSON-map path.
    #[test]
    fn unpackage_stream_rejects_unsafe_names() {
        let dir = scratch("evil");
        let name = b"../escape";
        let mut raw = Vec::new();
        raw.extend_from_slice(IDX_PACKAGE_MAGIC);
        raw.extend_from_slice(&(name.len() as u32).to_be_bytes());
        raw.extend_from_slice(name);
        raw.extend_from_slice(&0u64.to_be_bytes());
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &raw).unwrap();
        let bytes = enc.finish().unwrap();
        let pkg = dir.join("evil.idx.gz");
        std::fs::write(&pkg, &bytes).unwrap();

        let dest = dir.join("dest");
        let err = unpackage_dir(&pkg, &dest).unwrap_err();
        assert!(format!("{err:?}").contains("unsafe package entry"));
        assert!(!dest.join("escape.txt").exists());
    }
}
