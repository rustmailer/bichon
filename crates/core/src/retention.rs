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

//! Account-level retention (free/community feature).
//!
//! Each account can carry a `retention_days` window. A background sweep purges
//! envelopes whose *effective date* — the older of `date` (RFC 5322 Date
//! header) and `internal_date` (IMAP INTERNALDATE / import timestamp) — falls
//! outside the window. Using the older of the two is deliberate: it is the
//! timestamp an auditor would trust, and it is robust against the common
//! failure modes where INTERNALDATE is spuriously rewritten (PST/mbox/EML
//! import, mailbox migration, MTA redelivery).
//!
//! Accounts under a legal hold are always exempt, regardless of their
//! `retention_days`.

use crate::account::migration::AccountModel;
use crate::backup::gate::WRITE_GATE;
use crate::common::periodic::PeriodicTask;
use crate::error::BichonResult;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::utc_now;
use std::time::Duration;
use tracing::{info, warn};

/// How often the retention sweep runs.
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Milliseconds per day (used to derive the cutoff from `retention_days`).
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Epoch-millis cutoff for a retention window of `days` days, relative to now.
/// Overflow-safe: a window larger than `i64` days simply saturates so the
/// resulting cutoff matches nothing.
pub fn retention_cutoff_ms(days: u64) -> i64 {
    let days_i64 = days.min(i64::MAX as u64) as i64;
    utc_now!().saturating_sub(days_i64.saturating_mul(DAY_MS))
}

/// The effective (single) date of a message, mirroring the sweep's semantics:
/// the older of the Date header and the internal date, treating `0` as
/// "unknown". `None` means the message has no usable timestamp at all (both
/// unknown) and can therefore never be considered out of window.
pub fn effective_date_ms(date: i64, internal_date: i64) -> Option<i64> {
    match (date, internal_date) {
        (0, 0) => None,
        (0, x) => Some(x),
        (x, 0) => Some(x),
        (a, b) => Some(a.min(b)),
    }
}

/// IMAP `SINCE` floor for an account's retention window, or `None` when
/// retention is inactive (disabled, zero days, or the account is on legal
/// hold). Used by the download flows to keep full UID re-syncs from
/// re-downloading mail that the retention sweep already purged.
///
/// The floor is deliberately conservative: it only prunes remote messages
/// whose INTERNALDATE is older than the window — i.e. messages guaranteed to be
/// out of window regardless of their Date header. Messages that the server
/// still reports as recent are fetched and then re-checked against the exact
/// window at ingest time.
pub fn retention_floor_date(account: &AccountModel) -> Option<String> {
    if account.retention_days_effective() == 0 || account.is_on_hold() {
        return None;
    }
    let cutoff_ms = retention_cutoff_ms(account.retention_days_effective());
    let dt = chrono::DateTime::from_timestamp_millis(cutoff_ms)?;
    Some(dt.format("%d-%b-%Y").to_string())
}

/// Start the background retention scheduler.
///
/// The task runs on the shared shutdown signal (see `common::periodic`), so the
/// returned handle is intentionally dropped: the loop self-terminates when the
/// process is stopping. This is a community feature, so it is started from the
/// single `BichonContext::initialize()` hook shared by both the community and
/// Pro servers.
pub fn start_retention_scheduler() {
    let task = PeriodicTask::new("retention-sweep");
    task.start(
        |_param: Option<u64>| Box::pin(async move { retention_sweep().await }),
        None,
        SWEEP_INTERVAL,
        false,
        false,
    );
}

/// Run one retention sweep over every account.
///
/// Accounts with retention disabled (`None`/`0`) or currently under a legal
/// hold are skipped. Per-account failures are logged and do not abort the
/// sweep for the remaining accounts.
pub async fn retention_sweep() -> BichonResult<()> {
    // Skip a round entirely while a backup window is open: the sweep rewrites
    // the index in place and would race the snapshot.
    if WRITE_GATE.is_paused() {
        info!("retention: skipping sweep, backup in progress");
        return Ok(());
    }
    let accounts = match AccountModel::list_all() {
        Ok(accounts) => accounts,
        Err(e) => {
            warn!("retention sweep: failed to list accounts: {e:?}");
            return Ok(());
        }
    };

    let mut purged_total: u64 = 0;
    let mut held_accounts: usize = 0;

    for account in &accounts {
        let days = account.retention_days_effective();
        if days == 0 {
            continue;
        }
        if account.is_on_hold() {
            held_accounts += 1;
            info!(
                "retention: account {} ({}) is on legal hold, exempt from sweep",
                account.id, account.email
            );
            continue;
        }

        let cutoff_ms = retention_cutoff_ms(days);

        match ENVELOPE_MANAGER
            .delete_envelopes_before(account.id, cutoff_ms)
            .await
        {
            Ok(0) => {}
            Ok(purged) => {
                info!(
                    "retention: purged {purged} envelope(s) from account {} ({}) (window {days}d)",
                    account.id, account.email
                );
                purged_total += purged;
            }
            Err(e) => {
                warn!(
                    "retention: sweep failed for account {} ({}): {e:?}",
                    account.id, account.email
                );
            }
        }
    }

    if purged_total > 0 || held_accounts > 0 {
        info!(
            "retention sweep complete: purged {purged_total} envelope(s) across {} account(s); {held_accounts} account(s) on hold",
            accounts.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::migration::{AccountModel, AccountType};
    use crate::account::payload::AccountCreateRequest;

    fn test_account(retention_days: Option<u64>, legal_hold: bool) -> AccountModel {
        let request = AccountCreateRequest {
            email: "retention@test.example".to_string(),
            login_name: None,
            account_name: None,
            imap: None,
            enabled: true,
            date_since: None,
            date_before: None,
            account_type: AccountType::NoSync,
            download_interval_min: None,
            download_batch_size: None,
            max_email_size_bytes: None,
            use_dangerous: false,
            pgp_key: None,
            imap_quota_bytes: None,
            imap_quota_window: None,
            auto_download_new_mailboxes: None,
            download_schedule: None,
            archive_rules: None,
            extraction_rules: None,
            override_from_headers: None,
            retention_days: None,
            remote_cleanup: None,
            kind: None,
            view: None,
        };
        let mut account = AccountModel::new(1, request).unwrap();
        account.retention_days = retention_days;
        account.legal_hold = legal_hold;
        account
    }

    // ── effective_date_ms ────────────────────────────────────────────

    #[test]
    fn effective_date_uses_older_of_two_timestamps() {
        assert_eq!(effective_date_ms(1_000, 5_000), Some(1_000));
        assert_eq!(effective_date_ms(5_000, 1_000), Some(1_000));
        assert_eq!(effective_date_ms(1_000, 1_000), Some(1_000));
    }

    #[test]
    fn effective_date_single_timestamp_falls_back() {
        assert_eq!(effective_date_ms(0, 5_000), Some(5_000));
        assert_eq!(effective_date_ms(5_000, 0), Some(5_000));
    }

    #[test]
    fn effective_date_both_unknown_is_none() {
        assert_eq!(effective_date_ms(0, 0), None);
    }

    // ── retention_cutoff_ms ──────────────────────────────────────────

    #[test]
    fn cutoff_is_window_back_from_now() {
        let cutoff = retention_cutoff_ms(30);
        let now = utc_now!();
        let expected = now - 30 * DAY_MS;
        assert!((cutoff - expected).abs() < 1_000, "cutoff {cutoff} vs {expected}");
    }

    #[test]
    fn cutoff_zero_window_is_now() {
        assert!((retention_cutoff_ms(0) - utc_now!()).abs() < 1_000);
    }

    #[test]
    fn cutoff_huge_window_does_not_overflow() {
        // u64::MAX days would overflow naive arithmetic; must saturate cleanly.
        let _ = retention_cutoff_ms(u64::MAX);
    }

    // ── retention_floor_date ─────────────────────────────────────────

    #[test]
    fn floor_date_disabled_without_window() {
        let account = test_account(None, false);
        assert_eq!(retention_floor_date(&account), None);

        let account = test_account(Some(0), false);
        assert_eq!(retention_floor_date(&account), None);
    }

    #[test]
    fn floor_date_none_while_on_hold() {
        let account = test_account(Some(90), true);
        assert_eq!(retention_floor_date(&account), None);
    }

    #[test]
    fn floor_date_formats_imap_since_date() {
        let account = test_account(Some(90), false);
        let floor = retention_floor_date(&account).expect("floor date");
        // RFC 3501 date-key format, e.g. "16-Jun-2026".
        let parsed = chrono::NaiveDate::parse_from_str(&floor, "%d-%b-%Y")
            .expect("floor must be a valid IMAP date");
        let today = chrono::Local::now().date_naive();
        assert!(
            parsed <= today,
            "floor {floor} should not be in the future (today {today})"
        );
    }

    // ── retention sweep (integration, real memdb + Tantivy) ──────────
    //
    // Exercises `retention_sweep()` end to end: accounts live in the memdb,
    // envelopes in the real Tantivy index, so the hold exemption and the
    // purge path are verified against the actual storage stack.

    use crate::database::manager::DB_MANAGER;
    use crate::database::insert_impl;
    use crate::settings::cli::SETTINGS;
    use crate::settings::dir::DATA_DIR_MANAGER;
    use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
    use crate::store::tantivy::schema::SchemaTools;
    use std::sync::Once;
    use tantivy::collector::Count;
    use tantivy::query::TermQuery;
    use tantivy::schema::IndexRecordOption;
    use tantivy::Term;

    /// Same shared root as the migration tests so the globals freeze once
    /// regardless of which test module touches them first. Unique per process
    /// so a previous `cargo test` run can never leak index state in here.
    /// The four sweep tests share one memdb and one Tantivy index. Committing
    /// to that index from several test threads at once makes Windows refuse the
    /// files (`PermissionDenied`), so they are serialized — the same convention
    /// as `backup::BACKUP_STATE_TESTS` and `repair::DB_TESTS`.
    // Serialized with every other test that writes the process-global index
    // (not just the sweep tests): concurrent commits to the same tantivy
    // directory make Windows refuse the files with `PermissionDenied`.
    use crate::test_support::GLOBAL_INDEX_LOCK as SWEEP_TESTS;

    /// Wait (bounded) for a backup window to close.
    ///
    /// `retention_sweep` deliberately does nothing while one is open
    /// (`WRITE_GATE.is_paused()`), and the backup tests in this binary open
    /// one; a skipped round looks exactly like a round that found nothing, so
    /// an assertion on the count cannot tell them apart.
    async fn wait_for_gate_open() -> bool {
        use crate::backup::gate::WRITE_GATE;
        for _ in 0..300 {
            if !WRITE_GATE.is_paused() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    static TEST_ENV: Once = Once::new();
    fn init_sweep_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
            let _ = &*ENVELOPE_MANAGER;
        });
    }

    fn insert_account_with_retention(
        id: u64,
        email: &str,
        retention_days: Option<u64>,
    ) -> AccountModel {
        let mut account = test_account(retention_days, false);
        account.id = id;
        account.email = email.to_string();
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();
        account
    }

    async fn write_envelope(id: &str, account_id: u64, date: i64, internal_date: i64) {
        let f = SchemaTools::email_fields();
        let mut doc = tantivy::TantivyDocument::new();
        doc.add_text(f.f_id, id);
        doc.add_u64(f.f_account_id, account_id);
        doc.add_u64(f.f_mailbox_id, 10);
        doc.add_u64(f.f_uid, 1);
        doc.add_i64(f.f_date, date);
        doc.add_i64(f.f_internal_date, internal_date);
        let mut writer = ENVELOPE_MANAGER.index_writer().lock().await;
        writer.add_document(doc).unwrap();
        writer.commit().unwrap();
    }

    async fn envelope_count(account_id: u64) -> usize {
        let f = SchemaTools::email_fields();
        let reader = ENVELOPE_MANAGER.create_reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();
        let query = TermQuery::new(
            Term::from_field_u64(f.f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        searcher.search(&query, &Count).unwrap()
    }

    #[tokio::test]
    async fn sweep_purges_old_envelopes_and_keeps_recent() {
        let _serial = SWEEP_TESTS.lock().await;
        init_sweep_env();
        let account =
            insert_account_with_retention(9101, "sweep-purge@test.example", Some(30));

        let now = utc_now!();
        let old = now - 40 * DAY_MS; // older than the 30-day window
        let recent = now - 5 * DAY_MS; // inside the window
        write_envelope("sweep-old-1", account.id, old, old).await;
        write_envelope("sweep-new-1", account.id, recent, recent).await;

        // A skipped round and a round that found nothing are indistinguishable
        // from the count alone, so wait for the backup window to close and give
        // it a couple of rounds (the gate can be paused between the check and
        // the call). Bounded, and serialized by `SWEEP_TESTS`, so this cannot
        // become a commit storm against the shared index.
        let mut purged = false;
        for _ in 0..3 {
            assert!(wait_for_gate_open().await, "a backup window stayed open");
            retention_sweep().await.unwrap();
            if envelope_count(account.id).await == 1 {
                purged = true;
                break;
            }
        }
        assert!(
            purged,
            "old envelope must be purged and the recent one kept (the sweep was skipped by a \
             backup window on every attempt)"
        );
    }

    #[tokio::test]
    async fn sweep_skips_account_on_legal_hold() {
        let _serial = SWEEP_TESTS.lock().await;
        init_sweep_env();
        let account = insert_account_with_retention(9102, "sweep-held@test.example", Some(30));
        AccountModel::place_legal_hold(account.id, 7, Some("test hold".into())).unwrap();

        let now = utc_now!();
        let old = now - 40 * DAY_MS;
        write_envelope("sweep-held-old", account.id, old, old).await;

        retention_sweep().await.unwrap();

        assert_eq!(
            envelope_count(account.id).await,
            1,
            "held account must be exempt from the sweep"
        );
    }

    #[tokio::test]
    async fn sweep_skips_account_with_retention_disabled() {
        let _serial = SWEEP_TESTS.lock().await;
        init_sweep_env();
        let account = insert_account_with_retention(9103, "sweep-disabled@test.example", None);

        let now = utc_now!();
        let old = now - 40 * DAY_MS;
        write_envelope("sweep-disabled-old", account.id, old, old).await;

        retention_sweep().await.unwrap();

        assert_eq!(
            envelope_count(account.id).await,
            1,
            "disabled retention must not purge"
        );
    }
}
