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

//! Rolling log of backup-run anomalies.
//!
//! Every failed run appends one entry here, and so does every run whose task
//! died without reporting (a panic, a task abort, or a restart while it was
//! still running). Only the newest [`LIMIT`] entries are kept.
//!
//! This is a **separate** collection from `backup_records` on purpose: the run
//! history rolls at `BackupRecord::HISTORY_LIMIT` *runs*, most of which may be
//! successful, while an operator asking "why do my backups fail?" wants the
//! last N failures regardless of how many good runs happened in between.
//!
//! Writes go straight to the memdb collection (like the run records) instead
//! of through the gated helpers: a run can fail inside the capture window,
//! where the write gate is paused and the helpers would reject the write.

use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::database::manager::DB_MANAGER;
use crate::database::MemDbModel;
use crate::utc_now;

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "web-api", oai(rename_all = "snake_case"))]
pub enum BackupErrorKind {
    /// The run reached `finish()` and reported an error.
    Failed,
    /// The run task died without reporting: a panic, a task abort, or a
    /// process restart while the run was still in progress.
    AbruptExit,
}

/// One entry in the rolling error log.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BackupErrorEntry {
    pub id: String,
    /// UTC millis when the entry was written.
    pub at: i64,
    /// Run record id, when the run got far enough to have one.
    pub run_id: Option<String>,
    /// `"manual"` or `"schedule"` (empty when the trigger is unknown).
    pub trigger: String,
    /// Phase the run was in (`"preflight"`, `"capture:<preparer>"`,
    /// `"upload"`, `"finalize"`).
    pub phase: String,
    pub kind: BackupErrorKind,
    /// Operator-facing message — the same text the run record carries.
    pub message: String,
    /// Where the run was writing (target plus prefix), for context.
    pub target: Option<String>,
}

impl MemDbModel for BackupErrorEntry {
    fn collection() -> &'static str {
        "backup_errors"
    }
    fn key(&self) -> String {
        self.id.clone()
    }
}

/// How many entries the log keeps. The WebUI shows all of them.
pub const LIMIT: usize = 30;

/// Highest `at` handed out so far. Entries can share a wall-clock
/// millisecond (a failing preflight is fast), and ordering is what decides
/// which entry pruning drops; making the stamp strictly increasing keeps
/// "newest first" and the retention of the newest [`LIMIT`] entries exact.
static LAST_AT: AtomicI64 = AtomicI64::new(0);

fn next_at() -> i64 {
    let now = utc_now!();
    let mut last = LAST_AT.load(Ordering::Acquire);
    loop {
        let next = now.max(last + 1);
        match LAST_AT.compare_exchange_weak(last, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return next,
            Err(actual) => last = actual,
        }
    }
}

/// The newest `limit` entries, newest first. Never returns more than [`LIMIT`].
pub fn list_recent(limit: usize) -> Vec<BackupErrorEntry> {
    let mut all = list_all();
    all.sort_by_key(|e| std::cmp::Reverse(e.at));
    all.truncate(limit.min(LIMIT));
    all
}

/// Append one entry and prune the log back to [`LIMIT`].
///
/// Best-effort by design: a failure to persist an error must never turn into a
/// second failure (one call site runs while unwinding a panic), so problems
/// are logged and swallowed.
pub fn record(
    kind: BackupErrorKind,
    trigger: &str,
    run_id: Option<&str>,
    phase: &str,
    message: &str,
    target: Option<&str>,
) {
    let entry = BackupErrorEntry {
        // Stable unique key; ordering comes from `at` (see [`next_at`]).
        id: uuid::Uuid::now_v7().to_string(),
        at: next_at(),
        run_id: run_id.map(str::to_string),
        trigger: trigger.to_string(),
        phase: phase.to_string(),
        kind,
        message: message.to_string(),
        target: target.map(str::to_string),
    };
    let db = DB_MANAGER.db();
    if let Err(e) = db
        .collection(BackupErrorEntry::collection())
        .upsert(entry.key(), &entry)
    {
        error!("backup: cannot persist error entry: {e:?}");
        return;
    }
    prune_beyond(LIMIT);
}

/// Delete entries beyond the newest `limit`. Returns how many were removed.
fn prune_beyond(limit: usize) -> usize {
    let db = DB_MANAGER.db();
    let Ok(all) = db
        .collection(BackupErrorEntry::collection())
        .list_all::<BackupErrorEntry>()
    else {
        return 0;
    };
    if all.len() <= limit {
        return 0;
    }
    let mut all = all;
    all.sort_by_key(|e| std::cmp::Reverse(e.at));
    let mut removed = 0;
    for entry in &all[limit..] {
        if db
            .collection(BackupErrorEntry::collection())
            .delete(entry.key())
            .unwrap_or(false)
        {
            removed += 1;
        }
    }
    if removed > 0 {
        warn!("backup: pruned {removed} error log entries beyond the cap");
    }
    removed
}

fn list_all() -> Vec<BackupErrorEntry> {
    crate::database::list_all_impl(DB_MANAGER.db()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests share the global memdb and the manager tests write entries
    /// too, so they serialize on the same lock the backup state tests use.
    #[tokio::test]
    async fn entries_are_newest_first_and_capped() {
        init_test_env();
        let _guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        clear_for_test();

        for i in 0..LIMIT + 5 {
            record(
                BackupErrorKind::Failed,
                "manual",
                Some(&format!("run-{i}")),
                "preflight",
                &format!("failure {i}"),
                Some("s3://aws/bucket/prefix"),
            );
        }

        let all = list_recent(LIMIT);
        assert_eq!(all.len(), LIMIT, "the log must keep exactly {LIMIT} entries");
        assert_eq!(
            all[0].message,
            format!("failure {}", LIMIT + 4),
            "newest first, oldest pruned: {:?}",
            all[0]
        );
        assert!(
            !all.iter().any(|e| e.message == "failure 0"),
            "the oldest entries must be pruned"
        );
        // A caller asking for fewer gets fewer; asking for more is clamped.
        assert_eq!(list_recent(3).len(), 3);
        assert_eq!(list_recent(usize::MAX).len(), LIMIT);

        clear_for_test();
    }

    #[tokio::test]
    async fn an_entry_keeps_the_run_context() {
        init_test_env();
        let _guard = crate::backup::BACKUP_STATE_TESTS.lock().await;
        clear_for_test();

        record(
            BackupErrorKind::AbruptExit,
            "schedule",
            None,
            "capture:memdb",
            "the run ended unexpectedly",
            Some("local:/mnt/backup/prefix"),
        );

        let entry = list_recent(1).pop().expect("entry must be readable");
        assert_eq!(entry.kind, BackupErrorKind::AbruptExit);
        assert_eq!(entry.trigger, "schedule");
        assert_eq!(entry.phase, "capture:memdb");
        assert_eq!(entry.run_id, None);
        assert_eq!(entry.target.as_deref(), Some("local:/mnt/backup/prefix"));
        assert_eq!(entry.message, "the run ended unexpectedly");

        clear_for_test();
    }

    fn init_test_env() {
        use std::sync::Once;
        static TEST_ENV: Once = Once::new();
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*crate::settings::cli::SETTINGS;
            let _ = &*DB_MANAGER;
        });
    }

    fn clear_for_test() {
        let db = DB_MANAGER.db();
        for entry in list_all() {
            let _ = db
                .collection(BackupErrorEntry::collection())
                .delete(entry.key());
        }
    }
}
