use std::fs;
use std::path::Path;

use crate::bucket::IndexRecord;
use crate::error::Result;
use crate::meta::{GlobalMeta, SegmentStats};
use crate::segment::{self, SegmentReader};

/// Recover after a crash: scan any unindexed portions of segments,
/// update segment stats, and return newly discovered index records.
///
/// The caller is responsible for inserting the returned records into
/// the index store.
pub fn recover(store_root: &Path, meta: &mut GlobalMeta) -> Result<Vec<IndexRecord>> {
    scan_segments(store_root, meta, false)
}

/// Rebuild the entire index from scratch by scanning all segments from
/// offset 0.  Used when the index database is corrupted or lost.
///
/// Resets all segment stats and returns every entry found on disk.
/// The caller should replace the index database before calling this.
pub fn rebuild_index(store_root: &Path, meta: &mut GlobalMeta) -> Result<Vec<IndexRecord>> {
    // Reset all segment stats — they'll be recomputed during the scan.
    // Also reset indexed_up_to_offset so we scan from 0.
    for stats in meta.segments.values_mut() {
        stats.total_bytes = 0;
        stats.deleted_bytes = 0;
        stats.deleted_ratio = 0.0;
        stats.indexed_up_to_offset = 0;
    }
    scan_segments(store_root, meta, true)
}

/// Common implementation: scan segments and collect index records.
/// When `full_scan` is true, every segment is scanned from offset 0.
fn scan_segments(
    store_root: &Path,
    meta: &mut GlobalMeta,
    full_scan: bool,
) -> Result<Vec<IndexRecord>> {
    let seg_dir = store_root.join("segments");
    if !seg_dir.exists() {
        fs::create_dir_all(&seg_dir)?;
    }

    // Discover all segment files on disk
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

    let mut all_records: Vec<IndexRecord> = Vec::new();

    // For each segment, scan unindexed portions
    for &seg_id in &disk_segments {
        let seg_path = seg_dir.join(segment::segment_filename(seg_id));
        let file_size = fs::metadata(&seg_path)?.len();

        let mut stats = meta
            .segments
            .remove(&seg_id)
            .unwrap_or_else(|| SegmentStats::new(seg_id));
        let is_sealed = seg_id != meta.active_segment_id;
        stats.sealed = is_sealed;

        let cursor = stats.indexed_up_to_offset;

        // A sealed segment is immutable and durable, so its cursor must reach
        // EOF. If it does not, meta is stale (a pre-fix GC stored payload
        // bytes as the cursor, or an interrupted GC replaced the file) and the
        // cursor must not be trusted: resuming mid-file used to be mistaken
        // for a torn tail and truncated the segment. An unsealed cursor past
        // EOF means the file shrank under it — same treatment.
        let cursor_stale = is_sealed && cursor != file_size;
        let full_rescan = full_scan || cursor_stale || cursor > file_size;
        if cursor_stale {
            tracing::warn!(
                segment_id = seg_id,
                cursor,
                file_size,
                "sealed segment cursor does not reach EOF; rescanning from 0 \
                 and rebuilding its index"
            );
        }

        if full_rescan {
            // The scan below starts at byte 0 and re-accumulates these; the
            // stored totals describe the old indexed prefix and would
            // otherwise be counted twice.
            stats.total_bytes = 0;
            stats.deleted_bytes = 0;
        }
        if cursor > file_size {
            // The file shrank under the cursor, which only happens when an
            // interrupted GC renamed a compacted file before meta was saved.
            // The stored hash describes the pre-GC content and is now wrong;
            // backups must rehash instead of reusing it.
            stats.sha256 = None;
        }

        let scan_start = if full_rescan { 0 } else { cursor };

        if scan_start >= file_size {
            meta.segments.insert(seg_id, stats);
            continue;
        }

        let reader = SegmentReader::open(seg_path.clone(), seg_id)?;

        let truncation_point = reader.scan_entries(scan_start, |entry, offset| {
            all_records.push(IndexRecord::new(
                entry.key,
                seg_id,
                offset,
                entry.data.len() as u32,
                entry.flags,
            ));

            stats.total_bytes += entry.data.len() as u64;
            if entry.is_tombstone() {
                stats.deleted_bytes += entry.raw_size as u64;
            }

            Ok(())
        })?;

        // Truncate if tail corruption found
        if truncation_point < file_size {
            if is_sealed {
                // Sealed segments are fsynced before meta records them, so
                // this is real on-disk damage (or a stale cursor in a store
                // written before that invariant existed), never an in-flight
                // write. Say so instead of losing the tail silently.
                tracing::warn!(
                    segment_id = seg_id,
                    file_size,
                    truncation_point,
                    lost_bytes = file_size - truncation_point,
                    "sealed segment has a corrupt tail; truncating"
                );
            }
            segment::truncate_segment(&seg_path, truncation_point)?;
            // The hashed bytes are gone: a stored hash no longer describes
            // the file. Backups must rehash rather than upload truncated
            // content under the old hash (which would poison the object at
            // that key for the whole restore point).
            stats.sha256 = None;
        }

        stats.indexed_up_to_offset = truncation_point;
        stats.recompute_ratio();
        meta.segments.insert(seg_id, stats);
    }

    meta.save(store_root)?;

    Ok(all_records)
}

/// Marker recording that stored segment hashes were dropped once after the
/// GC index-cursor fix. See [`invalidate_stale_segment_hashes`].
pub const HASH_REPAIR_MARKER: &str = ".segment-hash-repair-v1";

/// Drop every stored segment hash the first time this build opens an existing
/// store.
///
/// Before the GC cursor fix, a restart could truncate the tail of a compacted
/// segment while meta still held the hash of the *pre-truncation* file.
/// Nothing in meta records the file size a hash belongs to, so such a hash
/// cannot be detected or repaired in place. Dropping them all is safe:
/// `Engine::backup_segments` rehashes any segment whose hash is `None` and
/// persists the result, so the only cost is one full rehash at the next
/// backup.
///
/// Returns `true` when the caller must write [`HASH_REPAIR_MARKER`] *after*
/// persisting meta, so a crash can never leave the marker without the reset.
pub fn invalidate_stale_segment_hashes(
    store_root: &Path,
    meta: &mut GlobalMeta,
) -> Result<bool> {
    if store_root.join(HASH_REPAIR_MARKER).exists() {
        return Ok(false);
    }

    let mut cleared = 0usize;
    for stats in meta.segments.values_mut() {
        if stats.sha256.take().is_some() {
            cleared += 1;
        }
    }
    if cleared > 0 {
        tracing::warn!(
            segments = cleared,
            "dropped stored segment hashes once after the GC cursor fix; \
             they will be recomputed on the next backup"
        );
    }
    Ok(true)
}

/// Persist [`HASH_REPAIR_MARKER`] durably.
pub fn mark_hash_repair_done(store_root: &Path) -> Result<()> {
    use std::io::Write;

    let mut file = fs::File::create(store_root.join(HASH_REPAIR_MARKER))?;
    file.write_all(b"1")?;
    file.sync_all()?;
    Ok(())
}

/// Clean up leftover temp files from interrupted GC.
pub fn cleanup_temp_files(store_root: &Path) -> Result<()> {
    let seg_dir = store_root.join("segments");
    if seg_dir.exists() {
        for entry in fs::read_dir(&seg_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("temp_") {
                let path = entry.path();
                tracing::warn!("Removing leftover temp file: {:?}", path);
                fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}
