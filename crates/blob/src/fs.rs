use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use crate::error::Result;

/// Max retries for transient filesystem errors (NFS ESTALE, CIFS sharing violations, etc.)
const MAX_RETRIES: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_millis(20);

/// Check if an I/O error is transient (retryable).
fn is_transient(err: &io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        err.kind(),
        ErrorKind::TimedOut
            | ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::UnexpectedEof
    ) || err.raw_os_error() == Some(116) // ESTALE on Linux
}

/// Open an existing file for reading, with retry on transient errors (NFS ESTALE etc.).
pub fn open_read(path: &Path) -> Result<File> {
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match File::open(path) {
            Ok(f) => return Ok(f),
            Err(e) if is_transient(&e) => {
                last_err = Some(e);
                if attempt > 0 {
                    std::thread::sleep(RETRY_DELAY * attempt);
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(crate::error::Error::Io(last_err.unwrap()))
}

/// Open an existing file for writing, with retry on transient errors.
pub fn open_write(path: &Path) -> Result<File> {
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match OpenOptions::new().write(true).open(path) {
            Ok(f) => return Ok(f),
            Err(e) if is_transient(&e) => {
                last_err = Some(e);
                if attempt > 0 {
                    std::thread::sleep(RETRY_DELAY * attempt);
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(crate::error::Error::Io(last_err.unwrap()))
}

/// Create a new file atomically: write content to a temp file, fsync, then rename.
/// Avoids `create_new(true)` which is racy on NFS.
pub fn create_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let tmp = path.with_extension(
        path.extension()
            .map(|e| format!("{}.tmp", e.to_string_lossy()))
            .unwrap_or_else(|| "tmp".to_string()),
    );

    {
        let mut f = File::create(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
    }

    fs::rename(&tmp, path)?;
    Ok(())
}

/// Truncate an existing file to the given size, with retry.
pub fn truncate(path: &Path, size: u64) -> Result<()> {
    let f = open_write(path)?;
    f.set_len(size)?;
    f.sync_all()?;
    Ok(())
}

/// Positional read: read `buf.len()` bytes at `offset` from `file`.
/// Uses platform-specific pread so `&File` (shared ref) suffices —
/// no Mutex needed for concurrent reads.
pub fn pread_exact(file: &File, offset: u64, buf: &mut [u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        // `seek_read` may return a short read; keep reading until the buffer
        // is full so this matches `read_exact_at` on Unix.
        let mut filled = 0;
        while filled < buf.len() {
            match file.seek_read(&mut buf[filled..], offset + filled as u64)? {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "failed to fill whole buffer",
                    )
                    .into())
                }
                n => filled += n,
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        // Fallback: seek+read (requires &mut, so this is best-effort on exotic platforms)
        use std::io::{Read, Seek, SeekFrom};
        let mut tmp = file.try_clone()?;
        tmp.seek(SeekFrom::Start(offset))?;
        tmp.read_exact(buf)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_open_read_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"hello").unwrap();

        let mut f = open_read(&path).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut f, &mut s).unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_open_read_missing() {
        let dir = TempDir::new().unwrap();
        let result = open_read(&dir.path().join("nope.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_create_atomic_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        create_atomic(&path, b"hello world").unwrap();

        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"hello world");
        // Temp file should not exist
        assert!(!dir.path().join("data.bin.tmp").exists());
    }

    #[test]
    fn test_create_atomic_overwrites() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        create_atomic(&path, b"first").unwrap();
        create_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn test_pread_exact_short_read_at_eof() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"1234567890").unwrap();
        let file = File::open(&path).unwrap();

        let mut full = [0u8; 10];
        pread_exact(&file, 0, &mut full).unwrap();
        assert_eq!(&full, b"1234567890");

        let mut past_end = [0u8; 20];
        let err = pread_exact(&file, 0, &mut past_end).unwrap_err();
        match err {
            crate::error::Error::Io(e) => {
                assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
            }
            other => panic!("expected UnexpectedEof, got {:?}", other),
        }
    }

    #[test]
    fn test_truncate() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("trunc.bin");
        std::fs::write(&path, b"1234567890").unwrap();
        truncate(&path, 5).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
    }
}
