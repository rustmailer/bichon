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

//! Source-UID tombstones.
//!
//! Deleting a message from the archive is permanent, but gap-fill and full
//! re-syncs discover local holes by diffing the remote UID list against the
//! local envelopes — a deleted message looks exactly like a failed download,
//! so without a record it would be fetched again on the next repair run.
//!
//! A tombstone records, per source mailbox, the set of remote UIDs that were
//! deliberately deleted locally. `gap_fill` and the full re-sync paths remove
//! those UIDs from their download candidates.
//!
//! **Storage.** One compressed bitmap per mailbox under
//! `<root>/tombstones/<account_id>/<mailbox_id>`. The file is self-describing
//! (magic, version, ids, `uid_validity`, CRC32) because IMAP UIDs are only
//! meaningful for one `UIDVALIDITY` generation: when the mailbox is rebuilt
//! the old bitmap is ignored and dropped.
//!
//! **Durability.** Writes are atomic (temp file + fsync + rename) and happen
//! *before* the messages are deleted, so a crash can leave a tombstone for a
//! message that still exists (harmless) but never a committed deletion without
//! one. Writes take the backup write gate like every other content write.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use roaring::RoaringBitmap;
use tracing::{info, warn};

use crate::backup::gate::{WRITE_GATE, BACKUP_ACQUIRE_TIMEOUT};
use crate::error::{code::ErrorCode, BichonResult};
use crate::raise_error;
use crate::settings::dir::DATA_DIR_MANAGER;

const MAGIC: &[u8; 8] = b"BCHNTOMB";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 8 + 2 + 8 + 8 + 4 + 4;
const CRC_LEN: usize = 4;
/// Refuse absurd files instead of trying to allocate whatever a corrupt
/// length field claims.
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Serializes tombstone read-modify-write cycles across the process. Writes
/// are rare (one per deletion batch per mailbox), so a single lock is simpler
/// than a per-mailbox map and cannot deadlock.
static WRITE_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone, Debug)]
pub struct Tombstones {
    pub account_id: u64,
    pub mailbox_id: u64,
    pub uid_validity: u32,
    pub bitmap: RoaringBitmap,
}

fn root_dir() -> PathBuf {
    DATA_DIR_MANAGER.tombstones_dir.clone()
}

fn file_path(account_id: u64, mailbox_id: u64) -> PathBuf {
    root_dir()
        .join(account_id.to_string())
        .join(mailbox_id.to_string())
}

fn corrupt(path: &Path, reason: impl std::fmt::Display) {
    warn!(
        path = %path.display(),
        reason = %reason,
        "tombstone file is unreadable; ignoring it (gap-fill may re-download messages it covered)"
    );
}

fn decode(path: &Path, bytes: &[u8]) -> Option<Tombstones> {
    if bytes.len() < HEADER_LEN + CRC_LEN {
        corrupt(path, "file too short");
        return None;
    }
    if &bytes[..8] != MAGIC {
        corrupt(path, "bad magic");
        return None;
    }
    let version = u16::from_le_bytes(bytes[8..10].try_into().ok()?);
    if version != VERSION {
        corrupt(path, format!("unsupported version {version}"));
        return None;
    }
    let account_id = u64::from_le_bytes(bytes[10..18].try_into().ok()?);
    let mailbox_id = u64::from_le_bytes(bytes[18..26].try_into().ok()?);
    let uid_validity = u32::from_le_bytes(bytes[26..30].try_into().ok()?);
    let payload_len = u32::from_le_bytes(bytes[30..34].try_into().ok()?) as usize;
    if payload_len > MAX_FILE_BYTES || bytes.len() != HEADER_LEN + payload_len + CRC_LEN {
        corrupt(path, "length mismatch");
        return None;
    }
    let payload = &bytes[HEADER_LEN..HEADER_LEN + payload_len];
    let expected_crc =
        u32::from_le_bytes(bytes[HEADER_LEN + payload_len..].try_into().ok()?);
    let actual_crc = crc32fast::hash(payload);
    if expected_crc != actual_crc {
        corrupt(path, "crc mismatch");
        return None;
    }
    let bitmap = match RoaringBitmap::deserialize_from(payload) {
        Ok(bitmap) => bitmap,
        Err(e) => {
            corrupt(path, format!("bitmap decode failed: {e}"));
            return None;
        }
    };
    Some(Tombstones {
        account_id,
        mailbox_id,
        uid_validity,
        bitmap,
    })
}

/// Read the tombstone file for a mailbox, if it exists and is valid. The
/// caller decides whether the embedded `uid_validity` still applies.
pub fn load(account_id: u64, mailbox_id: u64) -> BichonResult<Option<Tombstones>> {
    load_at(&file_path(account_id, mailbox_id))
}

fn load_at(path: &Path) -> BichonResult<Option<Tombstones>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(raise_error!(
                format!("cannot open tombstone file {}: {e}", path.display()),
                ErrorCode::InternalError
            ))
        }
    };
    let metadata = file.metadata().map_err(|e| {
        raise_error!(
            format!("cannot stat tombstone file {}: {e}", path.display()),
            ErrorCode::InternalError
        )
    })?;
    if metadata.len() as usize > MAX_FILE_BYTES + HEADER_LEN + CRC_LEN {
        corrupt(path, "file exceeds the size cap");
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|e| {
        raise_error!(
            format!("cannot read tombstone file {}: {e}", path.display()),
            ErrorCode::InternalError
        )
    })?;
    Ok(decode(path, &bytes))
}

/// Load a tombstone file only when it belongs to the given UIDVALIDITY
/// generation. A stale file is ignored (and left for the caller to drop).
pub fn load_for(
    account_id: u64,
    mailbox_id: u64,
    uid_validity: u32,
) -> BichonResult<Option<Tombstones>> {
    Ok(load(account_id, mailbox_id)?.filter(|t| t.uid_validity == uid_validity))
}

fn encode(tombstones: &Tombstones) -> BichonResult<Vec<u8>> {
    let mut payload = Vec::new();
    tombstones
        .bitmap
        .serialize_into(&mut payload)
        .map_err(|e| {
            raise_error!(
                format!("cannot serialize tombstone bitmap: {e}"),
                ErrorCode::InternalError
            )
        })?;
    let payload_len: u32 = payload
        .len()
        .try_into()
        .map_err(|_| raise_error!("tombstone bitmap is too large".into(), ErrorCode::InternalError))?;
    let crc = crc32fast::hash(&payload);

    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + CRC_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&tombstones.account_id.to_le_bytes());
    out.extend_from_slice(&tombstones.mailbox_id.to_le_bytes());
    out.extend_from_slice(&tombstones.uid_validity.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(handle) = File::open(dir) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// Atomically replace the tombstone file for one mailbox.
fn save(tombstones: &Tombstones) -> BichonResult<()> {
    save_at(&file_path(tombstones.account_id, tombstones.mailbox_id), tombstones)
}

fn save_at(path: &Path, tombstones: &Tombstones) -> BichonResult<()> {
    let dir = path.parent().ok_or_else(|| {
        raise_error!(
            format!("invalid tombstone path {}", path.display()),
            ErrorCode::InternalError
        )
    })?;
    fs::create_dir_all(dir).map_err(|e| {
        raise_error!(
            format!("cannot create tombstone directory {}: {e}", dir.display()),
            ErrorCode::InternalError
        )
    })?;
    let bytes = encode(tombstones)?;
    // A unique suffix keeps two processes (server + a running admin command)
    // from clobbering each other's temp file; the rename below is atomic.
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        tombstones.mailbox_id,
        std::process::id()
    ));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| {
                raise_error!(
                    format!("cannot create tombstone temp file {}: {e}", tmp.display()),
                    ErrorCode::InternalError
                )
            })?;
        file.write_all(&bytes).map_err(|e| {
            raise_error!(
                format!("cannot write tombstone temp file {}: {e}", tmp.display()),
                ErrorCode::InternalError
            )
        })?;
        file.sync_all().map_err(|e| {
            raise_error!(
                format!("cannot fsync tombstone temp file {}: {e}", tmp.display()),
                ErrorCode::InternalError
            )
        })?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        // Windows can transiently refuse a replace while a reader holds the
        // file; one retry covers the common antivirus/indexer race.
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Err(e2) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(raise_error!(
                format!(
                    "cannot replace tombstone file {}: {e}; retry failed: {e2}",
                    path.display()
                ),
                ErrorCode::InternalError
            ));
        }
    }
    sync_dir(dir);
    Ok(())
}

/// Record that `uids` were deleted locally from one source mailbox.
///
/// Must be called before the messages are deleted. A tombstone for a message
/// that is still present is harmless: gap-fill only consults it when the
/// message is missing locally.
pub async fn record_uids(
    account_id: u64,
    mailbox_id: u64,
    uid_validity: u32,
    uids: &[u32],
) -> BichonResult<()> {
    if uids.is_empty() {
        return Ok(());
    }
    let _lock = WRITE_LOCK.lock().await;
    let _gate = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    let mut tombstones = load(account_id, mailbox_id)?.unwrap_or(Tombstones {
        account_id,
        mailbox_id,
        uid_validity,
        bitmap: RoaringBitmap::new(),
    });
    if tombstones.uid_validity != uid_validity {
        // The mailbox was rebuilt since the last deletion; old UIDs mean
        // nothing in the new generation.
        info!(
            account_id,
            mailbox_id,
            old = tombstones.uid_validity,
            new = uid_validity,
            "tombstone generation changed; starting a fresh bitmap"
        );
        tombstones = Tombstones {
            account_id,
            mailbox_id,
            uid_validity,
            bitmap: RoaringBitmap::new(),
        };
    }
    let before = tombstones.bitmap.len();
    for uid in uids {
        tombstones.bitmap.insert(*uid);
    }
    if tombstones.bitmap.len() != before {
        save(&tombstones)?;
    }
    Ok(())
}

/// Drop the tombstone file for one mailbox (mailbox deleted).
pub async fn remove_mailbox(account_id: u64, mailbox_id: u64) -> BichonResult<()> {
    let _lock = WRITE_LOCK.lock().await;
    let _gate = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    let path = file_path(account_id, mailbox_id);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(raise_error!(
                format!("cannot remove tombstone file {}: {e}", path.display()),
                ErrorCode::InternalError
            ))
        }
    }
    // Best-effort: remove the now-empty account directory.
    if let Some(dir) = path.parent() {
        let _ = fs::remove_dir(dir);
    }
    Ok(())
}

/// Drop every tombstone belonging to an account (account deleted).
pub async fn remove_account(account_id: u64) -> BichonResult<()> {
    let _lock = WRITE_LOCK.lock().await;
    let _gate = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    let dir = root_dir().join(account_id.to_string());
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(raise_error!(
            format!("cannot remove tombstone directory {}: {e}", dir.display()),
            ErrorCode::InternalError
        )),
    }
}

/// Remove tombstoned UIDs from a download candidate list.
///
/// `uid_validity` is `None` for mailboxes that never had a remote generation
/// (imports, SMTP journal): there is nothing to match, so the list is returned
/// unchanged.
pub fn filter_uids(
    account_id: u64,
    mailbox_id: u64,
    uid_validity: Option<u32>,
    mut uids: Vec<u32>,
) -> BichonResult<Vec<u32>> {
    let Some(uid_validity) = uid_validity else {
        return Ok(uids);
    };
    let Some(tombstones) = load_for(account_id, mailbox_id, uid_validity)? else {
        return Ok(uids);
    };
    uids.retain(|uid| !tombstones.bitmap.contains(*uid));
    Ok(uids)
}

/// Drop tombstones for UIDs that no longer exist on the server. Only call
/// this after a *complete* remote enumeration: a partial UID list would make
/// us forget tombstones that are still needed and re-download those messages.
pub async fn retain_remote_uids(
    account_id: u64,
    mailbox_id: u64,
    uid_validity: u32,
    remote_uids: &HashSet<u32>,
) -> BichonResult<()> {
    let _lock = WRITE_LOCK.lock().await;
    let Some(mut tombstones) = load_for(account_id, mailbox_id, uid_validity)? else {
        return Ok(());
    };
    let before = tombstones.bitmap.len();
    let stale: Vec<u32> = tombstones
        .bitmap
        .iter()
        .filter(|uid| !remote_uids.contains(uid))
        .collect();
    for uid in stale {
        tombstones.bitmap.remove(uid);
    }
    if tombstones.bitmap.len() == before {
        return Ok(());
    }
    let _gate = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
    if tombstones.bitmap.is_empty() {
        let path = file_path(account_id, mailbox_id);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(raise_error!(
                    format!("cannot remove empty tombstone file {}: {e}", path.display()),
                    ErrorCode::InternalError
                ))
            }
        }
    } else {
        save(&tombstones)?;
    }
    Ok(())
}

/// Batch variant of [`record_uids`]: one file write per mailbox.
pub async fn record_deleted_messages(
    account_id: u64,
    by_mailbox: HashMap<u64, (u32, Vec<u32>)>,
) -> BichonResult<()> {
    for (mailbox_id, (uid_validity, uids)) in by_mailbox {
        record_uids(account_id, mailbox_id, uid_validity, &uids).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(uids: &[u32]) -> RoaringBitmap {
        uids.iter().copied().collect()
    }

    fn sample(account_id: u64, mailbox_id: u64, uid_validity: u32, uids: &[u32]) -> Tombstones {
        Tombstones {
            account_id,
            mailbox_id,
            uid_validity,
            bitmap: bitmap(uids),
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let t = sample(7, 11, 42, &[1, 2, 3, 900_001]);
        let bytes = encode(&t).unwrap();
        let decoded = decode(Path::new("in-memory"), &bytes).unwrap();
        assert_eq!(decoded.account_id, 7);
        assert_eq!(decoded.mailbox_id, 11);
        assert_eq!(decoded.uid_validity, 42);
        assert_eq!(decoded.bitmap, t.bitmap);
    }

    #[test]
    fn corrupt_payload_is_ignored() {
        let t = sample(1, 2, 3, &[10, 20, 30]);
        let mut bytes = encode(&t).unwrap();
        let last = bytes.len() - 1;
        bytes[HEADER_LEN] ^= 0xff; // flip a payload byte, CRC no longer matches
        assert!(decode(Path::new("in-memory"), &bytes).is_none());
        bytes[last] ^= 0xff; // and a header/crc byte
        assert!(decode(Path::new("in-memory"), &bytes).is_none());
    }

    #[test]
    fn truncated_and_bad_magic_are_ignored() {
        let t = sample(1, 2, 3, &[10]);
        let bytes = encode(&t).unwrap();
        assert!(decode(Path::new("in-memory"), &bytes[..HEADER_LEN]).is_none());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(decode(Path::new("in-memory"), &bad).is_none());
    }

    #[test]
    fn payload_length_mismatch_is_ignored() {
        let t = sample(1, 2, 3, &[10, 11]);
        let mut bytes = encode(&t).unwrap();
        let claimed = (bytes.len() as u32).to_le_bytes();
        bytes[30..34].copy_from_slice(&claimed);
        assert!(decode(Path::new("in-memory"), &bytes).is_none());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bichon-tombstone-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_round_trip_and_overwrite() {
        let dir = scratch("roundtrip");
        let path = dir.join("1").join("2");
        let first = sample(1, 2, 9, &[1, 7, 900]);
        save_at(&path, &first).unwrap();
        let loaded = load_at(&path).unwrap().unwrap();
        assert_eq!(loaded.bitmap, first.bitmap);
        assert_eq!(loaded.uid_validity, 9);

        // Overwrite keeps the old contents merged by the caller; `save_at`
        // itself must replace atomically and never leave a partial file.
        let second = sample(1, 2, 9, &[1, 7, 900, 901]);
        save_at(&path, &second).unwrap();
        assert_eq!(load_at(&path).unwrap().unwrap().bitmap, second.bitmap);
        // No temp file survives a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = scratch("missing");
        assert!(load_at(&dir.join("1").join("2")).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    static TEST_ENV: std::sync::Once = std::sync::Once::new();

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            let _ = &*crate::settings::cli::SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
        });
    }

    #[tokio::test]
    async fn record_filter_and_generation_change() {
        init_test_env();
        // Shares the process-global tombstone root with the backup preparer
        // test; serialize so one test's cleanup cannot delete a directory the
        // other is walking.
        let _global = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;
        let account_id = 70_001;
        let mailbox_id = 70_002;

        record_uids(account_id, mailbox_id, 5, &[10, 20, 30])
            .await
            .unwrap();
        assert_eq!(
            filter_uids(account_id, mailbox_id, Some(5), vec![10, 11, 20, 31]).unwrap(),
            vec![11, 31]
        );
        // A different UIDVALIDITY generation must not consult the old file.
        assert_eq!(
            filter_uids(account_id, mailbox_id, Some(6), vec![10, 11, 20, 31]).unwrap(),
            vec![10, 11, 20, 31]
        );
        assert_eq!(
            filter_uids(account_id, mailbox_id, None, vec![10]).unwrap(),
            vec![10]
        );

        // A new generation resets the bitmap rather than merging unrelated
        // UIDs into it.
        record_uids(account_id, mailbox_id, 6, &[55])
            .await
            .unwrap();
        assert_eq!(
            filter_uids(account_id, mailbox_id, Some(6), vec![55, 10]).unwrap(),
            vec![10]
        );

        // Pruning keeps only UIDs the server still has; once the set is empty
        // the file disappears.
        retain_remote_uids(account_id, mailbox_id, 6, &[55u32, 99].into_iter().collect())
            .await
            .unwrap();
        assert_eq!(
            filter_uids(account_id, mailbox_id, Some(6), vec![55]).unwrap(),
            Vec::<u32>::new()
        );
        retain_remote_uids(account_id, mailbox_id, 6, &[99u32].into_iter().collect())
            .await
            .unwrap();
        assert!(load(account_id, mailbox_id).unwrap().is_none());

        remove_account(account_id).await.unwrap();
    }

    #[test]
    fn bitmap_filtering_math() {
        let tombstones = sample(1, 2, 5, &[2, 4, 100]);
        let candidates = vec![1, 2, 3, 4, 5, 100, 101];
        let kept: Vec<u32> = candidates
            .into_iter()
            .filter(|uid| !tombstones.bitmap.contains(*uid))
            .collect();
        assert_eq!(kept, vec![1, 3, 5, 101]);
    }
}
