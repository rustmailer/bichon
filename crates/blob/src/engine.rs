use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fs2::FileExt;

use crate::bucket::{IndexRecord, IndexStore};
use crate::compress;
use crate::error::{Error, Result};
use crate::file_pool::FilePool;
use crate::gc::{self, GcStats};
use crate::meta::{GlobalMeta, SegmentStats};
use crate::segment::{self, SegmentReader, SegmentWriter};
use crate::types::{Codec, Config, ENTRY_HEADER_SIZE};

/// Global content-addressable blob store.
///
/// All data is keyed by a 32-byte content hash.  Identical content is stored
/// only once.  The caller is responsible for tracking which entities reference
/// which content hashes — this crate is a pure content-addressable KV engine.
pub struct Engine {
    shared: Arc<EngineShared>,
    flush_handle: Mutex<Option<FlushHandle>>,
    gc_handle: Mutex<Option<GcHandle>>,
    disk_handle: Mutex<Option<DiskHandle>>,
}

struct EngineShared {
    config: Config,
    inner: RwLock<EngineInner>,
    index_store: IndexStore,
    write_mutex: Mutex<()>,
    file_pool: FilePool,
    /// When set, the background GC thread skips its pass. Used to keep the
    /// store byte-stable while an external backup tool copies the segments.
    gc_paused: AtomicBool,
    /// Invoked once per low-disk episode. The engine never terminates the
    /// process itself; the handler decides how to shut down.
    low_disk_callback: RwLock<Option<Arc<dyn Fn() + Send + Sync>>>,
    low_disk: AtomicBool,
    low_disk_notified: AtomicBool,
    #[allow(dead_code)]
    lock_file: File,
}

struct FlushHandle {
    handle: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

struct GcHandle {
    handle: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

struct DiskHandle {
    handle: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

struct EngineInner {
    root: PathBuf,
    meta: GlobalMeta,
    active_writer: SegmentWriter,
    /// Free-space floor copied from `Config::min_free_bytes`.
    min_free_bytes: u64,
    /// While true, appends may leave the volume below `min_free_bytes`.
    /// Enabled while draining during a graceful shutdown.
    allow_below_floor: bool,
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub total_keys: u64,
    pub total_bytes: u64,
    pub deleted_bytes: u64,
    pub segment_count: usize,
}

/// One sealed, immutable segment as a backup artifact.
#[derive(Debug, Clone)]
pub struct SegmentArtifact {
    pub segment_id: u32,
    /// Path of the sealed `.seg` file (immutable until GC reclaims it).
    pub path: PathBuf,
    /// SHA-256 of the file — the S3 object key derives from it.
    pub sha256: [u8; 32],
    /// Size of the file in bytes.
    pub bytes: u64,
}

impl Engine {
    pub fn open(path: &Path, config: Config) -> Result<Self> {
        config.validate()?;

        fs::create_dir_all(path)?;
        fs::create_dir_all(path.join("segments"))?;

        // Acquire an exclusive file lock so that no two processes can open
        // the same database directory concurrently.
        let lock_path = path.join("LOCK");
        let lock_file = File::create(&lock_path)?;
        lock_file.try_lock_exclusive().map_err(|_| Error::AlreadyOpen {
            path: path.display().to_string(),
        })?;

        let (index_store, index_rebuilt) = match IndexStore::open(path) {
            Ok(s) => (s, false),
            Err(e) => {
                tracing::warn!("Index database unreadable, rebuilding from segments: {}", e);
                let index_path = path.join("index.redb");
                let _ = std::fs::remove_file(&index_path);
                (IndexStore::open(path)?, true)
            }
        };

        let mut meta = GlobalMeta::load(path)?;

        // Discover existing segments on disk
        let seg_dir = path.join("segments");
        let mut disk_segments: Vec<u32> = Vec::new();
        if seg_dir.exists() {
            for entry in fs::read_dir(&seg_dir)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".seg") && !name_str.contains("temp_") {
                    if let Some(id_str) = name_str.strip_suffix(".seg") {
                        if let Ok(id) = id_str.parse::<u32>() {
                            disk_segments.push(id);
                        }
                    }
                }
            }
        }
        disk_segments.sort_unstable();

        for &seg_id in &disk_segments {
            if !meta.segments.contains_key(&seg_id) {
                meta.segments.insert(seg_id, SegmentStats::new(seg_id));
            }
        }

        let max_disk_id = disk_segments.last().copied().unwrap_or(0);
        if max_disk_id > meta.active_segment_id {
            meta.active_segment_id = max_disk_id;
        }
        if meta.active_segment_id == 0 {
            meta.active_segment_id = 1;
        }

        crate::recovery::cleanup_temp_files(path)?;
        let recovered_records = if index_rebuilt {
            crate::recovery::rebuild_index(path, &mut meta)?
        } else {
            crate::recovery::recover(path, &mut meta)?
        };
        index_store.insert_batch(&recovered_records)?;

        let seg_path = path
            .join("segments")
            .join(segment::segment_filename(meta.active_segment_id));
        let active_writer = if seg_path.exists() {
            SegmentWriter::open_append(seg_path, meta.active_segment_id)?
        } else {
            SegmentWriter::create(seg_path, meta.active_segment_id)?
        };

        meta.save(path)?;

        let shared = Arc::new(EngineShared {
            config: config.clone(),
            inner: RwLock::new(EngineInner {
                root: path.to_path_buf(),
                meta,
                active_writer,
                min_free_bytes: config.min_free_bytes,
                allow_below_floor: false,
            }),
            index_store,
            write_mutex: Mutex::new(()),
            file_pool: FilePool::new(8),
            gc_paused: AtomicBool::new(false),
            low_disk_callback: RwLock::new(None),
            low_disk: AtomicBool::new(false),
            low_disk_notified: AtomicBool::new(false),
            lock_file,
        });

        let flush_handle = if config.flush_interval_secs > 0 {
            let shared2 = Arc::clone(&shared);
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);
            let interval = Duration::from_secs(config.flush_interval_secs);

            let handle = thread::Builder::new()
                .name("blob-flush".into())
                .spawn(move || {
                    while !stop2.load(Ordering::Acquire) {
                        thread::park_timeout(interval);
                        if stop2.load(Ordering::Acquire) {
                            break;
                        }
                        let _lock = shared2.write_mutex.lock().unwrap();
                        let mut inner = shared2.inner.write().unwrap();
                        if let Err(e) = inner.flush_active() {
                            tracing::error!("background flush failed: {}", e);
                        }
                    }
                })
                .expect("failed to spawn blob-flush thread");

            Some(FlushHandle { handle, stop })
        } else {
            None
        };

        let gc_handle = if config.gc_interval_secs > 0 {
            let shared3 = Arc::clone(&shared);
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);
            let interval = Duration::from_secs(config.gc_interval_secs);

            let handle = thread::Builder::new()
                .name("blob-gc".into())
                .spawn(move || {
                    while !stop2.load(Ordering::Acquire) {
                        thread::park_timeout(interval);
                        if stop2.load(Ordering::Acquire) {
                            break;
                        }
                        // Skip the pass entirely while a backup window is open:
                        // GC seals and renames segment files, which would race
                        // an external copy of the segments directory.
                        if shared3.gc_paused.load(Ordering::Acquire) {
                            continue;
                        }
                        // Seal the active segment first so that the data from
                        // this GC cycle becomes eligible for compaction.
                        // Without this, the active segment never seals until
                        // it reaches SEGMENT_MAX_SIZE (1 GB), and GC would
                        // have no candidates on small databases.
                        if let Err(e) = shared3.ensure_sealed() {
                            tracing::error!("background GC: seal failed: {}", e);
                        }
                        match shared3.gc_if_needed() {
                            Ok(Some(stats)) => {
                                tracing::info!(
                                    "GC compacted segment {}: {} → {} bytes (kept {}, skipped {})",
                                    stats.segment_id,
                                    stats.bytes_before,
                                    stats.bytes_after,
                                    stats.entries_kept,
                                    stats.entries_skipped,
                                );
                            }
                            Ok(None) => {
                                tracing::debug!("GC check: no segment exceeds deleted-ratio threshold");
                            }
                            Err(e) => {
                                tracing::error!("background GC failed: {}", e);
                            }
                        }
                    }
                })
                .expect("failed to spawn blob-gc thread");

            Some(GcHandle { handle, stop })
        } else {
            None
        };

        let disk_handle = if config.disk_check_interval_secs > 0 && config.min_free_bytes > 0 {
            let root = path.to_path_buf();
            let min_free = config.min_free_bytes;
            let shared4 = Arc::clone(&shared);
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);
            let interval = Duration::from_secs(config.disk_check_interval_secs);

            let handle = thread::Builder::new()
                .name("blob-disk-watch".into())
                .spawn(move || loop {
                    if stop2.load(Ordering::Acquire) {
                        break;
                    }
                    match fs2::available_space(&root) {
                        Ok(free) if free < min_free => {
                            if !shared4.low_disk.swap(true, Ordering::AcqRel) {
                                tracing::error!(
                                    path = %root.display(),
                                    free_bytes = free,
                                    min_free_bytes = min_free,
                                    "blob store volume is below the free-space floor; refusing new appends"
                                );
                            }
                            // Fires the registered handler once per episode.
                            // Without a handler the engine keeps refusing new
                            // appends but does not terminate the process.
                            shared4.notify_low_disk();
                        }
                        Ok(_) => {
                            if shared4.low_disk.swap(false, Ordering::AcqRel) {
                                shared4.low_disk_notified.store(false, Ordering::Release);
                                tracing::info!(
                                    "blob store volume is back above the free-space floor"
                                );
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "disk-space check failed"),
                    }
                    thread::park_timeout(interval);
                })
                .expect("failed to spawn blob-disk-watch thread");

            Some(DiskHandle { handle, stop })
        } else {
            None
        };

        Ok(Self {
            shared,
            flush_handle: Mutex::new(flush_handle),
            gc_handle: Mutex::new(gc_handle),
            disk_handle: Mutex::new(disk_handle),
        })
    }

    // ── Read / Write / Delete ───────────────────────────────────────────

    pub fn put(&self, key: [u8; 32], value: &[u8], codec: Codec) -> Result<()> {
        if value.len() > crate::types::MAX_VALUE_SIZE {
            return Err(Error::ValueTooLarge { size: value.len() });
        }

        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();

        let (data, actual_codec) = compress::compress(
            value,
            codec,
            self.shared.config.compress_threshold,
            self.shared.config.compression_level,
        );

        let original_len = value.len() as u32;
        let (segment_id, offset, data_size) =
            inner.append_entry(key, &data, original_len, 0, actual_codec)?;

        let record = IndexRecord::new(key, segment_id, offset, data_size, 0);
        self.shared.index_store.insert(&record)?;

        let entry_end = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;
        inner.mark_indexed(segment_id, entry_end)?;

        Ok(())
    }

    pub fn get(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        let record = match self.shared.index_store.get(key)? {
            Some(r) => r,
            None => return Ok(None),
        };

        let inner = self.shared.inner.read().unwrap();
        let seg_path = inner.segment_path(record.segment_id)?;

        if !seg_path.exists() {
            return Err(Error::SegmentNotFound(record.segment_id));
        }

        let reader = SegmentReader::open(seg_path.clone(), record.segment_id)?;
        let file = self.shared.file_pool.get(record.segment_id, &seg_path)?;
        let (entry, _) = reader.read_entry_at_file(record.offset, &file)?;

        let value = compress::decompress(&entry.data, entry.codec, entry.raw_size as usize)?;
        Ok(Some(value))
    }

    pub fn delete(&self, key: &[u8; 32]) -> Result<()> {
        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();

        // Look up the existing record so we can account deleted_bytes on the
        // segment that holds the original data — this is what drives GC.
        if let Some(rec) = self.shared.index_store.get(key)? {
            if !rec.is_tombstone() {
                if let Some(stats) = inner.meta.segments.get_mut(&rec.segment_id) {
                    stats.deleted_bytes += rec.data_size as u64;
                    stats.recompute_ratio();
                }
            }
        }

        let (segment_id, offset, data_size) =
            inner.append_entry(*key, &[], 0, 1, Codec::None)?;

        let record = IndexRecord::new(*key, segment_id, offset, data_size, 1);
        self.shared.index_store.insert(&record)?;

        let entry_end = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;
        inner.mark_indexed(segment_id, entry_end)?;

        Ok(())
    }

    pub fn exists(&self, key: &[u8; 32]) -> Result<bool> {
        self.shared.index_store.exists(key)
    }

    // ── Batch delete ─────────────────────────────────────────────────────

    pub fn delete_batch(&self, keys: &[[u8; 32]]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }

        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();

        let mut records: Vec<IndexRecord> = Vec::with_capacity(keys.len());
        let mut ends: Vec<(u32, u64)> = Vec::with_capacity(keys.len());

        for key in keys {
            // Track deleted_bytes for GC threshold on the original segment.
            if let Some(rec) = self.shared.index_store.get(key)? {
                if !rec.is_tombstone() {
                    if let Some(stats) = inner.meta.segments.get_mut(&rec.segment_id) {
                        stats.deleted_bytes += rec.data_size as u64;
                        stats.recompute_ratio();
                    }
                }
            }

            let (segment_id, offset, data_size) =
                inner.append_entry(*key, &[], 0, 1, Codec::None)?;

            let entry_end = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;
            records.push(IndexRecord::new(*key, segment_id, offset, data_size, 1));
            ends.push((segment_id, entry_end));
        }

        inner.flush_active()?;

        self.shared.index_store.insert_batch(&records)?;

        for (segment_id, entry_end) in &ends {
            inner.mark_indexed(*segment_id, *entry_end)?;
        }

        Ok(())
    }

    // ── Batch write ─────────────────────────────────────────────────────

    pub fn put_batch(&self, entries: &[([u8; 32], Vec<u8>, Codec)]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();

        let mut records: Vec<IndexRecord> = Vec::with_capacity(entries.len());
        let mut ends: Vec<(u32, u64)> = Vec::with_capacity(entries.len());

        for (key, value, codec) in entries {
            if value.len() > crate::types::MAX_VALUE_SIZE {
                return Err(Error::ValueTooLarge { size: value.len() });
            }
            let (data, actual_codec) = compress::compress(
                value,
                *codec,
                self.shared.config.compress_threshold,
                self.shared.config.compression_level,
            );

            let original_len = value.len() as u32;
            let (segment_id, offset, data_size) =
                inner.append_entry(*key, &data, original_len, 0, actual_codec)?;

            let entry_end = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;
            records.push(IndexRecord::new(*key, segment_id, offset, data_size, 0));
            ends.push((segment_id, entry_end));
        }

        inner.flush_active()?;

        self.shared.index_store.insert_batch(&records)?;

        // Deduplicate: keep only the max offset per segment.
        ends.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        ends.dedup_by(|a, b| a.0 == b.0);
        for (segment_id, entry_end) in &ends {
            inner.mark_indexed(*segment_id, *entry_end)?;
        }

        Ok(())
    }

    // ── GC ──────────────────────────────────────────────────────────────

    pub fn gc(&self) -> Result<Option<GcStats>> {
        self.shared.gc()
    }

    /// Run GC only if some segment exceeds the configured deleted-ratio threshold.
    /// Returns `Ok(None)` immediately without acquiring the write lock when no
    /// segment qualifies.
    pub fn gc_if_needed(&self) -> Result<Option<GcStats>> {
        self.shared.gc_if_needed()
    }

    // ── Flush / Stats / Shutdown ────────────────────────────────────────

    /// Fsync the active segment and save metadata without compacting buckets.
    /// Lightweight checkpoint suitable for periodic calls from the background
    /// flush thread or external schedulers.
    pub fn flush(&self) -> Result<()> {
        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();
        inner.flush_active()
    }

    /// Pause or resume the background GC thread. While paused, no segment is
    /// sealed, renamed or deleted, keeping the on-disk store byte-stable for
    /// an external backup copy. Must be re-enabled after the backup window
    /// closes.
    pub fn set_gc_paused(&self, paused: bool) {
        self.shared.gc_paused.store(paused, Ordering::Release);
    }

    /// Whether the background GC is currently paused for a backup window.
    pub fn is_gc_paused(&self) -> bool {
        self.shared.gc_paused.load(Ordering::Acquire)
    }

    /// Register the handler invoked when the store volume falls below
    /// [`Config::min_free_bytes`]. It fires at most once per low-disk episode
    /// and is expected to start a graceful shutdown; the engine itself never
    /// terminates the process.
    pub fn set_low_disk_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        *self.shared.low_disk_callback.write().unwrap() = Some(Arc::new(callback));
        // The watchdog may have fired before this handler was registered
        // (for example, a volume that is already below the floor at open).
        if self.shared.low_disk.load(Ordering::Acquire) {
            self.shared.notify_low_disk();
        }
    }

    /// While `true`, appends may leave the volume below the free-space floor.
    /// Enable this when draining during a graceful shutdown: the data is
    /// already queued, so refusing it would only discard it.
    pub fn allow_writes_below_free_floor(&self, allow: bool) {
        self.shared.inner.write().unwrap().allow_below_floor = allow;
    }

    /// Bring the store into a copy-safe, fully durable state.
    ///
    /// Runs inside a backup write window (no writers active):
    /// 1. Pause GC so no file is sealed/renamed/deleted mid-copy.
    /// 2. Seal the active segment so its file becomes immutable.
    /// 3. Flush the (now empty) active writer + metadata.
    /// 4. Fsync every segment file — a freshly sealed segment may still have
    ///    unflushed page-cache data that `flush_active` never synced.
    /// 5. Flush the redb index file.
    pub fn prepare_for_backup(&self) -> Result<()> {
        self.set_gc_paused(true);

        self.seal_active_segment()?;

        let seg_dir = {
            let _write_lock = self.shared.write_mutex.lock().unwrap();
            let mut inner = self.shared.inner.write().unwrap();
            inner.flush_active()?;
            inner.root.join("segments")
        };

        for entry in fs::read_dir(&seg_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".seg") {
                if let Ok(f) = File::options().write(true).open(entry.path()) {
                    if let Err(e) = f.sync_all() {
                        tracing::warn!(
                            "backup: failed to fsync segment {}: {}",
                            entry.path().display(),
                            e
                        );
                    }
                }
            }
        }

        self.shared.index_store.flush()
    }

    /// List every sealed segment as a backup artifact. Call after
    /// [`Engine::prepare_for_backup`] (GC paused, active segment sealed), so
    /// the listed files are immutable and nothing mutates the store.
    ///
    /// Hashes come from the meta (computed at seal / compaction); segments
    /// sealed before hashing existed are hashed here once and persisted, so
    /// the backup capture window only ever pays for genuinely first-time
    /// hashes.
    pub fn backup_segments(&self) -> Result<Vec<SegmentArtifact>> {
        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();
        let mut out = Vec::new();
        let mut dirty = false;
        let seg_dir = inner.root.join("segments");
        for (&seg_id, stats) in inner.meta.segments.iter_mut() {
            if !stats.sealed {
                continue; // the (empty) active segment is not part of a backup
            }
            let path = seg_dir.join(segment::segment_filename(seg_id));
            if !path.exists() {
                return Err(Error::SegmentNotFound(seg_id));
            }
            let bytes = fs::metadata(&path)?.len();
            let sha256 = match stats.sha256 {
                Some(h) => h,
                None => {
                    // One-time backfill for segments sealed before hashing.
                    let h = crate::checksum::sha256_file(&path)?;
                    stats.sha256 = Some(h);
                    dirty = true;
                    h
                }
            };
            out.push(SegmentArtifact {
                segment_id: seg_id,
                path,
                sha256,
                bytes,
            });
        }
        if dirty {
            inner.meta.save(&inner.root)?;
        }
        Ok(out)
    }

    pub fn stats(&self) -> Result<Stats> {
        let inner = self.shared.inner.read().unwrap();
        let meta = &inner.meta;

        let mut total_bytes = 0u64;
        let mut deleted_bytes = 0u64;
        for seg in meta.segments.values() {
            total_bytes += seg.total_bytes;
            deleted_bytes += seg.deleted_bytes;
        }

        let total_keys = self.shared.index_store.total_keys()? as u64;

        Ok(Stats {
            total_keys,
            total_bytes,
            deleted_bytes,
            segment_count: meta.segments.len(),
        })
    }

    /// Seal the active segment, forcing it to become a GC candidate.
    #[doc(hidden)]
    pub fn seal_active_segment(&self) -> Result<u32> {
        let _write_lock = self.shared.write_mutex.lock().unwrap();
        let mut inner = self.shared.inner.write().unwrap();
        let id = inner.active_writer.id();
        inner.seal_active()?;
        Ok(id)
    }

    pub fn shutdown(&self) -> Result<()> {
        // Stop background threads first
        if let Some(fh) = self.flush_handle.lock().unwrap().take() {
            fh.stop.store(true, Ordering::Release);
            fh.handle.thread().unpark();
            let _ = fh.handle.join();
        }
        if let Some(gh) = self.gc_handle.lock().unwrap().take() {
            gh.stop.store(true, Ordering::Release);
            gh.handle.thread().unpark();
            let _ = gh.handle.join();
        }
        if let Some(dh) = self.disk_handle.lock().unwrap().take() {
            dh.stop.store(true, Ordering::Release);
            dh.handle.thread().unpark();
            let _ = dh.handle.join();
        }

        let mut inner = self.shared.inner.write().unwrap();
        inner.flush_active()?;

        inner.meta.save(&inner.root)?;
        tracing::info!("bichon-blob shut down cleanly");
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Best-effort shutdown that is panic-safe: only signal threads to
        // stop — don't try to acquire write_mutex or inner.write(), which
        // would deadlock if we're unwinding from a panic that happened while
        // one of those locks was held.
        if let Some(fh) = self.flush_handle.lock().ok().and_then(|mut g| g.take()) {
            fh.stop.store(true, Ordering::Release);
            fh.handle.thread().unpark();
            let _ = fh.handle.join();
        }
        if let Some(gh) = self.gc_handle.lock().ok().and_then(|mut g| g.take()) {
            gh.stop.store(true, Ordering::Release);
            gh.handle.thread().unpark();
            let _ = gh.handle.join();
        }
        if let Some(dh) = self.disk_handle.lock().ok().and_then(|mut g| g.take()) {
            dh.stop.store(true, Ordering::Release);
            dh.handle.thread().unpark();
            let _ = dh.handle.join();
        }
    }
}

// ── EngineShared ────────────────────────────────────────────────────────────

impl EngineShared {
    /// Invoke the low-disk handler, at most once per low-disk episode.
    fn notify_low_disk(&self) {
        let callback = self.low_disk_callback.read().unwrap().clone();
        let Some(callback) = callback else {
            return;
        };
        if self.low_disk_notified.swap(true, Ordering::AcqRel) {
            return;
        }
        callback();
    }

    /// Seal the active segment if its deleted-ratio exceeds the GC threshold,
    /// so that the upcoming GC pass can compact it.  Called periodically by
    /// the background GC thread.
    fn ensure_sealed(&self) -> Result<()> {
        let mut inner = self.inner.write().unwrap();
        let active_id = inner.active_writer.id();
        if let Some(stats) = inner.meta.segments.get(&active_id) {
            if stats.deleted_ratio >= self.config.gc_deleted_ratio {
                inner.seal_active()?;
            }
        }
        Ok(())
    }

    fn gc_if_needed(&self) -> Result<Option<GcStats>> {
        let inner = self.inner.read().unwrap();
        let needs_gc = inner
            .meta
            .segments
            .values()
            .any(|s| s.sealed && s.deleted_ratio >= self.config.gc_deleted_ratio);
        drop(inner);

        if needs_gc {
            self.gc()
        } else {
            Ok(None)
        }
    }

    fn gc(&self) -> Result<Option<GcStats>> {
        // Phase 1: scan only the target segment, consult bucket index per entry.
        // Read-only with respect to Engine state — no write_mutex needed.
        let prep = {
            let inner = self.inner.read().unwrap();
            gc::gc_prepare(
                &inner.root,
                &inner.meta,
                self.config.gc_deleted_ratio,
                &self.index_store,
            )?
        };

        let mut prep = match prep {
            Some(p) => p,
            None => return Ok(None),
        };

        // Phase 2: rename temp file + update bucket index.
        let _write_lock = self.write_mutex.lock().unwrap();

        let kept_records = std::mem::take(&mut prep.kept_records);
        let deleted_keys = std::mem::take(&mut prep.deleted_keys);
        let stats = gc::gc_finish(prep)?;
        self.file_pool.invalidate(stats.segment_id);

        // Insert new index records with updated offsets for kept entries.
        if !kept_records.is_empty() {
            self.index_store.insert_batch(&kept_records)?;
        }

        // Remove tombstone IndexRecords that pointed to entries in this
        // compacted segment — they are gone now and would accumulate forever.
        if !deleted_keys.is_empty() {
            self.index_store.delete_batch(&deleted_keys)?;
        }

        {
            let mut inner = self.inner.write().unwrap();

            if stats.bytes_after == 0 {
                // Segment was completely emptied — remove it from meta first,
                // then delete the file.  If we crash between the two steps the
                // orphaned file is rediscovered on next open and retried.
                inner.meta.segments.remove(&stats.segment_id);
                inner.meta.save(&inner.root)?;
                drop(inner);

                let seg_path = self.inner.read().unwrap()
                    .root
                    .join("segments")
                    .join(segment::segment_filename(stats.segment_id));
                let _ = fs::remove_file(&seg_path);
            } else {
                // Update segment stats: now smaller and clean. The file was
                // rewritten by compaction, so the stored hash must be replaced
                // with the new content's hash (computed on the temp file).
                if let Some(seg_stats) = inner.meta.segments.get_mut(&stats.segment_id) {
                    seg_stats.total_bytes = stats.bytes_after;
                    seg_stats.deleted_bytes = 0;
                    seg_stats.deleted_ratio = 0.0;
                    seg_stats.indexed_up_to_offset = stats.bytes_after;
                    seg_stats.sha256 = Some(stats.sha256);
                }
                inner.meta.save(&inner.root)?;
            }
        }

        Ok(Some(stats))
    }
}

// ── EngineInner ───────────────────────────────────────────────────────────

impl EngineInner {
    /// Reject an append that would leave less than `min_free_bytes` on the
    /// store volume. Checked before writing so a refusal cannot tear an entry.
    fn ensure_space_for(&self, entry_bytes: u64) -> Result<()> {
        if self.min_free_bytes == 0 || self.allow_below_floor {
            return Ok(());
        }
        let free = fs2::available_space(&self.root)?;
        let required = entry_bytes.saturating_add(self.min_free_bytes);
        if free < required {
            return Err(Error::DiskFull(format!(
                "{}: {} bytes free, need {} ({} floor + {} entry)",
                self.root.display(),
                free,
                required,
                self.min_free_bytes,
                entry_bytes
            )));
        }
        Ok(())
    }

    fn append_entry(
        &mut self,
        key: [u8; 32],
        data: &[u8],
        raw_size: u32,
        flags: u8,
        codec: Codec,
    ) -> Result<(u32, u64, u32)> {
        if self.active_writer.is_full() {
            self.seal_active()?;
        }

        use crate::segment::Entry;
        let entry = if flags == 1 {
            Entry::tombstone(key)
        } else {
            Entry::new(key, data, raw_size, flags, codec)
        };

        // Refuse the write before touching the file when it would breach the
        // free-space floor. A mid-entry ENOSPC is what leaves a torn tail;
        // catching it here makes that impossible for graceful failures.
        self.ensure_space_for(ENTRY_HEADER_SIZE as u64 + entry.data.len() as u64)?;

        let data_size = entry.data.len() as u32;
        let segment_id = self.active_writer.id();
        let offset = self.active_writer.append(&entry)?;

        let stats = self
            .meta
            .segments
            .entry(segment_id)
            .or_insert_with(|| SegmentStats::new(segment_id));
        stats.total_bytes += data_size as u64;
        if flags == 1 {
            stats.deleted_bytes += entry.raw_size as u64;
        }
        stats.recompute_ratio();

        Ok((segment_id, offset, data_size))
    }

    fn flush_active(&mut self) -> Result<()> {
        self.active_writer.fsync()?;
        self.meta.save(&self.root)
    }

    fn mark_indexed(&mut self, segment_id: u32, offset: u64) -> Result<()> {
        if let Some(stats) = self.meta.segments.get_mut(&segment_id) {
            if offset > stats.indexed_up_to_offset {
                stats.indexed_up_to_offset = offset;
            }
        }
        self.meta.save(&self.root)
    }

    fn seal_active(&mut self) -> Result<()> {
        // An empty active segment holds nothing worth freezing — sealing it
        // (e.g. a backup window that opened with no new writes) would create a
        // pointless 0-byte sealed segment on every run, growing the file/meta
        // count without any data. Skip; `backup_segments` skips unsealed
        // segments, so the empty active simply stays out of the snapshot.
        if self.active_writer.bytes_written() == 0 {
            return Ok(());
        }
        let old_id = self.active_writer.id();
        let old_stats = self
            .meta
            .segments
            .entry(old_id)
            .or_insert_with(|| SegmentStats::new(old_id));
        old_stats.sealed = true;
        // The file just received its last entry and is page-cache warm —
        // hashing now is nearly free, and lets a backup skip every segment
        // (design doc §7: blob hash-at-seal).
        let old_path = self
            .root
            .join("segments")
            .join(segment::segment_filename(old_id));
        old_stats.sha256 = Some(crate::checksum::sha256_file(&old_path)?);

        let new_id = old_id + 1;
        self.meta.active_segment_id = new_id;
        let new_path = self
            .root
            .join("segments")
            .join(segment::segment_filename(new_id));
        self.active_writer = SegmentWriter::create(new_path, new_id)?;
        self.meta.save(&self.root)?;

        Ok(())
    }

    fn segment_path(&self, segment_id: u32) -> Result<PathBuf> {
        let path = self
            .root
            .join("segments")
            .join(segment::segment_filename(segment_id));
        if path.exists() {
            Ok(path)
        } else {
            Err(Error::SegmentNotFound(segment_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn no_background_config() -> Config {
        Config {
            gc_interval_secs: 0,
            flush_interval_secs: 0,
            ..Config::default()
        }
    }

    #[test]
    fn put_rejected_below_free_space_floor_without_touching_segment() {
        let dir = TempDir::new().unwrap();
        let mut config = no_background_config();
        // Any real free-space reading is below this, so every append must be
        // refused before it writes a byte.
        config.min_free_bytes = u64::MAX;
        let engine = Engine::open(dir.path(), config).unwrap();

        let err = engine
            .put(test_key(1), b"hello", Codec::None)
            .unwrap_err();
        assert!(matches!(err, Error::DiskFull(_)), "unexpected error: {err:?}");
        assert!(!engine.exists(&test_key(1)).unwrap());

        let seg_path = dir
            .path()
            .join("segments")
            .join(segment::segment_filename(1));
        assert_eq!(
            fs::metadata(&seg_path).unwrap().len(),
            0,
            "a refused append must not write into the segment"
        );
    }

    #[test]
    fn writes_allowed_below_floor_during_shutdown_drain() {
        let dir = TempDir::new().unwrap();
        let mut config = no_background_config();
        config.min_free_bytes = u64::MAX;
        let engine = Engine::open(dir.path(), config).unwrap();

        assert!(matches!(
            engine.put(test_key(1), b"hello", Codec::None).unwrap_err(),
            Error::DiskFull(_)
        ));

        // The graceful-shutdown drain must be able to flush queued blobs.
        engine.allow_writes_below_free_floor(true);
        engine.put(test_key(1), b"hello", Codec::None).unwrap();
        assert_eq!(engine.get(&test_key(1)).unwrap().unwrap(), b"hello");
    }

    /// M1 round-trip: write → prepare_for_backup → reopen → read.
    ///
    /// `prepare_for_backup` must leave the store byte-stable and fully durable
    /// (sealed segment + fsync + flushed index), so a process that exits right
    /// after the backup window closes and is restarted sees every byte.
    #[test]
    fn prepare_for_backup_leaves_store_reopenable() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        let key = test_key(0x42);
        let value = b"state-consistent-backup-payload".to_vec();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            engine.put(key, &value, Codec::Zstd).unwrap();

            // prepare_for_backup pauses GC and seals+fsyncs everything.
            engine.prepare_for_backup().unwrap();
            assert!(engine.is_gc_paused(), "GC must be paused during the window");

            // The store remains writable-independent while paused (no new
            // writers here — the write gate covers that at the core layer).

            // Drop without calling shutdown(): after a backup the process may
            // simply exit. All data must be on disk already.
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(engine.get(&key).unwrap(), Some(value.clone()));

        // GC is re-enabled after the window; a fresh prepare still round-trips.
        engine.set_gc_paused(false);
        assert!(!engine.is_gc_paused());

        let key2 = test_key(0x43);
        engine.put(key2, &value, Codec::Lz4).unwrap();
        engine.prepare_for_backup().unwrap();
        assert_eq!(engine.get(&key2).unwrap(), Some(value.clone()));
        engine.shutdown().unwrap();
    }

    /// An empty active segment must not be sealed by a backup window: repeated
    /// `prepare_for_backup` on an idle store must not accumulate 0-byte
    /// sealed segments (guard in `seal_active`), while a non-empty active is
    /// still sealed exactly once and stays in the snapshot.
    #[test]
    fn prepare_for_backup_skips_empty_active() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            // Idle store: two backup windows, no writes between.
            engine.prepare_for_backup().unwrap();
            assert!(
                engine.backup_segments().unwrap().is_empty(),
                "no data → no sealed segment must appear in the snapshot"
            );
            engine.prepare_for_backup().unwrap();
            assert!(
                engine.backup_segments().unwrap().is_empty(),
                "a second idle window must not add an empty sealed segment"
            );
            engine.set_gc_paused(false);

            // With data, the window seals exactly the non-empty active segment...
            let key = test_key(0x51);
            let value = b"payload".to_vec();
            engine.put(key, &value, Codec::Zstd).unwrap();
            engine.prepare_for_backup().unwrap();
            let segs = engine.backup_segments().unwrap();
            assert_eq!(segs.len(), 1);
            assert!(segs[0].bytes > 0);

            // ...and a second window with no new writes does NOT add an empty
            // sealed segment (the post-seal active is skipped, not re-sealed).
            engine.prepare_for_backup().unwrap();
            assert_eq!(engine.backup_segments().unwrap().len(), 1);
            engine.set_gc_paused(false);
            engine.shutdown().unwrap();
        }
    }
}
