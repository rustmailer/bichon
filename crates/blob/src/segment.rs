use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::checksum;
use crate::error::{Error, Result};
use crate::fs as fs_util;
use crate::types::{Codec, ENTRY_HEADER_SIZE, ENTRY_MAGIC, SEGMENT_MAX_SIZE};

/// In-memory representation of a stored entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub flags: u8,
    pub codec: Codec,
    pub key: [u8; 32],
    pub raw_size: u32,
    pub data: Vec<u8>,
}

impl Entry {
    /// Create a normal data entry.
    /// `raw_size` is the original uncompressed size; `data` is what goes to disk.
    pub fn new(key: [u8; 32], data: &[u8], raw_size: u32, flags: u8, codec: Codec) -> Self {
        Self {
            flags,
            codec,
            key,
            raw_size,
            data: data.to_vec(),
        }
    }

    /// Create a tombstone entry.
    pub fn tombstone(key: [u8; 32]) -> Self {
        Self {
            flags: 1,
            codec: Codec::None,
            key,
            raw_size: 0,
            data: Vec::new(),
        }
    }

    pub fn is_tombstone(&self) -> bool {
        self.flags == 1
    }

    /// Total on-disk size: header + data
    pub fn disk_size(&self) -> usize {
        ENTRY_HEADER_SIZE + self.data.len()
    }
}

/// Write entries sequentially to a segment file.
pub struct SegmentWriter {
    file: File,
    path: PathBuf,
    id: u32,
    bytes_written: u64,
}

impl SegmentWriter {
    pub fn create(path: PathBuf, id: u32) -> Result<Self> {
        // Use create+truncate instead of create_new to avoid NFS O_EXCL issues.
        let file = File::create(&path)?;
        Ok(Self {
            file,
            path,
            id,
            bytes_written: 0,
        })
    }

    pub fn open_append(path: PathBuf, id: u32) -> Result<Self> {
        let mut file = fs_util::open_write(&path)?;
        file.seek(SeekFrom::End(0))?;
        let bytes_written = file.stream_position()?;
        Ok(Self {
            file,
            path,
            id,
            bytes_written,
        })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn is_full(&self) -> bool {
        self.bytes_written >= SEGMENT_MAX_SIZE
    }

    /// Append an entry. Returns the offset where it was written.
    pub fn append(&mut self, entry: &Entry) -> Result<u64> {
        let offset = self.bytes_written;
        if let Err(e) = self.write_entry(entry) {
            let e = map_io_err(e, &self.path);
            // A failed write (ENOSPC, I/O error) can leave a partial entry at
            // the tail. Cut it back to the last complete entry so the segment
            // stays clean and a later scan doesn't have to repair it.
            if let Err(rb) = self.rollback_to(offset) {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %rb,
                    "failed to roll back a partial segment write"
                );
            }
            return Err(e);
        }
        Ok(offset)
    }

    /// Truncate the segment back to `len` and continue appending from there.
    fn rollback_to(&mut self, len: u64) -> std::io::Result<()> {
        self.file.set_len(len)?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    fn write_entry(&mut self, entry: &Entry) -> Result<()> {
        let data_size = entry.data.len() as u32;

        // Write magic
        self.file.write_all(&ENTRY_MAGIC.to_le_bytes())?;

        // CRC32 placeholder: write zeros, remember position
        let crc_pos = self.file.stream_position()?;
        self.file.write_all(&0u32.to_le_bytes())?;

        // Write flags, codec, key, raw_size, data_size
        self.file.write_all(&[entry.flags])?;
        self.file.write_all(&[entry.codec as u8])?;
        self.file.write_all(&entry.key)?;
        self.file.write_all(&entry.raw_size.to_le_bytes())?;
        self.file.write_all(&data_size.to_le_bytes())?;

        // Write data
        self.file.write_all(&entry.data)?;

        // Calculate CRC32 over everything after the crc32 field
        let crc = {
            let mut hasher = checksum::CrcWriter::new();
            hasher.update(&[entry.flags]);
            hasher.update(&[entry.codec as u8]);
            hasher.update(&entry.key);
            hasher.update(&entry.raw_size.to_le_bytes());
            hasher.update(&data_size.to_le_bytes());
            hasher.update(&entry.data);
            hasher.finalize()
        };

        // Seek back and write the real CRC32
        self.file.seek(SeekFrom::Start(crc_pos))?;
        self.file.write_all(&crc.to_le_bytes())?;

        // Seek back to end
        self.file.seek(SeekFrom::End(0))?;

        self.bytes_written += entry.disk_size() as u64;
        Ok(())
    }

    pub fn fsync(&self) -> Result<()> {
        self.file.sync_all().map_err(|e| {
            if e.kind() == std::io::ErrorKind::StorageFull {
                Error::DiskFull(format!("{}: {}", self.path.display(), e))
            } else {
                Error::Io(e)
            }
        })?;
        Ok(())
    }
}

/// Read entries from a segment file.
pub struct SegmentReader {
    path: PathBuf,
    id: u32,
}

impl SegmentReader {
    pub fn open(path: PathBuf, id: u32) -> Result<Self> {
        Ok(Self { path, id })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_size(&self) -> Result<u64> {
        Ok(fs::metadata(&self.path)?.len())
    }

    /// Read a single entry at the given offset. Returns the entry and the offset of the next entry.
    pub fn read_entry_at(&self, offset: u64) -> Result<(Entry, u64)> {
        let mut file = fs_util::open_read(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;

        // Read magic
        let mut magic_buf = [0u8; 4];
        file.read_exact(&mut magic_buf)?;
        let magic = u32::from_le_bytes(magic_buf);
        if magic != ENTRY_MAGIC {
            return Err(Error::CorruptEntry {
                path: self.path.clone(),
                offset,
                reason: format!("bad magic: 0x{:08X}", magic),
            });
        }

        // Read CRC32
        let mut crc_buf = [0u8; 4];
        file.read_exact(&mut crc_buf)?;
        let stored_crc = u32::from_le_bytes(crc_buf);

        // Read flags, codec
        let mut flags_buf = [0u8; 1];
        file.read_exact(&mut flags_buf)?;
        let flags = flags_buf[0];

        let mut codec_buf = [0u8; 1];
        file.read_exact(&mut codec_buf)?;
        let codec = Codec::from_u8(codec_buf[0]).ok_or_else(|| Error::CorruptEntry {
            path: self.path.clone(),
            offset,
            reason: format!("unknown codec: {}", codec_buf[0]),
        })?;

        // Read key, raw_size, data_size
        let mut key = [0u8; 32];
        file.read_exact(&mut key)?;

        let mut raw_size_buf = [0u8; 4];
        file.read_exact(&mut raw_size_buf)?;
        let raw_size = u32::from_le_bytes(raw_size_buf);

        let mut data_size_buf = [0u8; 4];
        file.read_exact(&mut data_size_buf)?;
        let data_size = u32::from_le_bytes(data_size_buf);

        if data_size as usize > crate::types::MAX_VALUE_SIZE {
            return Err(Error::CorruptEntry {
                path: self.path.clone(),
                offset,
                reason: format!("data_size {} exceeds max {}", data_size, crate::types::MAX_VALUE_SIZE),
            });
        }

        // Read data
        let mut data = vec![0u8; data_size as usize];
        file.read_exact(&mut data)?;

        // Verify CRC32 (over everything after the crc32 field)
        let computed_crc = {
            let mut hasher = checksum::CrcWriter::new();
            hasher.update(&[flags]);
            hasher.update(&[codec as u8]);
            hasher.update(&key);
            hasher.update(&raw_size.to_le_bytes());
            hasher.update(&data_size.to_le_bytes());
            hasher.update(&data);
            hasher.finalize()
        };

        if stored_crc != computed_crc {
            return Err(Error::CrcMismatch {
                path: self.path.clone(),
                offset,
            });
        }

        let next_offset = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;

        Ok((
            Entry {
                flags,
                codec,
                key,
                raw_size,
                data,
            },
            next_offset,
        ))
    }

    /// Read a single entry at the given offset using a pre-opened File.
    /// Uses pread so concurrent reads on the same segment don't block each other.
    pub fn read_entry_at_file(&self, offset: u64, file: &File) -> Result<(Entry, u64)> {
        // Read header (50 bytes)
        let mut header = [0u8; ENTRY_HEADER_SIZE];
        fs_util::pread_exact(file, offset, &mut header)?;

        let mut pos = 0;

        // Magic
        let magic = u32::from_le_bytes(header[pos..pos+4].try_into().unwrap());
        pos += 4;
        if magic != ENTRY_MAGIC {
            return Err(Error::CorruptEntry {
                path: self.path.clone(),
                offset,
                reason: format!("bad magic: 0x{:08X}", magic),
            });
        }

        // CRC32
        let stored_crc = u32::from_le_bytes(header[pos..pos+4].try_into().unwrap());
        pos += 4;

        // Flags, codec
        let flags = header[pos];
        pos += 1;
        let codec = Codec::from_u8(header[pos]).ok_or_else(|| Error::CorruptEntry {
            path: self.path.clone(),
            offset,
            reason: format!("unknown codec: {}", header[pos]),
        })?;
        pos += 1;

        // Key
        let mut key = [0u8; 32];
        key.copy_from_slice(&header[pos..pos+32]);
        pos += 32;

        // Raw size, data size
        let raw_size = u32::from_le_bytes(header[pos..pos+4].try_into().unwrap());
        pos += 4;
        let data_size = u32::from_le_bytes(header[pos..pos+4].try_into().unwrap());

        // Defense against header corruption: refuse absurdly large allocations
        if data_size as usize > crate::types::MAX_VALUE_SIZE {
            return Err(Error::CorruptEntry {
                path: self.path.clone(),
                offset,
                reason: format!("data_size {} exceeds max {}", data_size, crate::types::MAX_VALUE_SIZE),
            });
        }

        // Read data
        let data_offset = offset + ENTRY_HEADER_SIZE as u64;
        let mut data = vec![0u8; data_size as usize];
        fs_util::pread_exact(file, data_offset, &mut data)?;

        // Verify CRC32 (over everything after the crc32 field: flags+codec+key+raw_size+data_size+data)
        let computed_crc = {
            let mut hasher = crate::checksum::CrcWriter::new();
            hasher.update(&[flags]);
            hasher.update(&[codec as u8]);
            hasher.update(&key);
            hasher.update(&raw_size.to_le_bytes());
            hasher.update(&data_size.to_le_bytes());
            hasher.update(&data);
            hasher.finalize()
        };

        if stored_crc != computed_crc {
            return Err(Error::CrcMismatch {
                path: self.path.clone(),
                offset,
            });
        }

        let next_offset = offset + ENTRY_HEADER_SIZE as u64 + data_size as u64;

        Ok((
            Entry {
                flags,
                codec,
                key,
                raw_size,
                data,
            },
            next_offset,
        ))
    }

    /// Read data portion of an entry (for pread-style reads when you already know offset + data_size).
    pub fn read_data(&self, offset: u64, data_size: u32) -> Result<Vec<u8>> {
        let mut file = fs_util::open_read(&self.path)?;
        // Skip magic(4) + crc32(4) + flags(1) + codec(1) + key(32) + raw_size(4) + data_size(4) = 50 bytes
        let data_start = offset + ENTRY_HEADER_SIZE as u64;
        file.seek(SeekFrom::Start(data_start))?;
        let mut buf = vec![0u8; data_size as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Read the full entry header + data for verification (used by recovery and GC).
    pub fn read_full_entry(&self, offset: u64, data_size: u32) -> Result<Vec<u8>> {
        let mut file = fs_util::open_read(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let total = ENTRY_HEADER_SIZE + data_size as usize;
        let mut buf = vec![0u8; total];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Iterate over all valid entries in the segment, calling f for each.
    /// Stops when hitting a corrupt/incomplete entry at the tail.
    pub fn scan_entries<F>(&self, start_offset: u64, mut f: F) -> Result<u64>
    where
        F: FnMut(&Entry, u64) -> Result<()>,
    {
        let file_size = self.file_size()?;
        let mut offset = start_offset;

        while offset + ENTRY_HEADER_SIZE as u64 <= file_size {
            match self.read_entry_at(offset) {
                Ok((entry, next)) => {
                    f(&entry, offset)?;
                    offset = next;
                }
                Err(Error::CrcMismatch { .. }) | Err(Error::CorruptEntry { .. }) => {
                    // If near end of file (within one max entry), truncate
                    if file_size - offset < ENTRY_HEADER_SIZE as u64 + 100 * 1024 * 1024 {
                        // Likely a partial write at tail, stop here
                        break;
                    } else {
                        return Err(Error::CorruptEntry {
                            path: self.path.clone(),
                            offset,
                            reason: "mid-file corruption detected".into(),
                        });
                    }
                }
                Err(Error::Io(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // The file ends inside this entry: a torn write at the
                    // tail (typically the volume filled up mid-append). An
                    // entry is at most MAX_VALUE_SIZE + one header, so this
                    // can only be the last entry; recover exactly like a bad
                    // tail CRC — stop the scan and let the caller truncate or
                    // compact away the partial entry.
                    tracing::warn!(
                        path = %self.path.display(),
                        offset,
                        file_size,
                        "truncated tail entry; stopping scan"
                    );
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(offset) // return the truncation point
    }
}

/// Truncate a segment file to the given size.
pub fn truncate_segment(path: &Path, size: u64) -> Result<()> {
    fs_util::truncate(path, size)
}

/// Map an Error, converting Io(StorageFull) to DiskFull with path context.
fn map_io_err(e: Error, path: &Path) -> Error {
    match e {
        Error::Io(io) if io.kind() == std::io::ErrorKind::StorageFull => {
            Error::DiskFull(format!("{}: {}", path.display(), io))
        }
        _ => e,
    }
}

/// Segment file name from id: "00000001.seg"
pub fn segment_filename(id: u32) -> String {
    format!("{:08}.seg", id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_segment_path(dir: &TempDir, id: u32) -> PathBuf {
        dir.path().join(segment_filename(id))
    }

    #[test]
    fn test_write_and_read_entry() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        let key = [0xAAu8; 32];
        let data = b"hello world".to_vec();

        let entry = Entry::new(key, &data, data.len() as u32, 0, Codec::None);
        {
            let mut writer = SegmentWriter::create(path.clone(), 1).unwrap();
            writer.append(&entry).unwrap();
            writer.fsync().unwrap();
        }

        let reader = SegmentReader::open(path, 1).unwrap();
        let (read_entry, next) = reader.read_entry_at(0).unwrap();

        assert_eq!(read_entry.key, key);
        assert_eq!(read_entry.data, data);
        assert_eq!(read_entry.flags, 0);
        assert_eq!(read_entry.raw_size, 11);
        assert!(next > 0);
    }

    #[test]
    fn test_tombstone_entry() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        let key = [0xBBu8; 32];

        let entry = Entry::tombstone(key);
        {
            let mut writer = SegmentWriter::create(path.clone(), 1).unwrap();
            writer.append(&entry).unwrap();
            writer.fsync().unwrap();
        }

        let reader = SegmentReader::open(path, 1).unwrap();
        let (read_entry, _) = reader.read_entry_at(0).unwrap();

        assert!(read_entry.is_tombstone());
        assert_eq!(read_entry.data.len(), 0);
    }

    #[test]
    fn test_multiple_entries() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);

        let entries: Vec<_> = (0..10)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = i;
                Entry::new(key, &vec![i; 100], 100, 0, Codec::None)
            })
            .collect();

        {
            let mut writer = SegmentWriter::create(path.clone(), 1).unwrap();
            for e in &entries {
                writer.append(e).unwrap();
            }
            writer.fsync().unwrap();
        }

        let reader = SegmentReader::open(path, 1).unwrap();
        let mut offset = 0u64;
        for (i, expected) in entries.iter().enumerate() {
            let (entry, next) = reader.read_entry_at(offset).unwrap();
            assert_eq!(entry.key[0], i as u8);
            assert_eq!(entry.data, expected.data);
            offset = next;
        }
    }

    #[test]
    fn test_bad_magic_detected() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        // Write garbage
        std::fs::write(&path, vec![0xFFu8; 100]).unwrap();

        let reader = SegmentReader::open(path, 1).unwrap();
        let result = reader.read_entry_at(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_entries_stops_at_truncated_tail() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        let first = Entry::new([0x11u8; 32], b"first", 5, 0, Codec::None);
        let second = Entry::new([0x22u8; 32], b"second-entry-data", 17, 0, Codec::None);

        let second_offset = {
            let mut writer = SegmentWriter::create(path.clone(), 1).unwrap();
            writer.append(&first).unwrap();
            let offset = writer.append(&second).unwrap();
            writer.fsync().unwrap();
            offset
        };

        // Simulate running out of disk mid-append: the 50-byte header of the
        // second entry is complete, but its data is only partially written.
        let truncated = second_offset + ENTRY_HEADER_SIZE as u64 + 4;
        fs_util::truncate(&path, truncated).unwrap();

        let reader = SegmentReader::open(path, 1).unwrap();
        let mut seen = Vec::new();
        let end = reader
            .scan_entries(0, |entry, offset| {
                seen.push((offset, entry.key[0]));
                Ok(())
            })
            .expect("a truncated tail must not fail the scan");

        assert_eq!(seen, vec![(0, 0x11)]);
        assert_eq!(end, second_offset);
    }

    #[test]
    fn test_rollback_removes_partial_tail() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        let entry = Entry::new([0x33u8; 32], b"complete", 8, 0, Codec::None);

        let mut writer = SegmentWriter::create(path.clone(), 1).unwrap();
        writer.append(&entry).unwrap();
        let good_len = writer.bytes_written();

        // Simulate a write that died mid-entry.
        writer.file.write_all(&[0xABu8; 30]).unwrap();
        writer.rollback_to(good_len).unwrap();

        assert_eq!(fs::metadata(&path).unwrap().len(), good_len);
        assert_eq!(writer.bytes_written(), good_len);

        // The writer must stay usable at the same offset.
        writer.append(&entry).unwrap();
        assert_eq!(writer.bytes_written(), good_len * 2);
    }

    #[test]
    fn test_is_full() {
        let dir = TempDir::new().unwrap();
        let path = temp_segment_path(&dir, 1);
        let writer = SegmentWriter::create(path, 1).unwrap();
        assert!(!writer.is_full());
    }
}
