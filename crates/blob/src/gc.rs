use std::fs;
use std::path::{Path, PathBuf};

use crate::bucket::{IndexRecord, IndexStore};
use crate::error::Result;
use crate::meta::GlobalMeta;
use crate::segment::{self, SegmentReader, SegmentWriter};

/// Result of a GC run.
#[derive(Debug)]
pub struct GcStats {
    pub segment_id: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub entries_kept: usize,
    pub entries_skipped: usize,
    /// SHA-256 of the compacted segment (recomputed after the rewrite, since
    /// a seal-time hash would be stale). See [`GcPrepare::sha256`].
    pub sha256: [u8; 32],
}

/// Prepared GC result — the compacted segment has been written to a temp file
/// but not yet renamed over the original.  `gc_finish` must be called to commit.
pub struct GcPrepare {
    pub segment_id: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub entries_kept: usize,
    pub entries_skipped: usize,
    /// Index records for kept entries with their new offsets in the compacted segment.
    pub kept_records: Vec<IndexRecord>,
    /// Keys whose tombstone IndexRecord should be removed from redb after
    /// this segment is compacted (the tombstone entries they pointed to are gone).
    pub deleted_keys: Vec<[u8; 32]>,
    /// SHA-256 of the compacted file, computed on the temp file right after it
    /// is fsynced (page-cache warm, before the rename — no write lock held).
    pub sha256: [u8; 32],
    temp_path: PathBuf,
    seg_path: PathBuf,
    /// Removes the temp file if this prepare is dropped before `gc_finish`
    /// renamed it into place (scan, fsync, hashing or rename failure).
    _temp_guard: TempSegmentGuard,
}

/// Deletes a GC temp segment when dropped, unless it has already been renamed
/// over the original. `gc_prepare` has several early `?` returns; without this
/// each failure leaked one `temp_*.seg`, and the background GC retried the
/// same failing segment every cycle until the disk filled up.
struct TempSegmentGuard {
    path: PathBuf,
}

impl Drop for TempSegmentGuard {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "failed to remove GC temp segment"
            ),
        }
    }
}

/// Phase 1: pick the sealed segment with the highest deleted_ratio, then for
/// each entry in that segment consult the bucket index to decide whether it is
/// still the latest version.  Live entries are written to a temp file; stale
/// entries and tombstones are skipped.  Does NOT rename — the caller should
/// hold the write lock only during `gc_finish`.
pub fn gc_prepare(
    store_root: &Path,
    meta: &GlobalMeta,
    deleted_ratio_threshold: f64,
    index_store: &IndexStore,
) -> Result<Option<GcPrepare>> {
    let candidate = meta
        .segments
        .values()
        .filter(|s| s.sealed && s.deleted_ratio >= deleted_ratio_threshold)
        .max_by(|a, b| a.deleted_ratio.partial_cmp(&b.deleted_ratio).unwrap());

    let target = match candidate {
        Some(s) => s.clone(),
        None => return Ok(None),
    };

    let seg_path = store_root
        .join("segments")
        .join(segment::segment_filename(target.segment_id));

    // Guard against stale meta entries: if the segment file was deleted
    // (e.g. after a prior GC emptied it), skip this candidate.
    if !seg_path.exists() {
        return Ok(None);
    }

    let reader = SegmentReader::open(seg_path.clone(), target.segment_id)?;

    // Create temp segment (not renamed yet)
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_name = format!("temp_{:016x}.seg", timestamp);
    let temp_path = store_root.join("segments").join(&temp_name);
    let mut writer = SegmentWriter::create(temp_path.clone(), target.segment_id)?;
    // From here on, any error path below removes the partial temp segment.
    let temp_guard = TempSegmentGuard {
        path: temp_path.clone(),
    };

    let mut kept_records: Vec<IndexRecord> = Vec::new();
    let mut deleted_keys: Vec<[u8; 32]> = Vec::new();
    let mut bytes_after: u64 = 0;
    let mut entries_kept: usize = 0;
    let mut entries_skipped: usize = 0;

    reader.scan_entries(0, |entry, offset| {
        if entry.is_tombstone() {
            // If this tombstone is the latest version in the index, the key
            // must be removed from redb after compaction — otherwise it
            // accumulates forever.
            match index_store.get(&entry.key)? {
                Some(rec)
                    if rec.segment_id == target.segment_id && rec.offset == offset =>
                {
                    deleted_keys.push(entry.key);
                }
                _ => {}
            }
            entries_skipped += 1;
            return Ok(());
        }

        // Ask the bucket index whether this entry is still the latest version.
        match index_store.get(&entry.key)? {
            Some(rec) if rec.segment_id == target.segment_id && rec.offset == offset => {
                let new_offset = writer.append(entry)?;
                kept_records.push(IndexRecord::new(
                    entry.key,
                    target.segment_id,
                    new_offset,
                    entry.data.len() as u32,
                    entry.flags,
                ));
                bytes_after += entry.data.len() as u64;
                entries_kept += 1;
            }
            _ => {
                entries_skipped += 1;
            }
        }
        Ok(())
    })?;

    writer.fsync()?;
    // Compaction rewrites the segment, so the seal-time hash is stale — hash
    // the new content now, while it is page-cache warm (design doc §7).
    let sha256 = crate::checksum::sha256_file(&temp_path)?;

    Ok(Some(GcPrepare {
        segment_id: target.segment_id,
        bytes_before: target.total_bytes,
        bytes_after,
        entries_kept,
        entries_skipped,
        kept_records,
        deleted_keys,
        sha256,
        temp_path,
        seg_path,
        _temp_guard: temp_guard,
    }))
}

/// Phase 2: atomically replace the old segment with the compacted one.
pub fn gc_finish(prep: GcPrepare) -> Result<GcStats> {
    fs::rename(&prep.temp_path, &prep.seg_path)?;
    Ok(GcStats {
        segment_id: prep.segment_id,
        bytes_before: prep.bytes_before,
        bytes_after: prep.bytes_after,
        entries_kept: prep.entries_kept,
        entries_skipped: prep.entries_skipped,
        sha256: prep.sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn temp_guard_removes_partial_segment_on_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("temp_0000deadbeef.seg");
        fs::write(&path, b"partial").unwrap();

        {
            let _guard = TempSegmentGuard { path: path.clone() };
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_guard_ignores_already_renamed_segment() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("temp_0000cafebabe.seg");
        drop(TempSegmentGuard { path });
    }
}
