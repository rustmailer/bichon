# bichon-blob

Content-addressable blob store for [Bichon](https://github.com/rustmailer/bichon) email archival.

All data is keyed by a 32-byte content hash — identical content is stored only once. The caller (Bichon) strips attachments from emails and stores the "holed" raw email + attachments into this store, tracking reference counts in its own schema.

## On-disk layout

```
<root>/
├── meta.bin              # global metadata (bincode + CRC32)
├── index.redb            # key → (segment_id, offset, size) index (redb B-tree)
├── segments/
│   ├── 00000001.seg      # append-only segment files (≤ 1 GB each)
│   ├── 00000002.seg
│   └── ...
```

### Segment entry format (50-byte fixed header + variable data)

```
magic(4)  crc32(4)  flags(1)  codec(1)  key(32)  raw_size(4)  data_size(4)  data(*)
```

- `magic`: `0xB3DB_0001` — entry boundary validation
- `crc32`: covers everything after this field
- `flags`: `0` = live, `1` = tombstone
- `codec`: `0` = none, `1` = Zstd, `2` = Lz4
- `key`: 32-byte content hash (BLAKE3 / SHA-256)
- `raw_size`: original uncompressed size
- `data_size`: on-disk data size (after compression)

### Index store

The key → (segment_id, offset, data_size, flags) mapping is stored in a single [redb](https://github.com/cberner/redb) database (`index.redb`). redb provides:

- **B-tree + mmap**: O(log N) point lookups with zero heap allocation — pages are faulted in on demand.
- **ACID transactions**: every index write is durable and atomic.
- **Crash recovery**: handled transparently by redb's WAL — no manual reload or rebuild logic.
- **O(1) startup**: only the B-tree root page is read at open time.

Records are stored as fixed-size 56-byte blobs, each carrying an internal CRC32 checksum.

## Read path

```
get(key)
  → index_store.get(key)           # redb B-tree lookup, zero-copy
  → IndexRecord CRC32 verify       # (segment_id, offset, data_size, flags)
  → pread entry from segment file
  → entry CRC32 verify → decompress → return value
```

The index record and the segment entry carry independent CRC32 checksums. Corruption in one record or entry is contained — it never affects other keys.

## Write path

```
put(key, value, codec)
  → compress value (Zstd/Lz4 if ≥ 4 KB, else store raw)
  → append entry to active segment file
  → insert IndexRecord into redb (single write txn)
  → update metadata (indexed_up_to_offset)
  → if segment ≥ 1 GB → seal it, create new segment
```

## Delete

Deletes are **tombstones** — an entry with `flags=1` and empty data is appended to the active segment. Before writing the tombstone, the existing index record is consulted to increment `deleted_bytes` on the **original** segment (the one that holds the live data). This drives the GC threshold.

```
delete(key)
  → index_store.get(key) → find original (segment_id, data_size)
  → original_segment.deleted_bytes += data_size
  → recompute deleted_ratio on original segment
  → append tombstone entry to active segment
  → insert tombstone IndexRecord into redb
  → index_store.get(key) now returns None
```

## GC

**Segment GC:** two-phase, driven by the `deleted_ratio` tracked per segment.

### Trigger

- **Background**: the `blob-gc` thread wakes up every `gc_interval_secs` (default 300s), checks whether any sealed segment's `deleted_ratio ≥ gc_deleted_ratio` (default 0.30), and runs GC on the worst segment if so.
- **Manual**: `engine.gc_if_needed()` or `engine.gc()`.

### Phase 1 — scan & compact (read-only, no write lock)

1. Pick the sealed segment with the highest `deleted_ratio`.
2. Scan only that segment's entries.
3. For each entry, ask the index: "is this entry still the latest version for its key?"
   - If the index points to this exact `(segment_id, offset)` → **keep**, write to a temp segment file.
   - If the index points elsewhere (overwritten by a later segment, or a tombstone) → **skip** (stale).
4. Fsync the temp file.

Phase 1 holds only the read lock — `put` / `delete` continue uninterrupted.

### Phase 2 — commit & update index (write lock)

1. Atomically rename the temp file over the original segment.
2. Batch-insert new `IndexRecord`s (now at new offsets) into redb. Old records with the same key are naturally overwritten.
3. Reset the segment's `deleted_bytes` and `deleted_ratio` to zero.
4. Persist metadata.

Phase 2 holds the write lock, but is fast — no full segment scan, no full index rebuild.

## Data integrity

Every record on disk is independently checksummed:

| Layer | Format | Protection |
|---|---|---|
| Segment entry | 50-byte header + data | CRC32 covers all fields + data |
| Index record | 56 bytes | CRC32 covers key + segment_id + offset + data_size + flags |
| Global metadata | bincode blob | CRC32 + version header |

Corruption is **contained** — a bad segment entry or index record produces an error for that key only. Recovery and GC skip corrupt records (with a warning) rather than aborting. The index is backed by redb's B-tree which maintains its own internal integrity.

## Crash recovery

- Temp files from interrupted GC are cleaned up on open.
- Any segment data beyond `indexed_up_to_offset` is scanned and inserted into the index.
- Partial writes at the tail of a segment (detected via CRC32 mismatch or a short read near EOF) are truncated.
- Appends that fail mid-write are rolled back to the last complete entry, so a graceful ENOSPC does not leave a torn tail in the first place.
- redb's WAL ensures the index is always consistent — no manual reload or rebuild needed.

## Background threads

Set `Config.flush_interval_secs` and `Config.gc_interval_secs` to positive values to enable periodic background work:

| Thread | Config | Default | What it does |
|---|---|---|---|
| `blob-flush` | `flush_interval_secs` | `0` (off) | Fsync the active segment and save metadata |
| `blob-gc` | `gc_interval_secs` | `0` (off) | Check deleted-ratio, compact one segment if needed |
| `blob-disk-watch` | `disk_check_interval_secs` | `0` (off) | Invoke the low-disk handler when free space drops below `min_free_bytes` |

The threads are independent — a long GC run never blocks fsync.

The disk watchdog never terminates the process by itself. Register a handler with
`Engine::set_low_disk_callback` to decide what happens; call
`Engine::allow_writes_below_free_floor(true)` from that handler if draining during
the shutdown should still be able to write into the reserve.

## Config

| Field | Default | Notes |
|---|---|---|
| `compress_threshold` | 4096 | Bytes; smaller values stored uncompressed |
| `default_codec` | Zstd | Also supports Lz4 |
| `compression_level` | 0 | Zstd compression level |
| `gc_deleted_ratio` | 0.30 | Trigger GC when a sealed segment exceeds this |
| `flush_interval_secs` | 0 | 0 = disabled; ≥ 5 for periodic background fsync |
| `gc_interval_secs` | 0 | 0 = disabled; ≥ 10 for periodic background GC |
| `min_free_bytes` | 0 | Free-space floor. Appends that would breach it are refused before writing; 0 = off |
| `disk_check_interval_secs` | 0 | 0 = disabled; ≥ 5. Call `set_low_disk_callback`'s handler when free space falls below `min_free_bytes` |

## Basic usage

```rust
use bichon_blob::{Codec, Config, Engine};

let engine = Engine::open(path, Config::default())?;

// Store
let hash = blake3::hash(b"email body").into();
engine.put(hash, b"email body", Codec::Zstd)?;

// Retrieve
let value = engine.get(&hash)?; // Some(Vec<u8>) or None

// Delete (caller must track refcounts)
engine.delete(&hash)?;

// Batch operations
engine.put_batch(&[(hash1, data1, Codec::Zstd), (hash2, data2, Codec::Lz4)])?;
engine.delete_batch(&[hash1, hash2])?;

// GC
engine.gc_if_needed()?;

// Clean shutdown
engine.shutdown()?;
```
