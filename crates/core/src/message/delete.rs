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

use crate::account::migration::AccountModel;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::ext::audit_detail::{
    decide_batch, load_deleted_snapshot, BatchAuditDecision, DetailRef,
};
use crate::ext::event_bus::{emit, Event};
use crate::raise_error;
use crate::store::tantivy::attachment::ATTACHMENT_MANAGER;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use std::collections::HashMap;

pub use crate::ext::audit_detail::DeletedMessageSnapshot;

/// Collect a snapshot for every message about to be deleted.
///
/// Lives in core rather than the REST handler because there is now more than
/// one caller: the community handler and the Pro dual-control executor both
/// delete messages, and both must capture the subject and content summary
/// *before* the messages are gone. A handler-private copy would have meant the
/// approval path silently emitting `email.deleted` records with no subject.
///
/// The per-message loading itself lives in `ext::audit_detail` so the inline
/// path and the detail-file writer share one definition of "the detail".
pub fn audit_snapshots_for_deleted(
    request: &HashMap<u64, Vec<String>>,
) -> Vec<DeletedMessageSnapshot> {
    let mut out = Vec::new();
    for (account_id, envelope_ids) in request {
        for eid in envelope_ids {
            out.push(load_deleted_snapshot(*account_id, eid));
        }
    }
    out
}

/// The plan a delete batch produces for the audit trail: either inline
/// per-message `email.deleted` events (snapshots already captured) or a single
/// `email.batch_deleted` referencing an externalized detail file.
pub enum DeleteAuditPlan {
    Inline(Vec<DeletedMessageSnapshot>),
    Detail(DetailRef),
}

/// Decide the audit shape and capture whatever must be captured *before* the
/// messages are gone. Call this, run the delete, and — only on success — hand
/// the plan to `emit_delete_audit`.
///
/// A delete that succeeds must never be missing from the trail, so the detail
/// file's failure to write degrades to the inline path rather than aborting.
pub fn plan_delete_audit(request: &HashMap<u64, Vec<String>>) -> DeleteAuditPlan {
    match decide_batch(request) {
        BatchAuditDecision::Detail(r) => DeleteAuditPlan::Detail(r),
        BatchAuditDecision::Inline => DeleteAuditPlan::Inline(audit_snapshots_for_deleted(request)),
    }
}

/// Emit the audit records for a delete that has already succeeded. Calling this
/// on a failed delete would write "deleted" records for messages that still
/// exist — the two call sites are the only enforcement of that ordering.
pub fn emit_delete_audit(plan: DeleteAuditPlan, user: &str) {
    emit_delete_audit_scoped(plan, user, None, None);
}

/// Same as [`emit_delete_audit`], additionally recording which batch job ran
/// the delete and the search filter that produced its id set.
///
/// A batch can span many HTTP requests (and, under dual control, two people),
/// so "who deleted these messages" is incomplete without "under which search".
/// Single deletes pass `None` for both: their ids were chosen explicitly.
pub fn emit_delete_audit_scoped(
    plan: DeleteAuditPlan,
    user: &str,
    job_id: Option<&str>,
    filter: Option<&crate::message::search::EmailSearchFilter>,
) {
    let job_id = job_id.map(str::to_string);
    let filter = filter.cloned();
    match plan {
        DeleteAuditPlan::Inline(snapshots) => {
            for s in snapshots {
                emit(Event::EmailDeleted {
                    email_id: s.email_id,
                    user: user.to_string(),
                    account_id: s.account_id,
                    mailbox_id: s.mailbox_id,
                    subject: s.subject,
                    snapshot: s.snapshot,
                    job_id: job_id.clone(),
                    filter: filter.clone(),
                });
            }
        }
        DeleteAuditPlan::Detail(r) => {
            emit(Event::EmailBatchDeleted {
                user: user.to_string(),
                account_ids: r.account_ids,
                count: r.count,
                batch_id: r.batch_id,
                detail_file: r.rel_path,
                detail_size: r.size,
                detail_sha256: r.sha256,
                job_id,
                filter,
            });
        }
    }
}

/// Remember the source UIDs of the messages about to be deleted so gap-fill
/// and full re-syncs do not fetch them again.
///
/// Written *before* the delete: a crash can leave a tombstone for a message
/// that still exists locally (harmless — gap-fill only consults it when the
/// message is missing), but never a committed deletion without one.
async fn record_source_tombstones(request: &HashMap<u64, Vec<String>>) -> BichonResult<()> {
    for (account_id, ids) in request {
        let mut by_mailbox: HashMap<u64, Vec<u32>> = HashMap::new();
        // One term per id: chunk large REST batches so the query stays bounded.
        for chunk in ids.chunks(500) {
            let refs = ENVELOPE_MANAGER.get_source_refs(*account_id, chunk)?;
            for r in refs {
                if r.uid == 0 || r.mailbox_id == 0 {
                    continue;
                }
                by_mailbox.entry(r.mailbox_id).or_default().push(r.uid);
            }
        }
        record_tombstones_by_mailbox(*account_id, by_mailbox).await?;
    }
    Ok(())
}

/// Record source-UID tombstones from `(mailbox_id, uid)` pairs. Shared by the
/// manual delete path (which resolves ids through the index) and the retention
/// sweep (which already captured the pairs before deleting the documents).
pub async fn record_tombstones_by_mailbox(
    account_id: u64,
    by_mailbox: HashMap<u64, Vec<u32>>,
) -> BichonResult<()> {
    for (mailbox_id, uids) in by_mailbox {
        // A missing mailbox row means there is no remote folder left to
        // re-download from; skip rather than failing the delete.
        let Some(mailbox) =
            crate::archive::imap::mailbox::MailBox::find_mailbox(account_id, mailbox_id)?
        else {
            continue;
        };
        let Some(uid_validity) = mailbox.uid_validity else {
            continue;
        };
        crate::archive::imap::tombstones::record_uids(account_id, mailbox_id, uid_validity, &uids)
            .await?;
    }
    Ok(())
}

pub async fn delete_messages_impl(request: HashMap<u64, Vec<String>>) -> BichonResult<()> {
    delete_messages_impl_with(request, false).await
}

/// Like [`delete_messages_impl`], but optionally allows messages inside the
/// minimum-retention window to be deleted. The only caller that passes `true`
/// is the GDPR Art. 17 erasure path, whose request already records the data
/// subject, the identification criteria and the legal assessment that justifies
/// erasing despite a statutory retention duty.
pub async fn delete_messages_impl_with(
    request: HashMap<u64, Vec<String>>,
    allow_min_retention_override: bool,
) -> BichonResult<()> {
    // Legal hold: an account under a hold is frozen — no message may be
    // deleted from it, including manual single/batch deletes from the web UI
    // (retention and auto-expunge already skip held accounts at their own call
    // sites; this closes the manual-delete path). The check refuses the whole
    // request if any target account is held, so a batch can never partially
    // delete. In the community edition `legal_hold` is always false, so this
    // guard is a no-op there.
    for account_id in request.keys() {
        let account = AccountModel::get(*account_id)?;
        if account.is_on_hold() {
            return Err(raise_error!(
                format!(
                    "Account {} ({}) is under a legal hold; deleting its messages is disabled while the hold is active",
                    account.id, account.email
                ),
                ErrorCode::Forbidden
            ));
        }
    }

    // Minimum retention (Enterprise deletion lock): a message whose effective
    // date is still inside the window cannot be deleted by any ordinary path.
    // Check every account before deleting anything so a mixed request never
    // partially applies. The batch job filters locked ids up front; this is
    // the second, authoritative check.
    if !allow_min_retention_override {
        for (account_id, ids) in &request {
            let account = AccountModel::get(*account_id)?;
            let Some(cutoff) = crate::retention::min_retention_cutoff_ms(&account) else {
                continue;
            };
            // Chunk the index lookup: one term per id, so a page-sized REST
            // batch stays bounded instead of building a query with N clauses.
            let mut locked = Vec::new();
            for chunk in ids.chunks(500) {
                locked.extend(ENVELOPE_MANAGER.min_retention_locked_ids(
                    *account_id,
                    chunk,
                    cutoff,
                )?);
            }
            if !locked.is_empty() {
                return Err(raise_error!(
                    format!(
                        "Account {} ({}) has {} message(s) inside its {}-day minimum retention window; they cannot be deleted",
                        account.id,
                        account.email,
                        locked.len(),
                        account.min_retention_days_effective()
                    ),
                    ErrorCode::Forbidden
                ));
            }
        }
    }

    record_source_tombstones(&request).await?;
    ENVELOPE_MANAGER
        .delete_envelopes_multi_account(request.clone())
        .await?;
    ATTACHMENT_MANAGER
        .delete_attachments_multi_account(request)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use super::*;
    use crate::account::migration::{AccountModel, AccountType};
    use crate::account::payload::AccountCreateRequest;
    use crate::archive::imap::mailbox::MailBox;
    use crate::archive::imap::tombstones;
    use crate::database::{insert_impl, manager::DB_MANAGER};
    use crate::settings::cli::SETTINGS;
    use crate::settings::dir::DATA_DIR_MANAGER;
    use crate::store::tantivy::schema::SchemaTools;
    use tantivy::TantivyDocument;

    static ENV: Once = Once::new();

    fn init_test_env() {
        ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
            let _ = &*ENVELOPE_MANAGER;
        });
    }

    fn insert_account(id: u64, email: &str) -> AccountModel {
        insert_account_with_min_retention(id, email, None)
    }

    fn insert_account_with_min_retention(
        id: u64,
        email: &str,
        min_retention_days: Option<u64>,
    ) -> AccountModel {
        let request = AccountCreateRequest {
            email: email.to_string(),
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
            min_retention_days,
            remote_cleanup: None,
            kind: None,
            view: None,
        };
        let mut account = AccountModel::new(id, request).unwrap();
        // `AccountModel::new` assigns its own id; tests need the id they chose
        // so the fixture rows line up.
        account.id = id;
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();
        account
    }

    async fn write_doc(id: &str, account_id: u64, mailbox_id: u64, uid: u32) {
        write_doc_dated(id, account_id, mailbox_id, uid, 1).await
    }

    async fn write_doc_dated(
        id: &str,
        account_id: u64,
        mailbox_id: u64,
        uid: u32,
        date: i64,
    ) {
        let f = SchemaTools::email_fields();
        let mut doc = TantivyDocument::new();
        doc.add_text(f.f_id, id);
        doc.add_text(f.f_message_id, format!("{id}@test"));
        doc.add_u64(f.f_account_id, account_id);
        doc.add_u64(f.f_mailbox_id, mailbox_id);
        doc.add_u64(f.f_uid, uid as u64);
        doc.add_u64(f.f_imap_uid, uid as u64 + 1000);
        doc.add_i64(f.f_date, date);
        doc.add_i64(f.f_internal_date, date);
        doc.add_u64(f.f_size, 10);
        let mut writer = ENVELOPE_MANAGER.index_writer().lock().await;
        writer.add_document(doc).unwrap();
        writer.commit().unwrap();
    }

    fn mailbox(id: u64, account_id: u64, uid_validity: Option<u32>) -> MailBox {
        MailBox {
            id,
            account_id,
            name: format!("INBOX-{id}"),
            uid_validity,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn delete_records_source_uid_tombstone() {
        init_test_env();
        let _index_guard = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;
        let account_id = 92_001;
        let mailbox_id = 92_002;
        insert_account(account_id, "tombstone-delete@test.example");
        insert_impl(DB_MANAGER.db(), mailbox(mailbox_id, account_id, Some(77))).unwrap();
        write_doc("tombstone-env-1", account_id, mailbox_id, 42).await;

        let mut request = HashMap::new();
        request.insert(account_id, vec!["tombstone-env-1".to_string()]);
        delete_messages_impl(request).await.unwrap();

        let recorded = tombstones::load(account_id, mailbox_id)
            .unwrap()
            .expect("deletion must leave a tombstone");
        assert_eq!(recorded.uid_validity, 77);
        assert!(recorded.bitmap.contains(42));
        assert_eq!(recorded.bitmap.len(), 1);

        // A message that never came from IMAP (uid 0) has no remote copy and
        // must not produce a tombstone entry.
        write_doc("tombstone-env-2", account_id, mailbox_id, 0).await;
        let mut request = HashMap::new();
        request.insert(account_id, vec!["tombstone-env-2".to_string()]);
        delete_messages_impl(request).await.unwrap();
        let recorded = tombstones::load(account_id, mailbox_id).unwrap().unwrap();
        assert_eq!(recorded.bitmap.len(), 1);

        tombstones::remove_account(account_id).await.unwrap();
    }

    #[tokio::test]
    async fn delete_skips_tombstones_for_mailboxes_without_uid_validity() {
        init_test_env();
        let _index_guard = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;
        let account_id = 92_011;
        let mailbox_id = 92_012;
        insert_account(account_id, "no-validity@test.example");
        insert_impl(DB_MANAGER.db(), mailbox(mailbox_id, account_id, None)).unwrap();
        write_doc("no-validity-env", account_id, mailbox_id, 7).await;

        let mut request = HashMap::new();
        request.insert(account_id, vec!["no-validity-env".to_string()]);
        delete_messages_impl(request).await.unwrap();
        assert!(tombstones::load(account_id, mailbox_id).unwrap().is_none());
    }

    /// A minimum-retention lock refuses a message inside the window and lets
    /// an older message through; a message with no timestamp is treated as
    /// locked (conservative — there is no proof it is old enough).
    #[tokio::test]
    async fn min_retention_blocks_young_messages_only() {
        init_test_env();
        let _index_guard = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;
        let account_id = 92_021;
        let mailbox_id = 92_022;
        insert_account_with_min_retention(
            account_id,
            "min-retention@test.example",
            Some(30),
        );
        insert_impl(DB_MANAGER.db(), mailbox(mailbox_id, account_id, Some(11))).unwrap();

        let now = crate::utc_now!();
        write_doc_dated("locked-recent", account_id, mailbox_id, 1, now).await;
        write_doc_dated("locked-unknown", account_id, mailbox_id, 2, 0).await;
        write_doc_dated("unlocked-old", account_id, mailbox_id, 3, 1).await;

        let mut request = HashMap::new();
        request.insert(account_id, vec!["locked-recent".to_string()]);
        let err = delete_messages_impl(request).await.unwrap_err();
        assert!(err.to_string().contains("minimum retention"), "got: {err}");

        let mut request = HashMap::new();
        request.insert(account_id, vec!["locked-unknown".to_string()]);
        let err = delete_messages_impl(request).await.unwrap_err();
        assert!(err.to_string().contains("minimum retention"), "got: {err}");

        let mut request = HashMap::new();
        request.insert(account_id, vec!["unlocked-old".to_string()]);
        delete_messages_impl(request).await.unwrap();

        tombstones::remove_account(account_id).await.unwrap();
    }
}
