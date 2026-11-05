use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};
use redb::ReadableDatabase;

use crate::error::Result;
use crate::types::INDEX_RECORD_SIZE;

// ── IndexRecord ──────────────────────────────────────────────────────────────

/// On-disk format: 52 bytes per record + 4 bytes CRC = 56 bytes total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecord {
    pub key: [u8; 32],
    pub segment_id: u32,
    pub offset: u64,
    pub data_size: u32,
    pub flags: u8,
}

impl IndexRecord {
    pub fn new(key: [u8; 32], segment_id: u32, offset: u64, data_size: u32, flags: u8) -> Self {
        Self {
            key,
            segment_id,
            offset,
            data_size,
            flags,
        }
    }

    pub fn is_tombstone(&self) -> bool {
        self.flags == 1
    }

    pub fn encode(&self) -> [u8; INDEX_RECORD_SIZE] {
        let mut buf = [0u8; INDEX_RECORD_SIZE];
        buf[0..32].copy_from_slice(&self.key);
        buf[32..36].copy_from_slice(&self.segment_id.to_le_bytes());
        buf[36..44].copy_from_slice(&self.offset.to_le_bytes());
        buf[44..48].copy_from_slice(&self.data_size.to_le_bytes());
        buf[48] = self.flags;
        let crc = crate::checksum::crc32(&buf[..52]);
        buf[52..56].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8; INDEX_RECORD_SIZE]) -> crate::error::Result<Self> {
        let stored_crc = u32::from_le_bytes(buf[52..56].try_into().unwrap());
        let computed = crate::checksum::crc32(&buf[..52]);
        if stored_crc != computed {
            return Err(crate::error::Error::BucketIndexCorrupt {
                path: std::path::PathBuf::new(),
                reason: format!(
                    "CRC mismatch: stored=0x{:08X} computed=0x{:08X}",
                    stored_crc, computed
                ),
            });
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&buf[0..32]);
        let segment_id = u32::from_le_bytes(buf[32..36].try_into().unwrap());
        let offset = u64::from_le_bytes(buf[36..44].try_into().unwrap());
        let data_size = u32::from_le_bytes(buf[44..48].try_into().unwrap());
        let flags = buf[48];
        Ok(Self {
            key,
            segment_id,
            offset,
            data_size,
            flags,
        })
    }
}

// ── redb Value impl for fixed-size record bytes ──────────────────────────────

/// Newtype wrapper so we can implement `redb::Value` for `[u8; INDEX_RECORD_SIZE]`.
#[derive(Debug, Clone, Copy)]
struct RecordBytes([u8; INDEX_RECORD_SIZE]);

impl redb::Value for RecordBytes {
    type SelfType<'a> = RecordBytes;
    type AsBytes<'a> = [u8; INDEX_RECORD_SIZE];

    fn fixed_width() -> Option<usize> {
        Some(INDEX_RECORD_SIZE)
    }

    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a
    {
        let mut arr = [0u8; INDEX_RECORD_SIZE];
        arr.copy_from_slice(data);
        RecordBytes(arr)
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a> {
        value.0
    }

    fn type_name() -> redb::TypeName {
        redb::TypeName::new("IndexRecord")
    }
}

// ── IndexStore ───────────────────────────────────────────────────────────────

const INDEX_TABLE: TableDefinition<[u8; 32], RecordBytes> = TableDefinition::new("blob_index");

/// Zero-heap key-value index backed by redb.
///
/// B-tree + mmap + ACID.  No manual compact / sort / dedup / per-bucket mmap
/// management.  Startup is O(1) — redb reads only its root page.
pub struct IndexStore {
    db: Database,
    path: std::path::PathBuf,
}

impl IndexStore {
    /// Open (or create) the index database at `dir/index.redb`.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("index.redb");
        let db = Database::create(&path)
            .map_err(|e| crate::error::Error::IndexDb(format!("failed to create index: {}", e)))?;
        // Ensure the table exists so reads on a fresh database don't fail.
        {
            let txn = db
                .begin_write()
                .map_err(|e| crate::error::Error::IndexDb(format!("init write txn: {}", e)))?;
            txn.open_table(INDEX_TABLE)
                .map_err(|e| crate::error::Error::IndexDb(format!("init table: {}", e)))?;
            txn.commit()
                .map_err(|e| crate::error::Error::IndexDb(format!("init commit: {}", e)))?;
        }
        Ok(Self { db, path })
    }

    /// Look up a key. Returns the latest IndexRecord, or None if absent/tombstone.
    pub fn get(&self, key: &[u8; 32]) -> Result<Option<IndexRecord>> {
        Ok(self.get_raw(key)?.filter(|record| !record.is_tombstone()))
    }

    /// Like [`get`], but also returns tombstone records.
    ///
    /// Recovery uses this to compare recency: a tombstone in a newer segment
    /// must suppress an older data record surfaced by a rescan, even though
    /// [`get`] reports the key as absent.
    pub fn get_raw(&self, key: &[u8; 32]) -> Result<Option<IndexRecord>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| crate::error::Error::IndexDb(format!("read txn: {}", e)))?;
        let table = txn
            .open_table(INDEX_TABLE)
            .map_err(|e| crate::error::Error::IndexDb(format!("open table: {}", e)))?;

        match table
            .get(key)
            .map_err(|e| crate::error::Error::IndexDb(format!("get: {}", e)))?
        {
            Some(guard) => Ok(Some(IndexRecord::decode(&guard.value().0)?)),
            None => Ok(None),
        }
    }

    /// Check whether a key exists (non-tombstone) in the store.
    pub fn exists(&self, key: &[u8; 32]) -> Result<bool> {
        self.get(key).map(|r| r.is_some())
    }

    /// Insert or update a record for a key.  Committed in a single write txn.
    pub fn insert(&self, record: &IndexRecord) -> Result<()> {
        let txn = self
            .db
            .begin_write()
            .map_err(|e| crate::error::Error::IndexDb(format!("write txn: {}", e)))?;
        {
            let mut table = txn
                .open_table(INDEX_TABLE)
                .map_err(|e| crate::error::Error::IndexDb(format!("open table: {}", e)))?;
            table
                .insert(&record.key, RecordBytes(record.encode()))
                .map_err(|e| crate::error::Error::IndexDb(format!("insert: {}", e)))?;
        }
        txn.commit()
            .map_err(|e| crate::error::Error::IndexDb(format!("commit: {}", e)))?;
        Ok(())
    }

    /// Batch insert multiple records in a single write transaction.
    pub fn insert_batch(&self, records: &[IndexRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let txn = self
            .db
            .begin_write()
            .map_err(|e| crate::error::Error::IndexDb(format!("write txn: {}", e)))?;
        {
            let mut table = txn
                .open_table(INDEX_TABLE)
                .map_err(|e| crate::error::Error::IndexDb(format!("open table: {}", e)))?;
            for record in records {
                table
                    .insert(&record.key, RecordBytes(record.encode()))
                    .map_err(|e| crate::error::Error::IndexDb(format!("insert: {}", e)))?;
            }
        }
        txn.commit()
            .map_err(|e| crate::error::Error::IndexDb(format!("commit: {}", e)))?;
        Ok(())
    }

    /// Remove keys from the index in a single write transaction.
    pub fn delete_batch(&self, keys: &[[u8; 32]]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let txn = self
            .db
            .begin_write()
            .map_err(|e| crate::error::Error::IndexDb(format!("write txn: {}", e)))?;
        {
            let mut table = txn
                .open_table(INDEX_TABLE)
                .map_err(|e| crate::error::Error::IndexDb(format!("open table: {}", e)))?;
            for key in keys {
                table
                    .remove(key)
                    .map_err(|e| crate::error::Error::IndexDb(format!("remove: {}", e)))?;
            }
        }
        txn.commit()
            .map_err(|e| crate::error::Error::IndexDb(format!("commit: {}", e)))?;
        Ok(())
    }

    /// Flush all pending index writes to stable storage.
    ///
    /// redb is an mmap'd copy-on-write database with no public flush API: the
    /// OS may still hold dirty pages after a commit. We commit a no-op write
    /// transaction (runs redb's write path so the latest state is reflected in
    /// the mapped file) and then fsync the file itself, which pushes every
    /// mmap-dirtied page to disk. Called by the backup manager inside a write
    /// window so a subsequent external copy of `index.redb` is durable.
    pub fn flush(&self) -> Result<()> {
        {
            let txn = self
                .db
                .begin_write()
                .map_err(|e| crate::error::Error::IndexDb(format!("flush write txn: {}", e)))?;
            txn.open_table(INDEX_TABLE)
                .map_err(|e| crate::error::Error::IndexDb(format!("flush open table: {}", e)))?;
            txn.commit()
                .map_err(|e| crate::error::Error::IndexDb(format!("flush commit: {}", e)))?;
        }
        std::fs::File::options()
            .write(true)
            .open(&self.path)
            .and_then(|f| f.sync_all())
            .map_err(|e| crate::error::Error::IndexDb(format!("flush: {}", e)))
    }

    /// Total number of live (non-tombstone) keys.
    pub fn total_keys(&self) -> Result<usize> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| crate::error::Error::IndexDb(format!("read txn: {}", e)))?;
        let table = txn
            .open_table(INDEX_TABLE)
            .map_err(|e| crate::error::Error::IndexDb(format!("open table: {}", e)))?;

        let mut count = 0usize;
        let iter = table
            .iter()
            .map_err(|e| crate::error::Error::IndexDb(format!("iter: {}", e)))?;
        for item in iter {
            let (_, guard) =
                item.map_err(|e| crate::error::Error::IndexDb(format!("iter next: {}", e)))?;
            let record = IndexRecord::decode(&guard.value().0)?;
            if !record.is_tombstone() {
                count += 1;
            }
        }
        Ok(count)
    }
}
