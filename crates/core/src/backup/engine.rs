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

//! The native incremental backup engine.
//!
//! The engine owns upload, commit and GC against an `object_store` backend.
//! Which backend is decided by [`BackupTarget`] — an S3-compatible endpoint
//! (AWS / MinIO / R2) or a directory on a filesystem this host can see
//! (`InMemory` in unit tests). Nothing below this module is backend-specific:
//! it only ever sees `Arc<dyn ObjectStore>`. Capture — the seconds of write
//! pause — is the manager's job; everything this module does runs with the
//! write gate already reopened, because every artifact it uploads is an
//! immutable file or an offline export.
//!
//! The invariants this module pins down (design doc §2):
//!
//! * **R4 — manifest last.** Objects are written first, the manifest (the
//!   commit point) second, LATEST last. A crashed run leaves only
//!   unreferenced objects and an orphan manifest; the next GC reclaims them.
//! * **R5 — immutable, key = content.** Objects are never overwritten in
//!   place; the key is `obj/sha256/<hex>` and logical names exist only inside
//!   the manifest.
//! * **R6 — GC never over-deletes.** Objects referenced by *any* retained
//!   manifest survive, whether or not the local copy still exists.
//! * **R8 — idempotent.** Identical content ⇒ identical key ⇒ skipped upload;
//!   an unchanged run uploads nothing.
//!
//! Layout under `<prefix>/v1/`:
//!
//! ```text
//! meta/LATEST                    pointer to the newest manifest
//! meta/m-<id>.json.gz            restore point manifests (gzip JSON)
//! obj/sha256/<hex>               immutable, content-addressed objects
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use object_store::path::Path as StorePath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::backup::artifact::Artifact;
use crate::backup::manifest::{Manifest, ObjectRef, LATEST_NAME, MANIFEST_PREFIX};
use crate::backup::retention::{select_manifests, RetentionPolicy};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;

/// Multipart part size for large objects (segment files). All parts except
/// the last must be ≥ 5 MiB and equal-sized on R2; 16 MiB is a safe middle.
const UPLOAD_PART_SIZE: usize = 16 * 1024 * 1024;

/// Small objects go through a single PUT; above this they stream via
/// `put_multipart` so a multi-hundred-MB segment never sits in memory.
const SINGLE_PUT_MAX: u64 = UPLOAD_PART_SIZE as u64;

/// Layout directory under the configured prefix: everything the engine writes
/// lives under `<prefix>/<LAYOUT_DIR>/`. A local target's directory therefore
/// holds `<local_dir>/<prefix>/<LAYOUT_DIR>/{obj,meta}`.
pub(crate) const LAYOUT_DIR: &str = "v1";

/// Abandoned `#<n>` staging files older than this are swept before a run.
/// Long enough that a concurrent run's in-flight staging file is never
/// deleted out from under it.
const STAGING_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Per-component transfer stats for one run — which *part* of the archive was
/// new vs skipped (dedup), grouped by [`crate::backup::artifact::ArtifactKey::label`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ArtifactStat {
    /// Component name: `"memdb"`, `"blob"`, `"envelope-index"`, …
    pub kind: String,
    pub new_count: u64,
    pub new_bytes: u64,
    pub skipped_count: u64,
    pub skipped_bytes: u64,
}

/// Stats of one engine run, surfaced in the run record / status page.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineSummary {
    /// Id of the committed manifest (empty when the run failed before commit).
    pub manifest_id: String,
    pub uploaded_bytes: u64,
    pub new_objects: u64,
    pub skipped_objects: u64,
    /// Per-component breakdown of the totals above, in artifact delivery
    /// order (`#[serde(default)]` keeps pre-breakdown records readable).
    #[serde(default)]
    pub by_kind: Vec<ArtifactStat>,
}

/// Find (or append, first occurrence) the per-kind stat for `kind`. Uses an
/// index (not a held borrow) so the first occurrence can be returned mutably
/// after the append path.
fn stat_mut<'a>(summary: &'a mut EngineSummary, kind: &str) -> &'a mut ArtifactStat {
    if let Some(idx) = summary.by_kind.iter().position(|s| s.kind == kind) {
        return &mut summary.by_kind[idx];
    }
    summary.by_kind.push(ArtifactStat {
        kind: kind.to_string(),
        ..Default::default()
    });
    summary.by_kind.last_mut().expect("just pushed")
}

/// Stats of one GC pass.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub manifests_removed: u64,
    pub objects_removed: u64,
    pub bytes_reclaimed: u64,
}

/// S3 connection options (mapped from backup settings / env by the caller).
#[derive(Debug, Clone, Default)]
pub struct S3Options {
    /// Endpoint (e.g. `http://localhost:9000` for MinIO, empty for AWS).
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Use path-style addressing. Required by MinIO-style servers; custom
    /// endpoints default to it.
    pub force_path_style: bool,
}

/// Turn a configured S3 endpoint into a URL `object_store` can use.
///
/// Providers advertise their endpoint as a bare host
/// (`s3.us-west-004.backblazeb2.com`; MinIO often as `host:9000`), and
/// operators paste it that way. Without a scheme the failure only surfaces
/// when the first request is built, inside the retry loop, as an error that
/// does not point at the missing scheme. Defaulting to `https://` makes such
/// an entry work as intended, and trimming the trailing slash keeps the
/// concatenated `<endpoint>/<bucket>` path well-formed.
fn normalize_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim();
    let with_scheme = if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("https://{endpoint}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// Build the production `object_store` for an S3-compatible endpoint.
pub fn s3_store(opts: &S3Options) -> BichonResult<Arc<dyn ObjectStore>> {
    if opts.bucket.is_empty() {
        return Err(raise_error!(
            "backup: s3 bucket is required".to_string(),
            ErrorCode::MissingConfiguration
        ));
    }
    let mut builder = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(&opts.bucket)
        .with_region(&opts.region)
        .with_access_key_id(&opts.access_key)
        .with_secret_access_key(&opts.secret_key);
    if !opts.endpoint.is_empty() {
        let endpoint = normalize_endpoint(&opts.endpoint);
        builder = builder.with_endpoint(&endpoint);
        if endpoint.starts_with("http://") {
            builder = builder.with_allow_http(true);
        }
        if opts.force_path_style {
            builder = builder.with_virtual_hosted_style_request(false);
        }
    }
    let store = builder.build().map_err(|e| {
        raise_error!(
            format!("backup: cannot build S3 client: {e}"),
            ErrorCode::MissingConfiguration
        )
    })?;
    Ok(Arc::new(store))
}

/// Where a backup lands.
///
/// The two arms are interchangeable to everything downstream — [`BackupEngine`]
/// only ever holds `Arc<dyn ObjectStore>` — so this type exists purely to
/// carry the connection details and to run the target-specific checks that
/// must happen before a store is opened.
#[derive(Debug, Clone)]
pub enum BackupTarget {
    S3(S3Options),
    /// A directory on a filesystem this host can see, normally a mount point.
    Local(PathBuf),
}

impl BackupTarget {
    /// Open the `object_store` this target names.
    ///
    /// Opening is deliberately **not** cached anywhere. The backup manager
    /// rebuilds its backend on every run (`manager::request_run`), so a local
    /// target's directory is re-canonicalized each time and a target that is
    /// not currently attached fails the run synchronously, before anything is
    /// paused. Caching the store would silently remove that probe.
    pub fn build_store(&self) -> BichonResult<Arc<dyn ObjectStore>> {
        match self {
            BackupTarget::S3(opts) => s3_store(opts),
            BackupTarget::Local(root) => BackupEngine::local_store(root),
        }
    }

    /// Short backend name for logs and status views.
    pub fn kind(&self) -> &'static str {
        match self {
            BackupTarget::S3(_) => "s3",
            BackupTarget::Local(_) => "local",
        }
    }

    /// Where this target points, for logs and error messages. The object-store
    /// prefix is *not* included — it is a layout concern, not a target one.
    pub fn display(&self) -> String {
        match self {
            BackupTarget::S3(o) => {
                let host = if o.endpoint.is_empty() {
                    "aws"
                } else {
                    o.endpoint.as_str()
                };
                format!("s3://{host}/{}", o.bucket)
            }
            BackupTarget::Local(p) => p.display().to_string(),
        }
    }
}

/// Map an `object_store` local-backend failure onto a message an operator can
/// act on.
///
/// The raw errors are precise but not instructive ("Unable to canonicalize
/// path"), and the question behind almost every local-target failure is "is
/// the volume still mounted, writable, and not full?". Keyed on the rendered
/// error text, matching `admin::restore::diagnose_connection_error`.
fn local_store_err(what: &str, root: &Path, e: object_store::Error) -> crate::error::BichonError {
    let text = e.to_string().to_lowercase();
    let hint = if text.contains("canonicalize") || text.contains("no such file") {
        "the directory does not exist or is not reachable — is the volume still mounted?"
    } else if text.contains("permission denied") || text.contains("os error 13") {
        "the directory is not writable by the Bichon process"
    } else if text.contains("read-only") || text.contains("os error 30") {
        "the filesystem is mounted read-only"
    } else if text.contains("no space left") || text.contains("os error 28") {
        "the filesystem is full"
    } else if text.contains("stale file handle") || text.contains("os error 116") {
        "the network filesystem handle went stale — remount the target"
    } else {
        "check that the volume is mounted, writable and has free space"
    };
    raise_error!(
        format!(
            "backup: {what} failed for {}: {e} — {hint}",
            root.display()
        ),
        ErrorCode::InternalError
    )
}

/// Content-addressed backup engine. All state lives under `<prefix>/v1/`.
pub struct BackupEngine {
    store: Arc<dyn ObjectStore>,
    /// `<prefix>/v1` — object keys in manifests are relative to this.
    base: StorePath,
    meta_prefix: StorePath,
    /// `<prefix>/v1/obj` — listing prefix for the object store.
    obj_prefix: StorePath,
}

impl BackupEngine {
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        let base = StorePath::parse(format!("{prefix}/{LAYOUT_DIR}")).unwrap_or_else(|e| {
            // Unreachable through the validated config path (`config::validate`
            // rejects unusable prefixes at save time); a legacy value can still
            // land here. Never silently operate at the fallback location.
            tracing::error!(
                "backup: prefix '{prefix}' is not a valid object-store path ({e}); \
                 falling back to 'v1' — backups will NOT land where the config points"
            );
            StorePath::from("v1")
        });
        let meta = base.clone().join("meta");
        let obj = base.clone().join("obj");
        Self {
            store,
            base,
            meta_prefix: meta,
            obj_prefix: obj,
        }
    }

    /// Open a local `object_store` over a directory.
    ///
    /// The directory **must already exist**. `LocalFileSystem::new_with_prefix`
    /// canonicalizes it and fails otherwise, and that failure is load-bearing:
    /// it must never be papered over by creating the directory here, because
    /// doing so silently redirects backups onto whatever filesystem happens to
    /// be under the path. Note this only covers a path that is *absent* — a
    /// mount point whose volume is not mounted still exists, so
    /// `config::target` adds the volume-fingerprint check that `canonicalize`
    /// alone cannot make.
    ///
    /// Durability is on unconditionally — a backup target must not lose a
    /// write it reported as successful to a power cut. `with_fsync` also
    /// fsyncs the parent directory after each publish (on Unix), and
    /// `with_automatic_cleanup` prunes directories emptied by GC. Neither
    /// can ever remove the configured root itself.
    pub fn local_store(root: &Path) -> BichonResult<Arc<dyn ObjectStore>> {
        let store = object_store::local::LocalFileSystem::new_with_prefix(root)
            .map_err(|e| local_store_err("open local store", root, e))?
            .with_fsync(true)
            .with_automatic_cleanup(true);
        Ok(Arc::new(store))
    }

    /// Test/dev helper: create the directory, then open it exactly as
    /// production does, so tests exercise the same fsync path.
    #[cfg(test)]
    pub fn local_store_for_test(root: &Path) -> BichonResult<Arc<dyn ObjectStore>> {
        std::fs::create_dir_all(root).map_err(|e| {
            raise_error!(
                format!("backup: cannot create local store {}: {e}", root.display()),
                ErrorCode::InternalError
            )
        })?;
        Self::local_store(root)
    }

    /// An in-memory store for tests.
    #[cfg(test)]
    pub fn memory_store() -> Arc<dyn ObjectStore> {
        Arc::new(object_store::memory::InMemory::new())
    }

    /// Test helper: upload raw bytes as one content-addressed object and
    /// return its key (`obj/sha256/<hex>`).
    #[cfg(test)]
    pub async fn upload_bytes_for_test(&self, bytes: &[u8]) -> BichonResult<String> {
        let dir = std::env::temp_dir().join(format!(
            "bichon-engine-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Unique file per call: parallel tests must never share a temp path.
        let path = dir.join(format!(
            "bytes-{}-{:x}.bin",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
            bytes.len()
        ));
        std::fs::write(&path, bytes).unwrap();
        let sha256 = crate::backup::prepare::sha256_file(&path)?;
        let art = Artifact {
            local: path,
            logical: crate::backup::artifact::ArtifactKey::BlobSegment(0),
            sha256,
            bytes: bytes.len() as u64,
        };
        let mut esum = EngineSummary::default();
        self.upload_artifact(&art, &mut esum).await?;
        Ok(Manifest::object_key(&sha256))
    }

    // ── preflight ─────────────────────────────────────────────────────────

    /// Verify the target is reachable *and* writable before anything is
    /// paused, so a misconfigured bucket never stalls the server. Writes a
    /// marker object and removes it again.
    pub async fn preflight(&self) -> BichonResult<()> {
        let marker = self.meta_path(&format!("preflight-{}", uuid::Uuid::new_v4()));
        self.store
            .put(
                &marker,
                PutPayload::from_bytes(bytes::Bytes::from_static(b"bichon")),
            )
            .await
            .map_err(|e| self.store_err("preflight write", e))?;
        self.store
            .delete(&marker)
            .await
            .map_err(|e| self.store_err("preflight cleanup", e))
    }

    // ── upload ────────────────────────────────────────────────────────────

    /// Upload one artifact, content-addressed. Objects already present are
    /// skipped (identical content ⇒ identical key), so an unchanged run
    /// uploads nothing (R8). A present object whose size disagrees is
    /// considered corrupt and re-uploaded (never silently skip).
    pub async fn upload_artifact(
        &self,
        art: &Artifact,
        summary: &mut EngineSummary,
    ) -> BichonResult<ObjectRef> {
        let key = Manifest::object_key(&art.sha256);
        let path = self.obj_path(&key)?;
        let kind = art.logical.label().to_string();
        match self.store.head(&path).await {
            Ok(meta) => {
                if meta.size == art.bytes {
                    summary.skipped_objects += 1;
                    let stat = stat_mut(summary, &kind);
                    stat.skipped_count += 1;
                    stat.skipped_bytes += art.bytes;
                    return Ok(self.to_ref(&art, key));
                }
                tracing::warn!(
                    "backup: object {} exists with size {}, expected {}; re-uploading",
                    path,
                    meta.size,
                    art.bytes
                );
            }
            Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(self.store_err("head", e)),
        }
        self.put_file(&path, &art.local, art.bytes).await?;
        summary.new_objects += 1;
        summary.uploaded_bytes += art.bytes;
        let stat = stat_mut(summary, &kind);
        stat.new_count += 1;
        stat.new_bytes += art.bytes;
        Ok(self.to_ref(&art, key))
    }

    fn to_ref(&self, art: &Artifact, key: String) -> ObjectRef {
        ObjectRef {
            key,
            sha256: hex::encode(art.sha256),
            bytes: art.bytes,
        }
    }

    /// Stream a local file to an object — single PUT for small files,
    /// multipart for large ones.
    async fn put_file(&self, path: &StorePath, local: &Path, size: u64) -> BichonResult<()> {
        if size <= SINGLE_PUT_MAX {
            let data = tokio::fs::read(local).await.map_err(|e| {
                raise_error!(
                    format!("backup: cannot read {} for upload: {e}", local.display()),
                    ErrorCode::InternalError
                )
            })?;
            // The manifest will reference `size` bytes under a key derived
            // from the full-content hash; committing a short object would
            // make the restore point unusable, discovered only at restore.
            if data.len() as u64 != size {
                return Err(raise_error!(
                    format!(
                        "backup: {} changed size during capture ({} bytes, expected {size})",
                        local.display(),
                        data.len()
                    ),
                    ErrorCode::InternalError
                ));
            }
            self.store
                .put(path, PutPayload::from_bytes(data.into()))
                .await
                .map_err(|e| self.store_err("upload", e))?;
            Ok(())
        } else {
            self.put_file_multipart(path, local, size).await
        }
    }

    async fn put_file_multipart(
        &self,
        path: &StorePath,
        local: &Path,
        size: u64,
    ) -> BichonResult<()> {
        let mut upload = self
            .store
            .put_multipart(path)
            .await
            .map_err(|e| self.store_err("multipart start", e))?;
        let mut file = tokio::fs::File::open(local).await.map_err(|e| {
            raise_error!(
                format!("backup: cannot open {} for upload: {e}", local.display()),
                ErrorCode::InternalError
            )
        })?;
        let mut buf = vec![0u8; UPLOAD_PART_SIZE];
        let mut uploaded: u64 = 0;
        while uploaded < size {
            // Fill the part buffer completely before uploading it. `read`
            // may return short counts (notably on Windows), and MinIO
            // rejects any *non-final* part below 5 MiB with `EntityTooSmall`
            // on complete — so every part must be a full UPLOAD_PART_SIZE
            // unless it is the final partial read at EOF.
            let mut filled = 0usize;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..]).await.map_err(|e| {
                    raise_error!(
                        format!("backup: cannot read {} during upload: {e}", local.display()),
                        ErrorCode::InternalError
                    )
                })?;
                if n == 0 {
                    break; // EOF — this is the final (possibly short) part.
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            upload
                .put_part(PutPayload::from_bytes(buf[..filled].to_vec().into()))
                .await
                .map_err(|e| self.store_err("multipart part", e))?;
            uploaded += filled as u64;
        }
        // The manifest will reference `size` bytes; committing a truncated
        // multipart object would only surface at restore time. Dropping the
        // upload aborts it server-side.
        if uploaded != size {
            return Err(raise_error!(
                format!(
                    "backup: {} changed size during capture ({} bytes uploaded, expected {size})",
                    local.display(),
                    uploaded
                ),
                ErrorCode::InternalError
            ));
        }
        upload
            .complete()
            .await
            .map_err(|e| self.store_err("multipart complete", e))?;
        Ok(())
    }

    // ── commit ────────────────────────────────────────────────────────────

    /// Atomically commit a restore point: manifest object first, then LATEST.
    /// A crash between the two leaves only an orphan manifest and unreferenced
    /// objects, reclaimed by the next GC pass (R4).
    pub async fn commit(&self, manifest: &Manifest) -> BichonResult<()> {
        let bytes = manifest.encode_gz()?;
        let manifest_path = self.meta_path(&format!("{MANIFEST_PREFIX}{}.json.gz", manifest.id));
        self.store
            .put(&manifest_path, PutPayload::from_bytes(bytes.into()))
            .await
            .map_err(|e| self.store_err("manifest write", e))?;
        let latest = serde_json::json!({
            "manifest": format!("{MANIFEST_PREFIX}{}.json.gz", manifest.id)
        });
        self.store
            .put(
                &self.meta_path(LATEST_NAME),
                PutPayload::from_bytes(latest.to_string().into_bytes().into()),
            )
            .await
            .map_err(|e| self.store_err("LATEST write", e))?;
        Ok(())
    }

    // ── read / list ───────────────────────────────────────────────────────

    /// The newest committed manifest, following LATEST (`None` when no backup
    /// has ever committed).
    pub async fn load_latest(&self) -> BichonResult<Option<Manifest>> {
        let path = self.meta_path(LATEST_NAME);
        let data = match self.get_object(&path).await {
            Ok(d) => d,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(self.store_err("LATEST read", e)),
        };
        let latest: serde_json::Value = serde_json::from_slice(&data).map_err(|e| {
            raise_error!(
                format!("backup: LATEST is corrupt: {e}"),
                ErrorCode::InternalError
            )
        })?;
        let name = latest.get("manifest").and_then(|v| v.as_str()).ok_or_else(|| {
            raise_error!(
                "backup: LATEST has no manifest pointer".to_string(),
                ErrorCode::InternalError
            )
        })?;
        let bytes = self
            .get_object(&self.meta_path(name))
            .await
            .map_err(|e| self.store_err("manifest read", e))?;
        Ok(Some(Manifest::decode_gz(&bytes)?))
    }

    /// Load a manifest by id (`None` when it does not exist).
    pub async fn load_by_id(&self, id: &str) -> BichonResult<Option<Manifest>> {
        let path = self.meta_path(&format!("{MANIFEST_PREFIX}{id}.json.gz"));
        match self.get_object(&path).await {
            Ok(bytes) => Ok(Some(Manifest::decode_gz(&bytes)?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(self.store_err("manifest read", e)),
        }
    }

    /// All committed manifests, newest first, following the `previous` chain
    /// from LATEST. A broken link (a predecessor already reclaimed) ends the
    /// walk; a cycle is guarded against.
    pub async fn load_chain(&self) -> BichonResult<Vec<Manifest>> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut next = self.load_latest().await?;
        while let Some(m) = next {
            if !seen.insert(m.id.clone()) {
                break;
            }
            let previous = m.previous.clone();
            out.push(m);
            next = match previous {
                Some(id) => self.load_by_id(&id).await?,
                None => None,
            };
        }
        Ok(out)
    }

    /// Every manifest stored under `meta/`, regardless of `previous` linkage.
    /// Manifests that fail to decode (a torn write from an interrupted run)
    /// are skipped with a warning — GC removes them as garbage.
    async fn load_all_manifests(&self) -> BichonResult<Vec<Manifest>> {
        let mut out = Vec::new();
        for name in self.list_meta_names().await? {
            let Some(id) = name
                .strip_prefix(MANIFEST_PREFIX)
                .and_then(|n| n.strip_suffix(".json.gz"))
            else {
                continue;
            };
            match self.load_by_id(&id).await {
                Ok(Some(m)) => out.push(m),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("backup: skipping undecodable manifest {id}: {e}");
                }
            }
        }
        Ok(out)
    }

    /// Every usable restore point, newest first.
    ///
    /// The LATEST walk comes first; manifests unreachable through `previous`
    /// links trail after it. Unreachability is expected after non-contiguous
    /// retention deletes an interior manifest (older *retained* restore points
    /// sit behind the break) and after an interrupted run (its manifest was
    /// written but LATEST never advanced). Both kinds are valid restore
    /// points and must stay visible and selectable.
    pub async fn list_restore_points(&self) -> BichonResult<Vec<Manifest>> {
        let chain = self.load_chain().await?;
        let chain_ids: HashSet<String> = chain.iter().map(|m| m.id.clone()).collect();
        let mut rest = self.load_all_manifests().await?;
        rest.retain(|m| !chain_ids.contains(&m.id));
        // `created` is RFC 3339 UTC, so lexicographic order is chronological;
        // id breaks ties. These trail the chain, so retention's positional
        // `keep_last` still prefers committed history over them.
        rest.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        let mut out = chain;
        out.extend(rest);
        Ok(out)
    }

    /// Every object under `meta/` (manifests + LATEST), leaf names only.
    async fn list_meta_names(&self) -> BichonResult<Vec<String>> {
        let mut names = Vec::new();
        let mut stream = self.store.list(Some(&self.meta_prefix));
        while let Some(meta) = stream
            .next()
            .await
            .transpose()
            .map_err(|e| self.store_err("list meta", e))?
        {
            if let Some(name) = meta.location.filename() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    // ── GC ────────────────────────────────────────────────────────────────

    /// Reclaim everything not reachable from the retained restore points
    /// (R6): delete expired and orphan manifests, then delete objects not
    /// referenced by any retained manifest. Runs *after* a commit, so at
    /// least one manifest is always protected.
    pub async fn gc(&self, policy: &RetentionPolicy) -> BichonResult<GcStats> {
        // Selection must cover *all* manifests on disk, not just the LATEST
        // walk: non-contiguous retention deletes interior manifests, which
        // breaks `previous` links and strands older retained restore points
        // behind the break. Deleting "anything the chain doesn't reach" would
        // then destroy restore points the policy explicitly kept.
        let candidates = self.list_restore_points().await?;

        // Defensive: a policy that keeps nothing would wipe every restore
        // point, including the one committed seconds ago, and wedge every
        // future run (LATEST would dangle). Clamp so the newest survives.
        let mut effective = *policy;
        if effective.keep_last == 0
            && effective.keep_daily == 0
            && effective.keep_weekly == 0
            && effective.keep_monthly == 0
        {
            tracing::warn!(
                "backup: retention policy keeps nothing; clamping keep_last to 1"
            );
            effective.keep_last = 1;
        }
        let mut keep_ids = select_manifests(&candidates, &effective);

        // The manifest LATEST points to is always protected, whatever the
        // ordering above did with it.
        if let Some(latest) = self.load_latest().await? {
            keep_ids.insert(latest.id);
        }
        let mut stats = GcStats::default();

        // Delete manifests that are neither retained (expired) nor committed
        // (orphans from interrupted runs).
        for name in self.list_meta_names().await? {
            if name == LATEST_NAME {
                continue;
            }
            let Some(id) = name
                .strip_prefix(MANIFEST_PREFIX)
                .and_then(|n| n.strip_suffix(".json.gz"))
            else {
                continue;
            };
            if keep_ids.contains(id) {
                continue;
            }
            let path = self.meta_path(&name);
            self.store
                .delete(&path)
                .await
                .map_err(|e| self.store_err("manifest delete", e))?;
            stats.manifests_removed += 1;
        }

        // Delete objects not referenced by any retained manifest. The size is
        // reported by the listing, so this is the only place bytes are freed.
        self.reclaim_unreferenced_objects(&mut stats).await?;

        Ok(stats)
    }

    /// Delete every object under `obj/` that no manifest currently on disk
    /// references, accumulating into `stats`. Shared content-addressed
    /// objects referenced by surviving restore points are never touched.
    async fn reclaim_unreferenced_objects(&self, stats: &mut GcStats) -> BichonResult<()> {
        let mut referenced: HashSet<String> = HashSet::new();
        for m in self.load_all_manifests().await? {
            for key in m.object_keys() {
                referenced.insert(self.obj_path(key)?.to_string());
            }
        }
        let mut stream = self.store.list(Some(&self.obj_prefix));
        while let Some(meta) = stream
            .next()
            .await
            .transpose()
            .map_err(|e| self.store_err("list objects", e))?
        {
            let key = meta.location.to_string();
            if referenced.contains(&key) {
                continue;
            }
            self.store
                .delete(&meta.location)
                .await
                .map_err(|e| self.store_err("object delete", e))?;
            stats.objects_removed += 1;
            stats.bytes_reclaimed += meta.size;
        }
        Ok(())
    }

    /// Manually delete one restore point (manifest browser "delete"): remove
    /// its manifest, then reclaim every object only it referenced. Objects
    /// shared with surviving restore points stay. When the deleted manifest
    /// is the one `LATEST` points to, `LATEST` is repointed to the newest
    /// remaining manifest (or removed when none remain), so a later run
    /// simply chains from there instead of failing on a dangling pointer.
    ///
    /// Callers must hold the run lock: a concurrent run uploads and commits
    /// while we reclaim, and the reclaim would see the run's not-yet-
    /// committed objects as garbage.
    pub async fn delete_restore_point(&self, id: &str) -> BichonResult<GcStats> {
        let manifest = self.load_by_id(id).await?.ok_or_else(|| {
            raise_error!(
                format!("backup: restore point {id} not found"),
                ErrorCode::ResourceNotFound
            )
        })?;
        let mut stats = GcStats::default();

        // What LATEST points at, read raw (a dangling LATEST must not fail
        // the load — we may be about to fix it).
        let latest_path = self.meta_path(LATEST_NAME);
        let latest_name: Option<String> = match self.get_object(&latest_path).await {
            Ok(data) => serde_json::from_slice::<serde_json::Value>(&data)
                .ok()
                .and_then(|v| {
                    v.get("manifest")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                }),
            Err(object_store::Error::NotFound { .. }) => None,
            Err(e) => return Err(self.store_err("LATEST read", e)),
        };
        let manifest_meta = format!("{MANIFEST_PREFIX}{}.json.gz", manifest.id);
        let was_latest = latest_name.as_deref() == Some(manifest_meta.as_str());

        // Delete the manifest itself.
        self.store
            .delete(&self.meta_path(&manifest_meta))
            .await
            .map_err(|e| self.store_err("manifest delete", e))?;
        stats.manifests_removed = 1;

        // Repoint (or drop) LATEST when it pointed at the deleted manifest.
        if was_latest {
            let mut remaining = self.load_all_manifests().await?;
            // `created` is RFC 3339 UTC, so lexicographic order is
            // chronological; id breaks ties (same rule as list_restore_points).
            remaining.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
            match remaining.first() {
                Some(newest) => {
                    let latest = serde_json::json!({ "manifest": format!("{MANIFEST_PREFIX}{}.json.gz", newest.id) });
                    self.store
                        .put(
                            &latest_path,
                            PutPayload::from_bytes(latest.to_string().into_bytes().into()),
                        )
                        .await
                        .map_err(|e| self.store_err("LATEST write", e))?;
                }
                None => {
                    self.store
                        .delete(&latest_path)
                        .await
                        .map_err(|e| self.store_err("LATEST delete", e))?;
                }
            }
        }

        // Reclaim objects only this manifest referenced (the manifest is
        // already gone, so load_all_manifests no longer sees it).
        self.reclaim_unreferenced_objects(&mut stats).await?;
        Ok(stats)
    }

    // ── verify (restore drill) ────────────────────────────────────────────

    /// Check that every object a manifest references exists and has the
    /// recorded size. Returns a list of problems (empty = the restore point
    /// is complete). Used by the periodic drill and `--verify-only`.
    pub async fn verify_manifest(&self, manifest: &Manifest) -> BichonResult<Vec<String>> {
        let mut problems = Vec::new();
        for (key, expected_bytes) in manifest.object_refs() {
            let path = self.obj_path(key)?;
            match self.store.head(&path).await {
                Ok(meta) => {
                    if meta.size != expected_bytes {
                        problems.push(format!(
                            "size mismatch: {key} has {} bytes, manifest says {expected_bytes}",
                            meta.size
                        ));
                    }
                }
                Err(object_store::Error::NotFound { .. }) => {
                    problems.push(format!("missing: {key}"));
                }
                Err(e) => return Err(self.store_err("verify head", e)),
            }
        }
        Ok(problems)
    }

    // ── restore (download) ────────────────────────────────────────────────

    /// Stream one object to a local file, verifying its SHA-256 and byte
    /// size *while writing* (R11: a corrupt object is never silently
    /// restored). On any mismatch the partial file is removed and an error
    /// returned. A multi-hundred-MB segment never sits in memory.
    pub async fn download_object(
        &self,
        key: &str,
        dest: &Path,
        expected_sha256: &str,
        expected_bytes: u64,
    ) -> BichonResult<()> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                raise_error!(
                    format!("restore: cannot create {}: {e}", parent.display()),
                    ErrorCode::InternalError
                )
            })?;
        }
        let path = self.obj_path(key)?;
        let mut stream = self
            .store
            .get(&path)
            .await
            .map_err(|e| self.store_err("object read", e))?
            .into_stream();
        let mut file = tokio::fs::File::create(dest).await.map_err(|e| {
            raise_error!(
                format!("restore: cannot create {}: {e}", dest.display()),
                ErrorCode::InternalError
            )
        })?;
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        let mut total: u64 = 0;
        let write_result: BichonResult<()> = async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| self.store_err("object stream", e))?;
                ctx.update(&chunk);
                total += chunk.len() as u64;
                file.write_all(&chunk).await.map_err(|e| {
                    raise_error!(
                        format!("restore: cannot write {}: {e}", dest.display()),
                        ErrorCode::InternalError
                    )
                })?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = write_result {
            let _ = std::fs::remove_file(dest);
            return Err(e);
        }
        if total != expected_bytes {
            let _ = std::fs::remove_file(dest);
            return Err(raise_error!(
                format!(
                    "restore: object {key} has {total} bytes, manifest says {expected_bytes}"
                ),
                ErrorCode::InternalError
            ));
        }
        let digest = ctx.finish();
        let hex_hash = hex::encode(digest.as_ref());
        if hex_hash != expected_sha256 {
            let _ = std::fs::remove_file(dest);
            return Err(raise_error!(
                format!(
                    "restore: object {key} hash mismatch: got {hex_hash}, manifest says {expected_sha256}"
                ),
                ErrorCode::InternalError
            ));
        }
        Ok(())
    }

    /// Stream one object and return its SHA-256, verifying the byte count —
    /// the drill-mode integrity check. Nothing touches disk, so
    /// `--verify-only` is a true dry run (design doc §8).
    pub async fn hash_object(&self, key: &str, expected_bytes: u64) -> BichonResult<[u8; 32]> {
        let path = self.obj_path(key)?;
        let mut stream = self
            .store
            .get(&path)
            .await
            .map_err(|e| self.store_err("object read", e))?
            .into_stream();
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| self.store_err("object stream", e))?;
            ctx.update(&chunk);
            total += chunk.len() as u64;
        }
        if total != expected_bytes {
            return Err(raise_error!(
                format!(
                    "restore: object {key} has {total} bytes, manifest says {expected_bytes}"
                ),
                ErrorCode::InternalError
            ));
        }
        let digest = ctx.finish();
        let mut out = [0u8; 32];
        out.copy_from_slice(digest.as_ref());
        Ok(out)
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Full store path of an object given its manifest key (`obj/sha256/…`).
    /// Built via `parse` (not `join`) because `PathPart` percent-encodes `/`.
    fn obj_path(&self, key: &str) -> BichonResult<StorePath> {
        StorePath::parse(format!("{}/{}", self.base, key)).map_err(|e| {
            raise_error!(
                format!("backup: invalid object key {key}: {e}"),
                ErrorCode::InternalError
            )
        })
    }

    /// Full store path of an object under `meta/` (single segment leaf names).
    fn meta_path(&self, name: &str) -> StorePath {
        self.meta_prefix.clone().join(name)
    }

    /// Read one object fully into memory, verifying its SHA-256 and byte size
    /// (R11: a corrupt object is never silently served — the manifest browser
    /// uses this to hand out audit delta JSONL.gz).
    pub async fn read_object(
        &self,
        key: &str,
        expected_sha256: &str,
        expected_bytes: u64,
    ) -> BichonResult<Vec<u8>> {
        let path = self.obj_path(key)?;
        let bytes = self
            .get_object(&path)
            .await
            .map_err(|e| self.store_err("object read", e))?;
        if bytes.len() as u64 != expected_bytes {
            return Err(raise_error!(
                format!(
                    "backup: object {key} has {} bytes, manifest says {expected_bytes}",
                    bytes.len()
                ),
                ErrorCode::InternalError
            ));
        }
        let got = hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes));
        if got != expected_sha256 {
            return Err(raise_error!(
                format!("backup: object {key} hash mismatch (R11)"),
                ErrorCode::InternalError
            ));
        }
        Ok(bytes.to_vec())
    }

    async fn get_object(&self, path: &StorePath) -> Result<bytes::Bytes, object_store::Error> {
        self.store.get(path).await?.bytes().await
    }

    fn store_err(&self, what: &str, e: impl std::fmt::Display) -> crate::error::BichonError {
        raise_error!(
            format!("backup: {what} failed: {e}"),
            ErrorCode::InternalError
        )
    }
}

impl BackupTarget {
    /// Remove abandoned staging files left behind by an interrupted put.
    ///
    /// `object_store` writes each object to `<dest>#<n>` beside its
    /// destination and renames it into place. If the process is killed, the
    /// volume goes away, or the box loses power mid-put, that file stays — and it
    /// stays forever: `LocalFileSystem` treats `*#<digits>` as an invalid
    /// object name, so it is invisible to `list` and neither
    /// `list_meta_names` nor `reclaim_unreferenced_objects` will ever see it.
    /// Walking the tree directly is the only way to reclaim that space.
    ///
    /// Best-effort by design — a sweep that fails must never fail the run.
    /// Returns the number of files removed (0 for an S3 target).
    pub fn sweep_staging(&self, prefix: &str) -> u64 {
        let BackupTarget::Local(root) = self else {
            return 0;
        };
        let base = root.join(prefix).join(LAYOUT_DIR);
        let mut removed = 0;
        for sub in ["obj", "meta"] {
            removed += sweep_staging_in(&base.join(sub));
        }
        if removed > 0 {
            tracing::info!(
                "backup: reclaimed {removed} abandoned staging file(s) under {}",
                base.display()
            );
        }
        removed
    }
}

/// Recursively remove `*#<digits>` files under `dir` whose mtime is older than
/// [`STAGING_MAX_AGE`]. A file whose age cannot be read is left alone.
fn sweep_staging_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            removed += sweep_staging_in(&path);
            continue;
        }
        if !is_staging_name(&path) {
            continue;
        }
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age >= STAGING_MAX_AGE);
        if old_enough && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `object_store` stages an upload at `<dest>#<n>` (see `staged_upload_path`
/// in its `local.rs`).
fn is_staging_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once('#'))
        .is_some_and(|(_, suffix)| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::artifact::ArtifactKey;

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        // Unique per call: the directory only keys on pid + name, so parallel
        // tests writing the same file name would share one path and truncate
        // each other's file mid-capture, surfacing as spurious "changed size
        // during capture" upload failures.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bichon-engine-test-{}-{}-{}",
            std::process::id(),
            seq,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn artifact_at(path: PathBuf, logical: ArtifactKey, byte: u8) -> Artifact {
        let bytes = std::fs::metadata(&path).unwrap().len();
        Artifact {
            local: path,
            logical,
            sha256: [byte; 32],
            bytes,
        }
    }

    fn engine(store: Arc<dyn ObjectStore>) -> BackupEngine {
        BackupEngine::new(store, "test")
    }

    /// Upload every artifact and commit a restore point in one step — the
    /// real manager sequence (upload first, manifest last).
    async fn commit_run(
        e: &BackupEngine,
        id: &str,
        previous: Option<&str>,
        arts: &[Artifact],
    ) -> Manifest {
        let mut s = EngineSummary::default();
        for a in arts {
            e.upload_artifact(a, &mut s).await.unwrap();
        }
        let mut m = Manifest::new(id.to_string(), "manual", previous.map(str::to_string));
        for a in arts {
            m.add_artifact(a).unwrap();
        }
        e.commit(&m).await.unwrap();
        m
    }

    /// Full store path for a manifest-format key under the test engine root.
    fn store_path(key: &str) -> StorePath {
        StorePath::parse(format!("test/v1/{key}")).unwrap()
    }

    /// Endpoints are advertised without a scheme (Backblaze's docs, MinIO's
    /// console); a bare host must become an https endpoint, not an object key
    /// that fails at the first request.
    #[test]
    fn endpoint_normalization_adds_a_missing_scheme_and_trims() {
        assert_eq!(
            normalize_endpoint("s3.us-west-004.backblazeb2.com"),
            "https://s3.us-west-004.backblazeb2.com"
        );
        assert_eq!(
            normalize_endpoint("  https://s3.us-west-004.backblazeb2.com/  "),
            "https://s3.us-west-004.backblazeb2.com"
        );
        // An explicit scheme is kept as-is (http stays http, so `allow_http`
        // still applies to MinIO-style endpoints).
        assert_eq!(
            normalize_endpoint("http://minio.internal:9000"),
            "http://minio.internal:9000"
        );
        assert_eq!(
            normalize_endpoint("minio.internal:9000"),
            "https://minio.internal:9000"
        );
    }

    /// A bare-host endpoint must produce a usable client, not fail at build.
    #[test]
    fn s3_store_accepts_a_scheme_less_endpoint() {
        let opts = S3Options {
            endpoint: "s3.us-west-004.backblazeb2.com".to_string(),
            bucket: "backup-bucket".to_string(),
            region: "us-west-004".to_string(),
            access_key: "key-id".to_string(),
            secret_key: "secret".to_string(),
            force_path_style: true,
        };
        assert!(
            s3_store(&opts).is_ok(),
            "a bare host must not fail client construction"
        );
    }

    #[tokio::test]
    async fn preflight_writes_and_cleans_marker() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        e.preflight().await.unwrap();
        // No leftovers under meta/.
        let names = e.list_meta_names().await.unwrap();
        assert!(names.is_empty(), "preflight must leave nothing behind: {names:?}");
    }

    #[tokio::test]
    async fn upload_skips_existing_content_addressed_objects() {
        let store = BackupEngine::memory_store();
        let e = engine(store);
        let path = write_temp("a.txt", b"hello world");
        let art = artifact_at(path, ArtifactKey::Memdb, 0x11);

        let mut s = EngineSummary::default();
        let o1 = e.upload_artifact(&art, &mut s).await.unwrap();
        assert_eq!(s.new_objects, 1);
        assert_eq!(s.skipped_objects, 0);

        let mut s2 = EngineSummary::default();
        let o2 = e.upload_artifact(&art, &mut s2).await.unwrap();
        assert_eq!(s2.new_objects, 0);
        assert_eq!(s2.skipped_objects, 1);
        assert_eq!(o1, o2, "identical content must resolve to the same object");
    }

    #[tokio::test]
    async fn upload_tracks_per_kind_stats() {
        let store = BackupEngine::memory_store();
        let e = engine(store);
        let mem = artifact_at(
            write_temp("m.txt", b"memdb snapshot"),
            ArtifactKey::Memdb,
            0x11,
        );
        let seg = artifact_at(
            write_temp("s.txt", b"blob segment"),
            ArtifactKey::BlobSegment(1),
            0x22,
        );
        let mut s = EngineSummary::default();
        e.upload_artifact(&mem, &mut s).await.unwrap();
        e.upload_artifact(&seg, &mut s).await.unwrap();
        // Same memdb content again → content-addressed skip, grouped under
        // the same component.
        e.upload_artifact(&mem, &mut s).await.unwrap();
        assert_eq!(s.new_objects, 2);
        assert_eq!(s.skipped_objects, 1);
        assert_eq!(s.by_kind.len(), 2, "{:?}", s.by_kind);
        let mem_stat = s.by_kind.iter().find(|x| x.kind == "memdb").unwrap();
        assert_eq!(mem_stat.new_count, 1);
        assert_eq!(mem_stat.new_bytes, "memdb snapshot".len() as u64);
        assert_eq!(mem_stat.skipped_count, 1);
        assert_eq!(mem_stat.skipped_bytes, "memdb snapshot".len() as u64);
        let seg_stat = s.by_kind.iter().find(|x| x.kind == "blob").unwrap();
        assert_eq!(seg_stat.new_count, 1);
        assert_eq!(seg_stat.skipped_count, 0);
        // Different segment ids collapse into the same "blob" group.
        assert!(!s.by_kind.iter().any(|x| x.kind == "blob" && x.new_count > 1));
    }

    #[tokio::test]
    async fn reuploads_when_existing_object_size_mismatches() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        let path = write_temp("b.txt", b"hello");
        let art = artifact_at(path, ArtifactKey::Memdb, 0x22);
        // Corrupt the store behind the engine's back: an object with the same
        // key but wrong size.
        let key = Manifest::object_key(&art.sha256);
        store
            .put(
                &store_path(&key),
                PutPayload::from_bytes(bytes::Bytes::from_static(b"wrong-size")),
            )
            .await
            .unwrap();

        let mut s = EngineSummary::default();
        e.upload_artifact(&art, &mut s).await.unwrap();
        assert_eq!(s.new_objects, 1, "mismatched object must be re-uploaded");
        assert_eq!(s.skipped_objects, 0);
    }

    #[tokio::test]
    async fn multipart_upload_round_trips_a_file_larger_than_one_part() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());

        // Just over one 16 MiB part: the upload must take the multipart path
        // (files ≤ SINGLE_PUT_MAX go single PUT). Covers the part-filling loop
        // that guarantees every non-final part is a full 16 MiB.
        let size = (16 * 1024 * 1024) + 7;
        let content = vec![0xABu8; size];
        let path = write_temp("multipart.bin", &content);
        let art = artifact_at(path, ArtifactKey::BlobSegment(1), 0x61);

        let mut s = EngineSummary::default();
        let o = e.upload_artifact(&art, &mut s).await.unwrap();
        assert_eq!(s.new_objects, 1, "a fresh object must be uploaded");

        let got = store.get(&store_path(&o.key)).await.unwrap().bytes().await.unwrap();
        assert_eq!(got.len(), size, "uploaded object must carry every byte");
        assert_eq!(
            got.as_ref(),
            content.as_slice(),
            "multipart upload must round-trip the file byte-for-byte"
        );
    }

    #[tokio::test]
    async fn commit_makes_restore_point_and_latest() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        let mut m = Manifest::new("1".to_string(), "manual", None);
        let path = write_temp("c.txt", b"memdb snapshot");
        m.add_artifact(&artifact_at(path, ArtifactKey::Memdb, 0x33))
            .unwrap();

        e.commit(&m).await.unwrap();
        let latest = e.load_latest().await.unwrap().expect("LATEST must exist");
        assert_eq!(latest.id, "1");
        assert_eq!(latest.objects.memdb.as_ref().unwrap().bytes, 14);

        // The manifest object (`m-<id>.json.gz`) and LATEST both exist.
        let names = e.list_meta_names().await.unwrap();
        assert!(names.contains(&"m-1.json.gz".to_string()), "{names:?}");
        assert!(names.contains(&LATEST_NAME.to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn orphan_manifest_without_latest_is_invisible() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        // Simulate a crash between manifest write and LATEST write.
        let mut m = Manifest::new("orphan".to_string(), "manual", None);
        let path = write_temp("d.txt", b"data");
        m.add_artifact(&artifact_at(path, ArtifactKey::Memdb, 0x44))
            .unwrap();
        let bytes = m.encode_gz().unwrap();
        let p = StorePath::parse("test/v1/meta/m-orphan.json.gz").unwrap();
        store
            .put(&p, PutPayload::from_bytes(bytes.into()))
            .await
            .unwrap();

        assert!(e.load_latest().await.unwrap().is_none());
        assert!(e.load_chain().await.unwrap().is_empty());
        assert!(e.load_by_id("orphan").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn gc_removes_orphans_and_expired_manifests_but_keeps_shared_objects() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());

        // Run 1: memdb + segment A.
        let arts1 = vec![
            artifact_at(write_temp("s1.txt", b"segment A"), ArtifactKey::BlobSegment(1), 0x55),
            artifact_at(write_temp("mem1.txt", b"memdb 1"), ArtifactKey::Memdb, 0x56),
        ];
        let m1 = commit_run(&e, "1", None, &arts1).await;

        // Run 2: same segment A (identical content ⇒ skipped) + new segment B.
        let arts2 = vec![
            artifact_at(write_temp("s1b.txt", b"segment A"), ArtifactKey::BlobSegment(1), 0x55),
            artifact_at(write_temp("s2.txt", b"segment B"), ArtifactKey::BlobSegment(2), 0x57),
            artifact_at(write_temp("mem2.txt", b"memdb 2"), ArtifactKey::Memdb, 0x58),
        ];
        let m2 = commit_run(&e, "2", Some("1"), &arts2).await;
        assert_eq!(m1.previous, None);
        assert_eq!(m2.previous.as_deref(), Some("1"));

        // Orphan manifest + its object, referencing nothing committed.
        let mut mo = Manifest::new("orphan".to_string(), "manual", None);
        let po = write_temp("orphan.txt", b"orphan object");
        mo.add_artifact(&artifact_at(po, ArtifactKey::TantivyEnvelope, 0x59))
            .unwrap();
        let bytes = mo.encode_gz().unwrap();
        let op = StorePath::parse("test/v1/meta/m-orphan.json.gz").unwrap();
        store
            .put(&op, PutPayload::from_bytes(bytes.into()))
            .await
            .unwrap();
        let obj_key = Manifest::object_key(&[0x59; 32]);
        store
            .put(
                &store_path(&obj_key),
                PutPayload::from_bytes(bytes::Bytes::from_static(b"orphan object")),
            )
            .await
            .unwrap();

        // Policy keeping only manifest "2" — every other budget zeroed so the
        // daily/weekly/monthly selection cannot retain manifest "1".
        let policy = RetentionPolicy {
            keep_last: 1,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
        };
        let stats = e.gc(&policy).await.unwrap();

        // Expired "1" + orphan manifest removed; its unshared memdb object and
        // the orphan object reclaimed. Segment A survives: "2" shares it.
        assert_eq!(stats.manifests_removed, 2, "manifest 1 (expired) + orphan");
        assert_eq!(stats.objects_removed, 2, "manifest 1's memdb + orphan object");
        assert_eq!(stats.bytes_reclaimed, 7 + 13, "memdb 1 (7B) + orphan (13B)");

        // After GC: "2" is intact, its memdb + both segments exist.
        let latest = e.load_latest().await.unwrap().unwrap();
        assert_eq!(latest.id, "2");
        assert_eq!(latest.objects.blob_segments.len(), 2);
        for key in latest.object_keys() {
            let p = store_path(key);
            assert!(store.head(&p).await.is_ok(), "retained object must survive: {key}");
        }
        // "1" is gone (expired), its memdb object reclaimed; segment A object
        // survives because "2" shares it.
        assert!(e.load_by_id("1").await.unwrap().is_none());
        assert!(e.load_by_id("orphan").await.unwrap().is_none());
        let gone_key = Manifest::object_key(&[0x56; 32]);
        assert!(matches!(
            store.head(&store_path(&gone_key)).await,
            Err(object_store::Error::NotFound { .. })
        ));
        // The shared segment A object survived.
        let shared_key = Manifest::object_key(&[0x55; 32]);
        assert!(store.head(&store_path(&shared_key)).await.is_ok());
    }

    #[tokio::test]
    async fn gc_on_empty_store_is_a_noop() {
        let store = BackupEngine::memory_store();
        let e = engine(store);
        let stats = e.gc(&RetentionPolicy::default()).await.unwrap();
        assert_eq!(stats.manifests_removed, 0);
        assert_eq!(stats.objects_removed, 0);
    }

    /// Manual delete of the newest point: unshared objects are reclaimed,
    /// objects shared with the surviving point stay, and LATEST is repointed
    /// to the newest remaining manifest so later runs keep chaining.
    #[tokio::test]
    async fn delete_reclaims_unshared_objects_keeps_shared_and_repoints_latest() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());

        // Run 1: memdb 1 + segment A. Run 2: same segment A + memdb 2.
        let arts1 = vec![
            artifact_at(write_temp("s1.txt", b"segment A"), ArtifactKey::BlobSegment(1), 0x55),
            artifact_at(write_temp("mem1.txt", b"memdb 1"), ArtifactKey::Memdb, 0x56),
        ];
        commit_run(&e, "1", None, &arts1).await;
        let arts2 = vec![
            artifact_at(write_temp("s1b.txt", b"segment A"), ArtifactKey::BlobSegment(1), 0x55),
            artifact_at(write_temp("mem2.txt", b"memdb 2"), ArtifactKey::Memdb, 0x58),
        ];
        commit_run(&e, "2", Some("1"), &arts2).await;

        let stats = e.delete_restore_point("2").await.unwrap();

        assert_eq!(stats.manifests_removed, 1);
        assert_eq!(stats.objects_removed, 1, "only manifest 2's unshared memdb");
        assert_eq!(stats.bytes_reclaimed, 7, "memdb 2 (7B)");

        // Manifest 2 is gone, LATEST fell back to 1.
        assert!(e.load_by_id("2").await.unwrap().is_none());
        let latest = e.load_latest().await.unwrap().unwrap();
        assert_eq!(latest.id, "1");

        // Manifest 1's objects (including the shared segment) all survive.
        let m1 = e.load_by_id("1").await.unwrap().unwrap();
        for key in m1.object_keys() {
            assert!(
                store.head(&store_path(key)).await.is_ok(),
                "surviving object must stay: {key}"
            );
        }
        // The shared segment object survived.
        let shared_key = Manifest::object_key(&[0x55; 32]);
        assert!(store.head(&store_path(&shared_key)).await.is_ok());
    }

    /// Deleting the last restore point empties the store: LATEST is removed
    /// (no dangling pointer) and every object is reclaimed.
    #[tokio::test]
    async fn delete_last_restore_point_removes_latest_and_all_objects() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());

        let arts = vec![
            artifact_at(write_temp("s1.txt", b"segment A"), ArtifactKey::BlobSegment(1), 0x55),
            artifact_at(write_temp("mem1.txt", b"memdb 1"), ArtifactKey::Memdb, 0x56),
        ];
        commit_run(&e, "1", None, &arts).await;

        let stats = e.delete_restore_point("1").await.unwrap();
        assert_eq!(stats.manifests_removed, 1);
        assert_eq!(stats.objects_removed, 2, "memdb + segment, nothing shares them");

        assert!(e.load_latest().await.unwrap().is_none());
        assert!(e.list_restore_points().await.unwrap().is_empty());
        let gone_key = Manifest::object_key(&[0x55; 32]);
        assert!(matches!(
            store.head(&store_path(&gone_key)).await,
            Err(object_store::Error::NotFound { .. })
        ));
    }

    /// Deleting an unknown id is a clean ResourceNotFound, not a store error.
    #[tokio::test]
    async fn delete_unknown_restore_point_is_resource_not_found() {
        let e = engine(BackupEngine::memory_store());
        let err = e.delete_restore_point("nope").await.unwrap_err();
        assert!(format!("{err:?}").contains("not found"));
    }

    /// Retention keeps a5 (keep_last), a4 (newest of Mar 2) and a1 (newest of
    /// Mar 1) but deletes a2/a3 — manifests sitting *between* two kept ones in
    /// the `previous` chain. The next GC walks LATEST→a5→a4 and stops at the
    /// deleted a3: selection must still cover a1 (a listed manifest on disk),
    /// or the Mar 1 restore point is destroyed although the policy keeps it.
    #[tokio::test]
    async fn gc_keeps_restore_points_stranded_behind_a_broken_chain() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        let policy = RetentionPolicy {
            keep_last: 1,
            keep_daily: 2,
            keep_weekly: 0,
            keep_monthly: 0,
        };

        // Five daily runs; a2/a3 share Mar 2 with a4 (a4 newest).
        let specs = [
            ("a1", "2026-03-01T02:00:00Z", 0x81),
            ("a2", "2026-03-02T02:00:00Z", 0x82),
            ("a3", "2026-03-02T09:00:00Z", 0x83),
            ("a4", "2026-03-02T14:00:00Z", 0x84),
            ("a5", "2026-03-03T09:00:00Z", 0x85),
        ];
        let mut prev: Option<String> = None;
        let mut manifests = Vec::new();
        for (id, created, byte) in specs.into_iter() {
            let mut m = Manifest::new(id.to_string(), "manual", prev.clone());
            // Override the auto-generated timestamp so the retention buckets
            // in this scenario are deterministic.
            m.created = created.to_string();
            let mut s = EngineSummary::default();
            let art = artifact_at(
                write_temp(&format!("{id}.txt"), vec![byte; 4].as_slice()),
                ArtifactKey::Memdb,
                byte,
            );
            e.upload_artifact(&art, &mut s).await.unwrap();
            m.add_artifact(&art).unwrap();
            e.commit(&m).await.unwrap();
            prev = Some(id.to_string());
            manifests.push(m);
        }

        // GC 1: the chain is still fully walkable, so expired a2/a3 are
        // removed while a1 (Mar 1 bucket) survives.
        let stats = e.gc(&policy).await.unwrap();
        assert_eq!(stats.manifests_removed, 2, "a2 + a3 expired");
        assert!(e.load_by_id("a1").await.unwrap().is_some());

        // GC 2: the chain now breaks at a3. a1 must still be kept.
        let stats = e.gc(&policy).await.unwrap();
        assert_eq!(stats.manifests_removed, 0, "a1 is retained by policy");
        assert!(
            e.load_by_id("a1").await.unwrap().is_some(),
            "a restore point kept by retention must survive a broken chain"
        );

        // Its object survives too.
        let key = manifests[0].object_keys()[0].to_string();
        assert!(store.head(&store_path(&key)).await.is_ok(), "{key}");
    }

    /// A retention policy that keeps nothing must not wipe the store: GC
    /// clamps it so the newest restore point always survives.
    #[tokio::test]
    async fn gc_clamps_a_keep_nothing_policy() {
        let store = BackupEngine::memory_store();
        let e = engine(store);
        commit_run(
            &e,
            "1",
            None,
            &[artifact_at(write_temp("c1.txt", b"keep me"), ArtifactKey::Memdb, 0x87)],
        )
        .await;
        let policy = RetentionPolicy {
            keep_last: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
        };
        let stats = e.gc(&policy).await.unwrap();
        assert_eq!(stats.manifests_removed, 0, "the newest manifest survives");
        assert!(e.load_latest().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn verify_reports_missing_and_wrong_size() {
        let store = BackupEngine::memory_store();
        let e = engine(store.clone());
        let mut m = Manifest::new("m-1".to_string(), "manual", None);
        let p1 = write_temp("v1.txt", b"ok object");
        m.add_artifact(&artifact_at(p1.clone(), ArtifactKey::Memdb, 0x61))
            .unwrap();
        // A reference to an object we never upload.
        let p2 = write_temp("v2.txt", b"missing object");
        m.add_artifact(&artifact_at(p2, ArtifactKey::TantivyEnvelope, 0x62))
            .unwrap();
        let mut s = EngineSummary::default();
        e.upload_artifact(&artifact_at(p1.clone(), ArtifactKey::Memdb, 0x61), &mut s)
            .await
            .unwrap();

        let problems = e.verify_manifest(&m).await.unwrap();
        assert_eq!(problems.len(), 1, "only the un-uploaded object is missing");
        assert!(problems[0].contains("missing"), "{problems:?}");
    }

    #[tokio::test]
    async fn chain_follows_previous_links() {
        let store = BackupEngine::memory_store();
        let e = engine(store);
        let mut m1 = Manifest::new("1".to_string(), "manual", None);
        let p1 = write_temp("chain1.txt", b"one");
        m1.add_artifact(&artifact_at(p1, ArtifactKey::Memdb, 0x71))
            .unwrap();
        let mut m2 = Manifest::new("2".to_string(), "schedule", Some("1".to_string()));
        let p2 = write_temp("chain2.txt", b"two");
        m2.add_artifact(&artifact_at(p2, ArtifactKey::Memdb, 0x72))
            .unwrap();
        e.commit(&m1).await.unwrap();
        e.commit(&m2).await.unwrap();

        let chain = e.load_chain().await.unwrap();
        let ids: Vec<String> = chain.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec!["2".to_string(), "1".to_string()]);
    }

    // ── local target ──────────────────────────────────────────────────────

    /// A unique directory path that does **not** exist.
    fn missing_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "bichon-local-{}-{seq}-{tag}",
            std::process::id()
        ))
    }

    #[test]
    fn local_store_refuses_a_directory_that_does_not_exist() {
        let dir = missing_dir("absent");
        let err = BackupEngine::local_store(&dir).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("does not exist") || msg.contains("mount"),
            "the message must tell the operator what to check: {msg}"
        );
        assert!(
            !dir.exists(),
            "the store must never create the directory — that is what turns a \
             dropped mount into backups written onto the root filesystem"
        );
    }

    /// `new_with_prefix` only canonicalizes, so it happily accepts a path that
    /// is a *file*. Pinned here because it is the reason `config::validate`
    /// must keep its own `is_dir()` check — removing that check on the
    /// assumption that the store rejects non-directories would let the first
    /// put fail with an opaque `UnableToCreateFile` instead.
    #[test]
    fn local_store_alone_does_not_reject_a_file_path() {
        let file = write_temp("not-a-dir.txt", b"x");
        assert!(
            BackupEngine::local_store(&file).is_ok(),
            "if this ever starts failing, config::validate's is_dir() check may \
             be redundant — but do not remove it without checking"
        );
    }

    #[tokio::test]
    async fn local_store_round_trips_an_object() {
        let dir = missing_dir("roundtrip");
        let store = BackupEngine::local_store_for_test(&dir).unwrap();
        let e = engine(store);
        let p = write_temp("local-roundtrip.txt", b"local payload");
        let art = artifact_at(p, ArtifactKey::Memdb, 0x31);
        let mut sum = EngineSummary::default();
        e.upload_artifact(&art, &mut sum).await.unwrap();

        // Reopen from scratch: the bytes must be there, and no `#<n>` staging
        // file may be left beside them.
        let reopened = BackupEngine::local_store(&dir).unwrap();
        let got = reopened
            .get(&store_path(&Manifest::object_key(&art.sha256)))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(
            &got[..],
            b"local payload",
            "the object must survive a reopen"
        );

        let mut staging = 0;
        for entry in walk_files(&dir) {
            if is_staging_name(&entry) {
                staging += 1;
            }
        }
        assert_eq!(staging, 0, "a completed put must leave no staging file");
    }

    #[test]
    fn sweep_staging_removes_only_old_staging_files() {
        let dir = missing_dir("sweep");
        let base = dir.join("test").join(LAYOUT_DIR);
        let obj = base.join("obj").join("sha256");
        std::fs::create_dir_all(&obj).unwrap();

        // Three shapes: a stale staging file (the target), a fresh staging
        // file (a concurrent run's, must survive), and a real object.
        let stale = obj.join("aa#1");
        let fresh = obj.join("bb#2");
        let real = obj.join("cc");
        std::fs::write(&stale, b"stale").unwrap();
        std::fs::write(&fresh, b"fresh").unwrap();
        std::fs::write(&real, b"real").unwrap();

        // Age the stale one past the threshold.
        let old = std::time::SystemTime::now() - (STAGING_MAX_AGE + std::time::Duration::from_secs(60));
        set_mtime(&stale, old);

        let target = BackupTarget::Local(dir.clone());
        assert_eq!(target.sweep_staging("test"), 1);
        assert!(!stale.exists(), "the stale staging file must be removed");
        assert!(fresh.exists(), "a fresh staging file may belong to a live put");
        assert!(real.exists(), "real objects are never touched");

        // An S3 target has no directory tree and must be a no-op.
        let s3 = BackupTarget::S3(S3Options::default());
        assert_eq!(s3.sweep_staging("test"), 0);
    }

    #[test]
    fn is_staging_name_matches_only_the_staging_suffix() {
        assert!(is_staging_name(Path::new("/x/ab#1")));
        assert!(is_staging_name(Path::new("/x/ab#12")));
        assert!(!is_staging_name(Path::new("/x/ab")));
        assert!(!is_staging_name(Path::new("/x/ab#")));
        assert!(!is_staging_name(Path::new("/x/ab#c")));
        // A `#` that is not a suffix must not match either.
        assert!(!is_staging_name(Path::new("/x/a#1/b")));
    }

    #[test]
    fn backup_target_kind_and_display() {
        let local = BackupTarget::Local(PathBuf::from("/mnt/backup"));
        assert_eq!(local.kind(), "local");
        assert_eq!(local.display(), "/mnt/backup");

        let s3 = BackupTarget::S3(S3Options {
            endpoint: "http://minio:9000".to_string(),
            bucket: "bichon".to_string(),
            ..Default::default()
        });
        assert_eq!(s3.kind(), "s3");
        assert_eq!(s3.display(), "s3://http://minio:9000/bichon");
    }

    /// Every file under `dir`, recursively (test helper).
    fn walk_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_files(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    /// Backdate a file's mtime. `filetime` is not a dependency, so this uses
    /// the platform API directly.
    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(when).unwrap();
    }
}
