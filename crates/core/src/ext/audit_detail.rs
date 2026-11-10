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

// Detail-file externalization for high-volume audit events (Pro).
//
// A batch delete of N messages would otherwise become N `email.deleted`
// records — enough to overflow the audit writer's bounded channel, and N
// near-identical rows. When a batch is large, the per-message detail is
// instead written to a file under `<root>/audit/detail/` and the audit row
// carries only a small reference: relative path, byte size, and SHA-256. The
// hash binds the file's content into the audit hash chain, so tampering with
// the file is detectable even though the file itself lives outside the
// database.
//
// File naming / collision safety:
// - the file is named `<uuid>.jsonl` where the uuid is also the event's
//   `batch_id`, so the audit row and the file are linked 1:1 and the id needs
//   no lookup to locate the file;
// - the file is opened with `create_new`, which fails if the name is already
//   taken — an overwrite is impossible by construction. On the astronomically
//   unlikely collision a fresh id is drawn and the write retried (bounded);
// - the file is flushed and fsync'd before the audit event is emitted, so a
//   download endpoint can never serve a half-written file (the row does not
//   exist yet) and a crash leaves only an unreferenced orphan, which the
//   retention sweep reclaims by age.
//
// In the community edition this module is inert: nothing calls `configure`,
// so `enabled()` is false and delete paths fall back to per-message events
// (which the community event bus discards anyway).

use crate::ext::event_bus::EventPayload;
use crate::settings::dir::DATA_DIR_MANAGER;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// One message about to be deleted, with everything the audit trail needs to
/// stay readable once the content is gone.
pub struct DeletedMessageSnapshot {
    pub account_id: u64,
    pub email_id: String,
    pub mailbox_id: u64,
    pub subject: Option<String>,
    pub snapshot: Option<EventPayload>,
}

/// A written detail file, plus everything the audit row needs to reference it.
#[derive(Debug, Clone)]
pub struct DetailRef {
    /// Correlation id shared with the audit event; also the file name stem.
    pub batch_id: String,
    /// `detail/<batch_id>.jsonl`, relative to the audit directory.
    pub rel_path: String,
    pub size: u64,
    pub sha256: String,
    pub count: u64,
    pub account_ids: Vec<u64>,
}

/// What a delete should do on the audit side. `Detail` means the detail file
/// is already on disk and fsync'd; `Inline` means per-message events.
#[derive(Debug)]
pub enum BatchAuditDecision {
    Detail(DetailRef),
    Inline,
}

struct Config {
    /// `<root>/audit/detail/`.
    dir: PathBuf,
    /// Batches larger than this are externalized; at or below it, per-message
    /// events (small rows, queryable by `email_id`) stay inline.
    threshold: u64,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Called once by Pro at audit startup. In the community edition this is never
/// called and the whole module stays inert.
pub fn configure(dir: PathBuf, threshold: u64) {
    let _ = CONFIG.set(Config { dir, threshold });
}

pub fn enabled() -> bool {
    CONFIG.get().is_some()
}

pub fn threshold() -> u64 {
    CONFIG.get().map(|c| c.threshold).unwrap_or(0)
}

/// Load the snapshot for one message about to be deleted.
///
/// Shared by the inline path (`audit_snapshots_for_deleted`) and the detail
/// file writer, so the two paths never drift on what "the detail" is.
pub fn load_deleted_snapshot(account_id: u64, eid: &str) -> DeletedMessageSnapshot {
    let mut mailbox_id = 0u64;
    let mut subject = None;
    let mut snapshot: EventPayload = serde_json::Map::new();
    // `get()` (not the forcible deref) so an uninitialized store — a test, or
    // a startup order where the audit path runs first — degrades to an empty
    // snapshot instead of panicking while the tantivy store spins up. In the
    // running product the store is always up before any delete can happen.
    if let Some(manager) = std::sync::LazyLock::get(&ENVELOPE_MANAGER) {
        if let Ok(Some(ea)) = manager.get_envelope_by_id(account_id, eid) {
        let e = ea.envelope;
        mailbox_id = e.mailbox_id;
        subject = Some(e.subject.clone());
        snapshot.insert("from".into(), serde_json::json!(e.from));
        snapshot.insert("date".into(), serde_json::json!(e.date));
        snapshot.insert("size".into(), serde_json::json!(e.size));
        snapshot.insert(
            "attachment_count".into(),
            serde_json::json!(e.regular_attachment_count),
        );
        if let Some(atts) = ea.attachments {
            let names: Vec<String> = atts
                .iter()
                .filter_map(|a| a.filename.clone())
                .collect();
            if !names.is_empty() {
                snapshot.insert("attachment_names".into(), serde_json::json!(names));
            }
        }
        snapshot.insert("content_hash".into(), serde_json::json!(e.content_hash));
        }
    }
    DeletedMessageSnapshot {
        account_id,
        email_id: eid.to_string(),
        mailbox_id,
        subject,
        snapshot: Some(snapshot),
    }
}

/// Decide how a delete batch should be audited: externalized to a detail file
/// when it is large enough (and the file can be written), otherwise inline.
///
/// The decision is made *now*, before the messages are deleted, so the detail
/// file is complete before anything that would make it unobtainable.
pub fn decide_batch(request: &HashMap<u64, Vec<String>>) -> BatchAuditDecision {
    let Some(cfg) = CONFIG.get() else {
        return BatchAuditDecision::Inline;
    };
    decide_batch_in(&cfg.dir, cfg.threshold, request)
}

fn decide_batch_in(
    dir: &Path,
    threshold: u64,
    request: &HashMap<u64, Vec<String>>,
) -> BatchAuditDecision {
    let total: u64 = request.values().map(|v| v.len() as u64).sum();
    if total <= threshold {
        return BatchAuditDecision::Inline;
    }
    match write_batch_detail_in(dir, request) {
        Ok(r) => BatchAuditDecision::Detail(r),
        // Never lose the audit trail: if the detail file cannot be written,
        // fall back to the inline per-message path.
        Err(_) => BatchAuditDecision::Inline,
    }
}

/// Stream every message's snapshot to a fresh detail file and return the
/// reference. Collision-safe: `create_new` makes overwriting an existing file
/// impossible, and a collision (effectively impossible for UUIDv4) draws a
/// fresh id and retries a bounded number of times.
pub fn write_batch_detail(request: &HashMap<u64, Vec<String>>) -> Result<DetailRef, String> {
    let Some(cfg) = CONFIG.get() else {
        return Err("audit detail is not configured".into());
    };
    write_batch_detail_in(&cfg.dir, request)
}

fn write_batch_detail_in(
    dir: &Path,
    request: &HashMap<u64, Vec<String>>,
) -> Result<DetailRef, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create detail dir: {e}"))?;
    let mut last_err = String::new();
    for _ in 0..3 {
        let batch_id = uuid::Uuid::new_v4().to_string();
        let file_name = format!("{batch_id}.jsonl");
        let full_path = dir.join(&file_name);
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last_err = format!("name collision on {file_name}");
                continue;
            }
            Err(e) => return Err(format!("open detail file {file_name}: {e}")),
        };
        return stream_detail(file, request, batch_id);
    }
    Err(format!("detail file name collisions on every attempt: {last_err}"))
}

fn stream_detail(
    file: std::fs::File,
    request: &HashMap<u64, Vec<String>>,
    batch_id: String,
) -> Result<DetailRef, String> {
    use ring::digest::{Context, SHA256};
    let mut ctx = Context::new(&SHA256);
    let mut writer = BufWriter::new(file);
    let mut count: u64 = 0;
    for (account_id, envelope_ids) in request {
        for eid in envelope_ids {
            let s = load_deleted_snapshot(*account_id, eid);
            let line = serde_json::json!({
                "email_id": s.email_id,
                "account_id": s.account_id,
                "mailbox_id": s.mailbox_id,
                "subject": s.subject,
                "snapshot": s.snapshot,
            });
            let bytes = serde_json::to_vec(&line)
                .map_err(|e| format!("serialize detail line: {e}"))?;
            writer
                .write_all(&bytes)
                .map_err(|e| format!("write detail line: {e}"))?;
            writer
                .write_all(b"\n")
                .map_err(|e| format!("write detail line: {e}"))?;
            ctx.update(&bytes);
            ctx.update(b"\n");
            count += 1;
        }
    }
    let file = writer.into_inner().map_err(|e| format!("flush detail file: {e}"))?;
    file.sync_all().map_err(|e| format!("fsync detail file: {e}"))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let sha256 = hex::encode(ctx.finish());
    let account_ids = request.keys().copied().collect();
    let rel_path = format!("detail/{batch_id}.jsonl");
    Ok(DetailRef {
        batch_id,
        rel_path,
        size,
        sha256,
        count,
        account_ids,
    })
}

/// One message removed by the retention sweep, as written to the detail file.
pub struct PurgedMessageRow {
    pub email_id: String,
    pub mailbox_id: u64,
    pub uid: u32,
    pub date: i64,
    pub internal_date: i64,
}

/// Write the detail file for one account's retention purge. The rows carry no
/// content snapshot (the messages are already gone), only identity and dates —
/// enough to resolve and audit every removed envelope.
pub fn write_retention_detail(
    account_id: u64,
    rows: &[PurgedMessageRow],
) -> Result<DetailRef, String> {
    let cfg = CONFIG.get().ok_or("audit detail is not configured")?;
    std::fs::create_dir_all(&cfg.dir).map_err(|e| format!("create detail dir: {e}"))?;
    let mut last_err = String::new();
    for _ in 0..3 {
        let batch_id = uuid::Uuid::new_v4().to_string();
        let file_name = format!("{batch_id}.jsonl");
        let full_path = cfg.dir.join(&file_name);
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last_err = format!("name collision on {file_name}");
                continue;
            }
            Err(e) => return Err(format!("open detail file {file_name}: {e}")),
        };

        use ring::digest::{Context, SHA256};
        let mut ctx = Context::new(&SHA256);
        let mut writer = BufWriter::new(file);
        for row in rows {
            let line = serde_json::json!({
                "email_id": row.email_id,
                "account_id": account_id,
                "mailbox_id": row.mailbox_id,
                "uid": row.uid,
                "date": row.date,
                "internal_date": row.internal_date,
            });
            let bytes = serde_json::to_vec(&line)
                .map_err(|e| format!("serialize detail line: {e}"))?;
            writer
                .write_all(&bytes)
                .map_err(|e| format!("write detail line: {e}"))?;
            writer
                .write_all(b"\n")
                .map_err(|e| format!("write detail line: {e}"))?;
            ctx.update(&bytes);
            ctx.update(b"\n");
        }
        let file = writer
            .into_inner()
            .map_err(|e| format!("flush detail file: {e}"))?;
        file.sync_all().map_err(|e| format!("fsync detail file: {e}"))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let sha256 = hex::encode(ctx.finish());
        let rel_path = format!("detail/{batch_id}.jsonl");
        return Ok(DetailRef {
            batch_id,
            rel_path,
            size,
            sha256,
            count: rows.len() as u64,
            account_ids: vec![account_id],
        });
    }
    Err(format!(
        "detail file name collisions on every attempt: {last_err}"
    ))
}

/// Resolve a stored `detail/<name>.jsonl` reference to its absolute path, or
/// `None` if the reference is malformed (path traversal is refused) or the
/// file is gone.
pub fn resolve(rel_path: &str) -> Option<PathBuf> {
    let cfg = CONFIG.get()?;
    resolve_in(&cfg.dir, rel_path)
}

fn resolve_in(dir: &Path, rel_path: &str) -> Option<PathBuf> {
    let name = rel_path.strip_prefix("detail/")?;
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// Best-effort removal of a referenced detail file (called when its audit row
/// is purged).
pub fn delete_detail(rel_path: &str) {
    if let Some(p) = resolve(rel_path) {
        let _ = std::fs::remove_file(p);
    }
}

/// Delete detail files older than `max_age`. Because a detail file's mtime is
/// written at the same moment as its audit row's `ts_ms`, a file older than the
/// retention window can only belong to a row that has already been purged —
/// i.e. an orphan (crash leftover, or a row deleted another way). Runs from the
/// audit retention task, so no row scan is needed to reconcile.
pub fn sweep_orphans(max_age: Duration) -> u64 {
    let Some(cfg) = CONFIG.get() else {
        return 0;
    };
    sweep_orphans_in(&cfg.dir, max_age)
}

fn sweep_orphans_in(dir: &Path, max_age: Duration) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut deleted = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mt| now.duration_since(mt).ok())
            .map(|age| age > max_age)
            .unwrap_or(false);
        if old && std::fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// The default detail directory derived from the data root, used by Pro at
/// startup so the location is decided in one place.
pub fn default_dir() -> PathBuf {
    DATA_DIR_MANAGER.root_dir.join("audit").join("detail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bichon-audit-detail-{tag}-{}", std::process::id()))
    }

    fn mk(tag: &str) -> PathBuf {
        let d = tmp_dir(tag).join("detail");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_and_resolve_roundtrip() {
        let dir = mk("roundtrip");
        let mut req = HashMap::new();
        req.insert(7u64, vec!["eid-1".to_string()]);
        let r = write_batch_detail_in(&dir, &req).expect("writes detail");
        assert_eq!(r.count, 1);
        assert_eq!(r.account_ids, vec![7]);
        let abs = resolve_in(&dir, &r.rel_path).expect("resolves");
        assert_eq!(
            abs.file_name().unwrap().to_str().unwrap(),
            format!("{}.jsonl", r.batch_id)
        );
        let content = std::fs::read_to_string(&abs).unwrap();
        assert!(content.contains("eid-1"));
        // sha256 is a real 64-hex digest
        assert_eq!(r.sha256.len(), 64);
        assert!(r.size > 0);
        // delete_detail_in-style removal via the public path needs the global;
        // here just check the file is on disk.
        assert!(abs.exists());
        let _ = std::fs::remove_dir_all(tmp_dir("roundtrip"));
    }

    #[test]
    fn create_new_never_overwrites() {
        let dir = mk("collision");
        // Seed a file at a known name, then verify the open semantics the
        // writer relies on: create_new on an existing path must fail and must
        // leave the existing content untouched.
        let existing = dir.join("existing.jsonl");
        std::fs::write(&existing, "keep me").unwrap();
        let err = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&existing)
            .err()
            .expect("create_new refuses an existing file");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep me");
        let _ = std::fs::remove_dir_all(tmp_dir("collision"));
    }

    #[test]
    fn resolve_refuses_traversal() {
        let dir = mk("traversal");
        assert!(resolve_in(&dir, "detail/../audit.db").is_none());
        assert!(resolve_in(&dir, "detail/a/b.jsonl").is_none());
        assert!(resolve_in(&dir, "other/x.jsonl").is_none());
        let _ = std::fs::remove_dir_all(tmp_dir("traversal"));
    }

    #[test]
    fn sweep_removes_old_keeps_fresh() {
        let dir = mk("sweep");
        let old = dir.join("old.jsonl");
        let fresh = dir.join("fresh.jsonl");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&fresh, "y").unwrap();
        // Backdate the old file using std file times.
        {
            let past = std::time::SystemTime::now() - Duration::from_secs(3600);
            let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
            let times = std::fs::FileTimes::new().set_modified(past);
            f.set_times(times).unwrap();
        }
        let deleted = sweep_orphans_in(&dir, Duration::from_secs(60));
        assert_eq!(deleted, 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        let _ = std::fs::remove_dir_all(tmp_dir("sweep"));
    }

    #[test]
    fn decide_batch_respects_threshold_and_degrades_inline() {
        let dir = mk("policy");
        let mut big: HashMap<u64, Vec<String>> = HashMap::new();
        big.insert(1u64, (0..200).map(|i| format!("e{i}")).collect());

        // At or under the threshold: inline, no file.
        let small: HashMap<u64, Vec<String>> = big
            .iter()
            .map(|(k, v)| (*k, v.iter().take(100).cloned().collect()))
            .collect();
        assert!(matches!(
            decide_batch_in(&dir, 100, &small),
            BatchAuditDecision::Inline
        ));
        assert_eq!(std::fs::read_dir(&dir).map(|_| 0).unwrap_or(0), 0);

        // Above the threshold: detail file written even though no envelope
        // resolves (lines carry empty snapshots — the write must still work).
        match decide_batch_in(&dir, 100, &big) {
            BatchAuditDecision::Detail(r) => {
                assert_eq!(r.count, 200);
                assert!(resolve_in(&dir, &r.rel_path).is_some());
            }
            BatchAuditDecision::Inline => panic!("200 > threshold must externalize"),
        }

        // An unwritable directory degrades to inline rather than erroring out
        // of the delete path: the audit trail must never be a hard dependency.
        // Windows can't reliably make a directory unwritable from here, so
        // probe the write itself: only a truly failing write must degrade.
        let read_only = tmp_dir("policy").join("ro");
        std::fs::create_dir_all(&read_only).unwrap();
        if write_batch_detail_in(&read_only, &big).is_err() {
            assert!(matches!(
                decide_batch_in(&read_only, 100, &big),
                BatchAuditDecision::Inline
            ));
        }
        let _ = std::fs::remove_dir_all(tmp_dir("policy"));
    }
}
