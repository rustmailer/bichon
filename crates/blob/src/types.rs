use serde::{Deserialize, Serialize};

/// Magic number for entry identification
pub const ENTRY_MAGIC: u32 = 0xB3DB_0001;

/// Fixed header size: magic(4) + crc32(4) + flags(1) + codec(1) + key(32) + raw_size(4) + data_size(4)
pub const ENTRY_HEADER_SIZE: usize = 50;

/// Index record size: key(32) + segment_id(4) + offset(8) + data_size(4) + flags(1) + _pad(3) + crc32(4)
pub const INDEX_RECORD_SIZE: usize = 56;

/// Maximum segment size (1 GB)
pub const SEGMENT_MAX_SIZE: u64 = 1024 * 1024 * 1024;

/// Maximum value size (100 MB)
pub const MAX_VALUE_SIZE: usize = 100 * 1024 * 1024;

/// Default compression threshold (4 KB)
pub const DEFAULT_COMPRESS_THRESHOLD: usize = 4096;

/// Default GC deleted ratio threshold
pub const DEFAULT_GC_DELETED_RATIO: f64 = 0.30;

/// Default GC interval in seconds (5 minutes)
pub const DEFAULT_GC_INTERVAL_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    None = 0,
    Zstd = 1,
    Lz4 = 2,
}

impl Codec {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Codec::None),
            1 => Some(Codec::Zstd),
            2 => Some(Codec::Lz4),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub compress_threshold: usize,
    pub default_codec: Codec,
    pub compression_level: i32,
    pub gc_deleted_ratio: f64,
    /// Interval in seconds for periodic background flush (0 = disabled).
    /// When set, a background thread fsyncs the active segment and saves
    /// metadata at this interval, bounding recovery time after a crash.
    pub flush_interval_secs: u64,
    /// Interval in seconds for periodic background GC (0 = disabled).
    /// When set, a background thread checks whether any sealed segment
    /// exceeds the deleted-ratio threshold and compacts it if needed.
    pub gc_interval_secs: u64,
    /// Free-space floor in bytes on the store volume (0 = no floor).
    /// Appends that would leave less than this much free space are rejected
    /// with [`crate::error::Error::DiskFull`] *before* any byte is written, so
    /// a full volume can never leave a partially written entry behind.
    pub min_free_bytes: u64,
    /// Interval in seconds for the disk-space watchdog (0 = disabled).
    /// When the free space on the store volume drops below `min_free_bytes`,
    /// the watchdog invokes the handler registered with
    /// [`crate::Engine::set_low_disk_callback`]; the engine never terminates
    /// the process itself. Call [`crate::Engine::allow_writes_below_free_floor`]
    /// from that handler if the shutdown drain should still be able to write.
    pub disk_check_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            compress_threshold: DEFAULT_COMPRESS_THRESHOLD,
            default_codec: Codec::Zstd,
            compression_level: 0,
            gc_deleted_ratio: DEFAULT_GC_DELETED_RATIO,
            flush_interval_secs: 0,
            gc_interval_secs: 0,
            min_free_bytes: 0,
            disk_check_interval_secs: 0,
        }
    }
}

impl Config {
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.gc_deleted_ratio <= 0.0 || self.gc_deleted_ratio >= 1.0 {
            return Err(crate::error::Error::InvalidConfig(
                "gc_deleted_ratio must be in (0.0, 1.0)".into(),
            ));
        }
        if self.compression_level < 0 {
            return Err(crate::error::Error::InvalidConfig(
                "compression_level must be >= 0".into(),
            ));
        }
        if self.flush_interval_secs > 0 && self.flush_interval_secs < 5 {
            return Err(crate::error::Error::InvalidConfig(
                "flush_interval_secs must be 0 (disabled) or >= 5".into(),
            ));
        }
        if self.gc_interval_secs > 0 && self.gc_interval_secs < 10 {
            return Err(crate::error::Error::InvalidConfig(
                "gc_interval_secs must be 0 (disabled) or >= 10".into(),
            ));
        }
        if self.disk_check_interval_secs > 0 && self.disk_check_interval_secs < 5 {
            return Err(crate::error::Error::InvalidConfig(
                "disk_check_interval_secs must be 0 (disabled) or >= 5".into(),
            ));
        }
        Ok(())
    }
}
