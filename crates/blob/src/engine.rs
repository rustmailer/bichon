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
        let (recovered_records, full_rescan_segments) = if index_rebuilt {
            crate::recovery::rebuild_index(path, &mut meta)?
        } else {
            crate::recovery::recover(path, &mut meta)?
        };
        // Which recovered records may overwrite what the index already holds:
        //
        // * A record from a segment that was *not* fully rescanned comes from
        //   the incremental region past the cursor. The cursor only advances
        //   past an entry after that entry's index insert committed, so such
        //   a record is always at least as new as anything the index holds
        //   for the key — it can be applied unconditionally. (It is either
        //   identical to the indexed record, or its index insert was lost to
        //   a crash and the index still holds an older version.)
        //
        // * A record from a fully rescanned segment reflects the segment's
        //   current on-disk content, byte for byte. For the same segment the
        //   index cannot hold anything more recent — the rescan covers the
        //   whole file — so the rescan record wins unconditionally. This is
        //   what heals a GC interrupted between renaming the compacted file
        //   into place and updating the index: the index still carries the
        //   pre-compaction offsets, which are larger and used to win the
        //   recency comparison, leaving the stale records in place
        //   permanently. Only a record in a *newer* segment outranks the
        //   rescan (a key deleted or re-written there after this segment
        //   was compacted; applying the older record would resurrect it).
        //
        // A rebuild starts from an empty index (the DB file is recreated), so
        // every record is new there; skip the per-record lookup in that case.
        let recovered_records = if index_rebuilt {
            recovered_records
        } else {
            let mut current: Vec<crate::bucket::IndexRecord> =
                Vec::with_capacity(recovered_records.len());
            for rec in recovered_records {
                if full_rescan_segments.contains(&rec.segment_id) {
                    // `get_raw` so a newer tombstone counts as a newer
                    // record too.
                    if let Some(existing) = index_store.get_raw(&rec.key)? {
                        if existing.segment_id > rec.segment_id {
                            continue;
                        }
                    }
                }
                current.push(rec);
            }
            current
        };
        // One-time repair for stores written by builds where a GC-compacted
        // segment's hash could describe the file as it was *before* an old
        // restart truncated it. That cannot be detected from meta, so all
        // stored hashes are dropped once; backups rehash lazily and persist
        // the fresh value.
        let reset_stale_hashes =
            crate::recovery::invalidate_stale_segment_hashes(path, &mut meta)?;
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
        if reset_stale_hashes {
            // Marker after the save: a crash in between simply repeats the
            // (idempotent) reset on the next start, never the other way round.
            crate::recovery::mark_hash_repair_done(path)?;
        }

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
                                    "GC compacted segment {}: {} → {} payload bytes \
                                     ({} bytes on disk, kept {}, skipped {})",
                                    stats.segment_id,
                                    stats.bytes_before,
                                    stats.bytes_after,
                                    stats.bytes_on_disk,
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

        // The entry's CRC says the *entry* is intact, but not that it is the
        // entry this record points at: a stale index offset that happens to
        // land on another entry's header would otherwise return that entry's
        // data for this key, silently. Fail loudly instead — an index that
        // disagrees with segment data is corruption, whatever wrote it.
        if entry.key != *key {
            return Err(Error::CorruptEntry {
                path: seg_path,
                offset: record.offset,
                reason: format!(
                    "index points at an entry for a different key (index key \
                     {key:02x?}, entry key {:02x?}); the index is inconsistent \
                     with the segment data",
                    entry.key
                ),
            });
        }

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

            if stats.bytes_on_disk == 0 {
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
                    // `total_bytes` is payload-only (same unit as the append
                    // path); the index cursor is a physical offset and must be
                    // the size of the rewritten file.
                    seg_stats.total_bytes = stats.bytes_after;
                    seg_stats.deleted_bytes = 0;
                    seg_stats.deleted_ratio = 0.0;
                    seg_stats.indexed_up_to_offset = stats.bytes_on_disk;
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
        // Persist the segment before meta records it as sealed. Recovery treats
        // a sealed segment as immutable and fully durable (a short cursor then
        // means stale meta, not a torn write), so meta must never claim
        // `sealed` while the tail is still only in the page cache.
        self.active_writer.fsync()?;
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

    // ── GC index-cursor regressions (#393 follow-up) ─────────────────────

    fn key_from_u32(v: u32) -> [u8; 32] {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&v.to_le_bytes());
        key
    }

    fn segment_path(root: &std::path::Path, id: u32) -> std::path::PathBuf {
        root.join("segments").join(segment::segment_filename(id))
    }

    /// Compact a store of `n` tiny entries (half deleted) and return the
    /// surviving value, the compacted segment id, the payload total, and the
    /// physical file length.
    fn compacted_store(
        dir: &TempDir,
        config: Config,
        n: u32,
        value_len: usize,
    ) -> (Vec<u8>, u32, u64, u64) {
        let value = vec![b'Z'; value_len];
        {
            let engine = Engine::open(dir.path(), config).unwrap();
            for i in 0..n {
                engine.put(key_from_u32(i), &value, Codec::None).unwrap();
            }
            for i in (0..n).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            let stats = engine
                .gc()
                .unwrap()
                .expect("the sealed, half-deleted segment must be compacted");
            assert_eq!(stats.entries_kept as u32, n / 2);
            assert_eq!(
                stats.bytes_on_disk,
                stats.bytes_after + ENTRY_HEADER_SIZE as u64 * stats.entries_kept as u64,
                "on-disk size must include one header per kept entry"
            );
            let file_len = fs::metadata(segment_path(dir.path(), stats.segment_id))
                .unwrap()
                .len();
            assert_eq!(stats.bytes_on_disk, file_len);
            (value, stats.segment_id, stats.bytes_after, file_len)
        }
    }

    /// The compacted segment's index cursor must be its physical size. Storing
    /// the payload count instead made recovery resume mid-entry on the next
    /// open, treat the remainder as a torn tail and truncate it.
    ///
    /// Tiny entries make the old failure catastrophic (~5/6 of the segment),
    /// not a last-few-records loss.
    #[test]
    fn gc_cursor_is_physical_and_reopen_keeps_segment_tail() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let (value, seg_id, payload, file_len) = compacted_store(&dir, config.clone(), 200, 10);

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            let inner = engine.shared.inner.read().unwrap();
            let stats = inner.meta.segments.get(&seg_id).unwrap();
            assert_eq!(
                stats.indexed_up_to_offset, file_len,
                "the index cursor must be the physical file size"
            );
            assert_eq!(
                stats.total_bytes, payload,
                "total_bytes stays payload-only, like the append path"
            );
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            fs::metadata(segment_path(dir.path(), seg_id)).unwrap().len(),
            file_len,
            "reopening must not truncate a compacted segment"
        );
        for i in (1..200u32).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(value.clone()),
                "key {i} lost across reopen"
            );
        }
        let inner = engine.shared.inner.read().unwrap();
        assert_eq!(
            inner.meta.segments[&seg_id].indexed_up_to_offset,
            file_len
        );
    }

    /// A store written by a pre-fix build carries the payload count in the
    /// cursor. Opening it must rebuild that segment from byte 0 — never treat
    /// the stale cursor as a torn tail.
    #[test]
    fn legacy_payload_cursor_is_repaired_without_data_loss() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let (value, seg_id, payload, file_len) = compacted_store(&dir, config.clone(), 80, 32);
        assert!(payload < file_len);

        // Rewrite meta exactly as the old build left it.
        let mut meta = crate::meta::GlobalMeta::load(dir.path()).unwrap();
        meta.segments
            .get_mut(&seg_id)
            .unwrap()
            .indexed_up_to_offset = payload;
        meta.save(dir.path()).unwrap();

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            fs::metadata(segment_path(dir.path(), seg_id)).unwrap().len(),
            file_len,
            "a stale cursor must not be mistaken for a torn tail"
        );
        for i in (1..80u32).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(value.clone()),
                "key {i} lost while repairing a stale cursor"
            );
        }
        let inner = engine.shared.inner.read().unwrap();
        let stats = inner.meta.segments.get(&seg_id).unwrap();
        assert_eq!(stats.indexed_up_to_offset, file_len);
        assert_eq!(
            stats.total_bytes, payload,
            "a full rescan must rebuild totals, not add to them"
        );
    }

    /// Repairing a stale cursor must not resurrect a key that was deleted in a
    /// newer segment after the compacted segment was written. The index holds
    /// the tombstone (newer segment/offset), so the older data record found by
    /// the rescan must be discarded.
    #[test]
    fn stale_cursor_rescan_does_not_resurrect_deleted_keys() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let value = vec![b'R'; 16];
        let n: u32 = 40;

        let (seg_id, payload) = {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..n {
                engine.put(key_from_u32(i), &value, Codec::None).unwrap();
            }
            for i in (0..n).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            let stats = engine.gc().unwrap().expect("segment must be compacted");

            // Delete a surviving key *after* the compaction: its tombstone
            // lands in the new active segment.
            engine.delete(&key_from_u32(1)).unwrap();
            (stats.segment_id, stats.bytes_after)
        };

        let mut meta = crate::meta::GlobalMeta::load(dir.path()).unwrap();
        meta.segments
            .get_mut(&seg_id)
            .unwrap()
            .indexed_up_to_offset = payload;
        meta.save(dir.path()).unwrap();

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            engine.get(&key_from_u32(1)).unwrap(),
            None,
            "the rescan resurrected a key deleted in a newer segment"
        );
        assert_eq!(
            engine.get(&key_from_u32(3)).unwrap(),
            Some(value.clone()),
            "untouched keys must survive the repair"
        );
    }

    /// Helper: run `gc_prepare` against the live engine and commit only the
    /// rename, simulating a GC interrupted at the given point:
    ///
    /// * `AfterRename` — crash between `gc_finish` and the index update. The
    ///   file is the compacted one, the index still holds pre-compaction
    ///   offsets, and meta still records the pre-compaction cursor (past EOF).
    /// * `AfterIndexUpdate` — crash after the index update but before the new
    ///   cursor is saved. The index is correct, the cursor is stale.
    /// * `BeforeRename` — crash while the compacted file is still a temp
    ///   segment; nothing was committed.
    #[derive(Clone, Copy, PartialEq)]
    enum Interrupt {
        BeforeRename,
        AfterRename,
        AfterIndexUpdate,
    }

    fn interrupt_gc(engine: &Engine, at: Interrupt) {
        let prep = {
            let inner = engine.shared.inner.read().unwrap();
            crate::gc::gc_prepare(
                &inner.root,
                &inner.meta,
                0.0,
                &engine.shared.index_store,
            )
            .unwrap()
            .expect("the sealed segment must be a candidate")
        };
        if at == Interrupt::BeforeRename {
            drop(prep); // the temp guard removes the uncommitted segment
            return;
        }
        let mut prep = prep;
        let kept_records = std::mem::take(&mut prep.kept_records);
        let deleted_keys = std::mem::take(&mut prep.deleted_keys);
        crate::gc::gc_finish(prep).unwrap();
        if at == Interrupt::AfterIndexUpdate {
            engine
                .shared
                .index_store
                .insert_batch(&kept_records)
                .unwrap();
            engine.shared.index_store.delete_batch(&deleted_keys).unwrap();
        }
    }

    /// Deterministic per-key value, so a key that comes back with another
    /// key's data is caught byte-exactly, not just as "some wrong bytes".
    fn distinct_value(i: u32, len: usize) -> Vec<u8> {
        (0..len).map(|j| (i * 31 + j as u32 * 17) as u8).collect()
    }

    /// The minimal interrupted-GC regression: crash right between the rename
    /// and the index update. Recovery's full rescan must heal the stale index
    /// — before the fix, the recency guard kept the stale (larger) offsets
    /// and `get` silently returned *another key's data* for this one.
    #[test]
    fn interrupted_gc_after_rename_is_healed_by_rescan() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..4u32 {
                engine
                    .put(key_from_u32(i), &distinct_value(i, 10), Codec::None)
                    .unwrap();
            }
            engine.delete(&key_from_u32(0)).unwrap();
            engine.seal_active_segment().unwrap();
            interrupt_gc(&engine, Interrupt::AfterRename);
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        for i in 1..4u32 {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(distinct_value(i, 10)),
                "key {i} must come back with its own data after the heal"
            );
        }
        assert_eq!(
            engine.get(&key_from_u32(0)).unwrap(),
            None,
            "the key deleted before compaction must stay deleted"
        );
    }

    /// Crash after the index update but before the new cursor is saved: the
    /// index already holds the compacted offsets, meta still records the
    /// pre-compaction size. The rescan must agree with the index, not fight
    /// it.
    #[test]
    fn interrupted_gc_after_index_update_is_healed_by_rescan() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..6u32 {
                engine
                    .put(key_from_u32(i), &distinct_value(i, 12), Codec::None)
                    .unwrap();
            }
            for i in (0..6u32).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            interrupt_gc(&engine, Interrupt::AfterIndexUpdate);
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        for i in (1..6u32).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(distinct_value(i, 12)),
                "key {i} must survive the heal with its own data"
            );
        }
        for i in (0..6u32).step_by(2) {
            assert_eq!(engine.get(&key_from_u32(i)).unwrap(), None);
        }
    }

    /// Crash before the rename: the original file is intact, the index still
    /// matches it, only the temp segment is lost (removed by its guard). The
    /// store must reopen with every entry accounted for.
    #[test]
    fn interrupted_gc_before_rename_leaves_store_intact() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..10u32 {
                engine
                    .put(key_from_u32(i), &distinct_value(i, 20), Codec::None)
                    .unwrap();
            }
            for i in (0..10u32).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            interrupt_gc(&engine, Interrupt::BeforeRename);
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        for i in (1..10u32).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(distinct_value(i, 20)),
                "key {i} must be untouched by the interrupted GC"
            );
        }
        for i in (0..10u32).step_by(2) {
            assert_eq!(engine.get(&key_from_u32(i)).unwrap(), None);
        }
    }

    /// The interrupted-GC heal at scale: every surviving key must come back
    /// byte-exact — a single misaligned offset in the healed index would show
    /// up as another key's data — and the rebuilt stats must match the
    /// compacted file, not the stale pre-compaction totals.
    #[test]
    fn interrupted_gc_heal_is_byte_exact_for_every_key() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        const N: u32 = 400;
        const VALUE_LEN: u64 = 64;

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..N {
                engine
                    .put(key_from_u32(i), &distinct_value(i, VALUE_LEN as usize), Codec::None)
                    .unwrap();
            }
            for i in (0..N).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            interrupt_gc(&engine, Interrupt::AfterRename);
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        for i in (1..N).step_by(2) {
            let got = engine.get(&key_from_u32(i)).unwrap();
            assert_eq!(
                got,
                Some(distinct_value(i, VALUE_LEN as usize)),
                "key {i}: healed index must point at the key's own entry"
            );
        }
        for i in (0..N).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                None,
                "key {i}: deleted before compaction, must not resurrect"
            );
        }
        // The full rescan rebuilds totals from the compacted file: half the
        // entries survived, so total_bytes is half the original payload sum.
        assert_eq!(
            engine.stats().unwrap().total_bytes,
            VALUE_LEN * (N / 2) as u64,
            "stats must describe the compacted file, not the stale pre-GC totals"
        );
    }

    /// A GC that fails mid-commit does not take the engine down: the process
    /// keeps serving, and later writes land in a newer segment. When the
    /// store is eventually reopened, the heal must honour those newer
    /// records — a key deleted or re-written after the interruption must not
    /// be resurrected from the rescanned segment — while untouched keys are
    /// healed from it.
    #[test]
    fn healed_rescan_does_not_resurrect_keys_rewritten_after_interrupted_gc() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        const N: u32 = 20;
        let old_value = |i: u32| distinct_value(i, 16);

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for i in 0..N {
                engine.put(key_from_u32(i), &old_value(i), Codec::None).unwrap();
            }
            for i in (0..N).step_by(2) {
                engine.delete(&key_from_u32(i)).unwrap();
            }
            engine.seal_active_segment().unwrap();
            interrupt_gc(&engine, Interrupt::AfterRename);

            // The engine keeps serving after the failed GC: these land in the
            // new active segment and are indexed normally.
            engine.delete(&key_from_u32(1)).unwrap(); // survivor, deleted later
            engine
                .put(key_from_u32(3), b"rewritten after the interrupted GC", Codec::None)
                .unwrap(); // survivor, re-written later
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            engine.get(&key_from_u32(1)).unwrap(),
            None,
            "a key deleted in a newer segment after the interruption must stay deleted"
        );
        assert_eq!(
            engine.get(&key_from_u32(3)).unwrap(),
            Some(b"rewritten after the interrupted GC".to_vec()),
            "the newer version written after the interruption must win"
        );
        for i in (5..N).step_by(2) {
            assert_eq!(
                engine.get(&key_from_u32(i)).unwrap(),
                Some(old_value(i)),
                "key {i} must be healed from the rescanned segment byte-exactly"
            );
        }
        for i in (0..N).step_by(2) {
            assert_eq!(engine.get(&key_from_u32(i)).unwrap(), None);
        }
    }

    /// Second line of defence for `get`: if the index ever disagrees with the
    /// segment data (a record pointing at another key's entry), the read must
    /// fail loudly instead of returning that entry's data as this key's.
    #[test]
    fn get_rejects_index_pointing_at_another_keys_entry() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        let engine = Engine::open(dir.path(), config).unwrap();
        engine.put(key_from_u32(1), b"value one", Codec::None).unwrap();
        engine.put(key_from_u32(2), b"value two", Codec::None).unwrap();

        // Corrupt the index by hand: point key 1's record at key 2's entry.
        let rec2 = engine
            .shared
            .index_store
            .get_raw(&key_from_u32(2))
            .unwrap()
            .expect("key 2 must be indexed");
        let forged = crate::bucket::IndexRecord::new(
            key_from_u32(1),
            rec2.segment_id,
            rec2.offset,
            rec2.data_size,
            0,
        );
        engine.shared.index_store.insert_batch(&[forged]).unwrap();

        match engine.get(&key_from_u32(1)) {
            Err(Error::CorruptEntry { .. }) => {}
            other => panic!(
                "a record pointing at another key's entry must fail with \
                 CorruptEntry, got {other:?}"
            ),
        }
        // The healthy record is unaffected.
        assert_eq!(
            engine.get(&key_from_u32(2)).unwrap(),
            Some(b"value two".to_vec())
        );
    }

    /// A cursor past EOF (crash before the meta save recorded the tail) must
    /// also rescan without double-counting `total_bytes`.
    #[test]
    fn cursor_past_eof_rescan_does_not_double_count_totals() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let value = vec![b'P'; 100];
        let keys: Vec<[u8; 32]> = (0..10u32).map(key_from_u32).collect();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            for key in &keys {
                engine.put(*key, &value, Codec::None).unwrap();
            }
            drop(engine);
        }

        let mut meta = crate::meta::GlobalMeta::load(dir.path()).unwrap();
        let active = meta.active_segment_id;
        meta.segments.get_mut(&active).unwrap().indexed_up_to_offset = u64::MAX / 2;
        meta.save(dir.path()).unwrap();

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            engine.stats().unwrap().total_bytes,
            100 * keys.len() as u64,
            "a rescan from 0 must reset totals first"
        );
        for key in &keys {
            assert_eq!(engine.get(key).unwrap(), Some(value.clone()));
        }
    }

    /// The one-time hash repair: hashes stored by pre-fix builds are dropped
    /// on the first open, and hashes written afterwards are preserved.
    #[test]
    fn stale_segment_hashes_are_dropped_exactly_once() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let seg_id = {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            engine.put(test_key(1), b"x", Codec::None).unwrap();
            let id = engine.seal_active_segment().unwrap();
            assert!(
                engine.shared.inner.read().unwrap().meta.segments[&id]
                    .sha256
                    .is_some(),
                "sealing records a hash"
            );
            id
        };

        // Simulate a store written by a pre-fix build: it never wrote the
        // marker, because the repair did not exist yet.
        fs::remove_file(dir.path().join(crate::recovery::HASH_REPAIR_MARKER)).unwrap();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            assert!(
                engine.shared.inner.read().unwrap().meta.segments[&seg_id]
                    .sha256
                    .is_none(),
                "the first open after the fix must drop pre-fix hashes"
            );
        }
        assert!(dir
            .path()
            .join(crate::recovery::HASH_REPAIR_MARKER)
            .exists());

        // A hash written by this build survives later opens.
        let mut meta = crate::meta::GlobalMeta::load(dir.path()).unwrap();
        meta.segments.get_mut(&seg_id).unwrap().sha256 = Some([7u8; 32]);
        meta.save(dir.path()).unwrap();

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            engine.shared.inner.read().unwrap().meta.segments[&seg_id].sha256,
            Some([7u8; 32]),
            "the repair must run only once"
        );
    }

    /// A segment whose only survivors have empty payloads compacts to zero
    /// payload bytes but still occupies a header each. The emptied-segment
    /// check must look at the on-disk size; deleting the file on
    /// `bytes_after == 0` would leave dangling index records.
    #[test]
    fn gc_keeps_segment_holding_empty_values() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            engine.put(key_from_u32(0), &[b'X'; 100], Codec::None).unwrap();
            engine.put(key_from_u32(1), b"", Codec::None).unwrap();
            engine.seal_active_segment().unwrap();
            engine.delete(&key_from_u32(0)).unwrap();

            let stats = engine.gc().unwrap().expect("deleted payload makes it a candidate");
            assert_eq!(stats.entries_kept, 1);
            assert_eq!(stats.bytes_after, 0, "the survivor has no payload bytes");
            assert_eq!(
                stats.bytes_on_disk, ENTRY_HEADER_SIZE as u64,
                "but it still occupies a header"
            );
            assert!(
                segment_path(dir.path(), stats.segment_id).exists(),
                "a segment holding an empty-value entry must not be deleted"
            );
        }

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            engine.get(&key_from_u32(1)).unwrap(),
            Some(Vec::new()),
            "the empty-value entry must survive GC and a restart"
        );
    }

    /// After the one-time hash repair, a backup must recompute and persist the
    /// real hash instead of trusting the dropped value.
    #[test]
    fn backup_rehashes_segments_after_hash_repair() {
        let dir = TempDir::new().unwrap();
        let config = no_background_config();

        let (seg_id, expected) = {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            engine.put(test_key(3), b"hash me", Codec::None).unwrap();
            let id = engine.seal_active_segment().unwrap();
            let hash = engine.shared.inner.read().unwrap().meta.segments[&id]
                .sha256
                .expect("seal records a hash");
            (id, hash)
        };

        fs::remove_file(dir.path().join(crate::recovery::HASH_REPAIR_MARKER)).unwrap();
        let engine = Engine::open(dir.path(), config).unwrap();
        assert!(engine.shared.inner.read().unwrap().meta.segments[&seg_id]
            .sha256
            .is_none());

        let segments = engine.backup_segments().unwrap();
        let artifact = segments
            .iter()
            .find(|s| s.segment_id == seg_id)
            .expect("sealed segment should be part of a backup");
        assert_eq!(
            artifact.sha256, expected,
            "the backup must recompute the segment hash"
        );
        assert_eq!(
            engine.shared.inner.read().unwrap().meta.segments[&seg_id].sha256,
            Some(expected),
            "the recomputed hash must be persisted"
        );
    }

    /// Truncating a corrupt sealed tail must invalidate the stored hash, or a
    /// backup would upload the shortened file under the old content's hash and
    /// poison that object for the restore point.
    #[test]
    fn corrupt_sealed_tail_is_truncated_and_hash_invalidated() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let config = no_background_config();
        let value = b"sealed payload".to_vec();

        let (seg_id, file_len) = {
            let engine = Engine::open(dir.path(), config.clone()).unwrap();
            engine.put(test_key(2), &value, Codec::None).unwrap();
            let id = engine.seal_active_segment().unwrap();
            // Pretend this store was already opened by a fixed build, so the
            // one-time repair does not mask the truncation behaviour.
            crate::recovery::mark_hash_repair_done(dir.path()).unwrap();
            let len = fs::metadata(segment_path(dir.path(), id)).unwrap().len();
            (id, len)
        };

        let path = segment_path(dir.path(), seg_id);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0xAB; 37]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let engine = Engine::open(dir.path(), config).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            file_len,
            "the corrupt tail must be cut back to the last complete entry"
        );
        assert_eq!(engine.get(&test_key(2)).unwrap(), Some(value));
        assert!(
            engine.shared.inner.read().unwrap().meta.segments[&seg_id]
                .sha256
                .is_none(),
            "a hash of the pre-truncation bytes must not survive"
        );
    }
}
