use std::collections::HashMap;

use tantivy::schema::Term;
use tantivy::{IndexReader, IndexWriter};

use crate::common::periodic::{PeriodicTask, TaskHandle};
use crate::context::BichonTask;
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use crate::store::tantivy::attachment::ATTACHMENT_MANAGER;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::store::tantivy::fields::{
    F_ACCOUNT_ID, F_CONTENT_HASH, F_ID, F_INGEST_AT, F_MAILBOX_ID, F_SHARD_ID,
};
use crate::store::tantivy::schema::SchemaTools;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A single document candidate for deduplication.
/// Holds just enough information to compare and delete duplicates.
struct DedupEntry {
    /// Unix timestamp (seconds) when this document was ingested.
    /// Used to determine which copy to keep: we always keep the latest,
    /// so that a post-uidvalidity-reset uid is preferred over a stale one.
    ingest_at: i64,
    /// The email's f_id value, used to delete the duplicate email (via term
    /// query on f_id) and to cascade-delete attachments whose f_envelope_id
    /// matches this id.
    email_id: String,
}

/// Dedup map for one account.
/// Key   = (mailbox_id, content_hash)  — stable identity across uidvalidity resets
/// Value = all documents sharing that key, to be reduced to exactly one.
type DedupMap = HashMap<(u64, String), Vec<DedupEntry>>;

pub(crate) const UIDONLY_SHARD_BIT: u64 = 1 << 63;

pub(crate) fn is_uidonly_shard(shard_id: u64) -> bool {
    shard_id & UIDONLY_SHARD_BIT != 0
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Background deduplication task.
///
/// Iterates over every account found in the index and removes duplicate emails
/// within each (mailbox_id, content_hash) group, keeping the most recently
/// ingested copy.
///
/// For each duplicate email removed, all attachments in the attachment index
/// whose f_envelope_id matches the removed email's f_id are also deleted.
///
/// Why keep the *latest* ingest_at?
///   After a uidvalidity reset the server reassigns UIDs. If we kept an old
///   copy (lower ingest_at) its uid would be stale, and uid-based incremental
///   sync would re-download emails that are already present.
///
/// Processing is done account-by-account so that peak memory is bounded by
/// the largest single account rather than the entire index.
pub async fn dedup_task(
    email_reader: &IndexReader,
    email_writer: &mut IndexWriter,
    attachment_writer: &mut IndexWriter,
) -> BichonResult<()> {
    let account_ids = collect_account_ids(email_reader)?;
    let mut total_deleted = 0u64;

    for account_id in account_ids {
        total_deleted += dedup_account(email_reader, email_writer, attachment_writer, account_id)?;
    }

    tracing::info!("dedup: finished, total removed={}", total_deleted);
    Ok(())
}

// ─── Periodic task ──────────────────────────────────────────────────────────

const DEDUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(12 * 60 * 60);

/// Periodically scans the email index for duplicate (mailbox_id, content_hash)
/// entries and removes redundant copies, keeping the most recently ingested one.
/// Attachments belonging to removed emails are cascade-deleted from the
/// attachment index.
pub struct DedupTask;

impl BichonTask for DedupTask {
    fn start() -> TaskHandle {
        let periodic_task = PeriodicTask::new("index-dedup");

        let task = move |_: Option<u64>| {
            Box::pin(async move {
                // Acquire both writers before creating a reader. The fresh reader
                // sees the last committed state, while the writers ensure we have
                // exclusive access to perform deletions.
                let mut email_writer = ENVELOPE_MANAGER.index_writer().lock().await;
                let mut attach_writer = ATTACHMENT_MANAGER.index_writer().lock().await;
                let email_reader = ENVELOPE_MANAGER.create_reader()?;

                dedup_task(&email_reader, &mut email_writer, &mut attach_writer).await?;

                // Commit any remaining changes from the dedup pass.
                // dedup_account commits per-account, but we ensure a final commit
                // so the attachment index is in sync.
                crate::store::tantivy::fatal_commit(&mut attach_writer);

                drop(attach_writer);
                drop(email_writer);
                Ok(())
            })
        };

        periodic_task.start(task, None, DEDUP_INTERVAL, false, false)
    }
}

// ─── Internals ────────────────────────────────────────────────────────────────

/// Collect the distinct set of account_ids present in the index.
///
/// Scans only the account_id FAST column — no stored field reads, no I/O
/// beyond the column file itself.
fn collect_account_ids(reader: &IndexReader) -> BichonResult<Vec<u64>> {
    let searcher = reader.searcher();
    let mut ids = std::collections::HashSet::new();

    for segment_reader in searcher.segment_readers() {
        let account_col = segment_reader
            .fast_fields()
            .u64(F_ACCOUNT_ID)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let max_doc = segment_reader.max_doc();
        for doc_id in 0..max_doc {
            // Skip documents that have already been soft-deleted
            if segment_reader.is_deleted(doc_id) {
                continue;
            }
            ids.insert(account_col.values.get_val(doc_id));
        }
    }
    Ok(ids.into_iter().collect())
}

/// Deduplicate all emails for a single account, and cascade-delete their attachments.
///
/// Strategy:
///   1. Scan FAST columns for (mailbox_id, content_hash, ingest_at, f_id) — no heap reads.
///   2. Group by (mailbox_id, content_hash).
///   3. Within each group, sort descending by ingest_at and soft-delete all
///      but the first (most recent) entry.
///   4. For each removed email, delete all attachments in the attachment index
///      whose f_envelope_id matches the removed email's f_id.
///   5. Commit both writers once per account so memory is released before the
///      next account is processed.
///
/// Peak memory for this function ≈ account_email_count × ~160 bytes
/// (the extra ~80 bytes over previous version comes from storing email_id strings).
fn dedup_account(
    email_reader: &IndexReader,
    email_writer: &mut IndexWriter,
    attachment_writer: &mut IndexWriter,
    account_id: u64,
) -> BichonResult<u64> {
    let searcher = email_reader.searcher();
    let fields = SchemaTools::email_fields();
    // eprintln!(
    //     "DEBUG dedup_account: entry account={account_id} f_id_field={:?} f_content_hash_field={:?}",
    //     fields.f_id, fields.f_content_hash
    // );
    let mut map: DedupMap = HashMap::new();

    // ── Phase 1: build the dedup map via FAST column scans ──────────────────
    for segment_reader in searcher.segment_readers() {
        let account_col = segment_reader
            .fast_fields()
            .u64(F_ACCOUNT_ID)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mailbox_col = segment_reader
            .fast_fields()
            .u64(F_MAILBOX_ID)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let ingest_col = segment_reader
            .fast_fields()
            .i64(F_INGEST_AT)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let shard_col = segment_reader
            .fast_fields()
            .u64(F_SHARD_ID)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        // content_hash and f_id are text fields with FAST; stored as dictionary-encoded strings
        let hash_col = segment_reader
            .fast_fields()
            .str(F_CONTENT_HASH)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(|| raise_error!(format!("FAST str column '{}' not found in segment; ensure the field is declared with FAST in the schema", F_CONTENT_HASH), ErrorCode::InternalError))?;
        let id_col = segment_reader
            .fast_fields()
            .str(F_ID)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .ok_or_else(|| raise_error!(format!("FAST str column '{}' not found in segment; ensure the field is declared with FAST in the schema", F_ID), ErrorCode::InternalError))?;

        let max_doc = segment_reader.max_doc();
        for doc_id in 0..max_doc {
            if segment_reader.is_deleted(doc_id) {
                continue;
            }
            // Filter to the current account without touching stored fields
            if account_col.values.get_val(doc_id) != account_id {
                continue;
            }
            // UIDONLY records represent server UIDs, not unique bodies.
            if is_uidonly_shard(shard_col.values.get_val(doc_id)) {
                continue;
            }

            let mailbox_id = mailbox_col.values.get_val(doc_id);
            let ingest_at = ingest_col.values.get_val(doc_id);

            // Read content_hash from the dictionary-encoded string column
            let hash_ord = hash_col
                .ords()
                .values_for_doc(doc_id as u32)
                .next()
                .unwrap_or(0);
            let mut hash_buf = String::new();
            hash_col
                .ord_to_str(hash_ord, &mut hash_buf)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let content_hash = hash_buf;

            // Read f_id from the dictionary-encoded string column
            let id_ord = id_col
                .ords()
                .values_for_doc(doc_id as u32)
                .next()
                .unwrap_or(0);
            let mut id_buf = String::new();
            id_col
                .ord_to_str(id_ord, &mut id_buf)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let email_id = id_buf;

            // eprintln!(
            //     "DEBUG dedup_account: account={account_id} doc_id={doc_id} mailbox={mailbox_id} hash={content_hash:?} id={email_id:?} ingest_at={ingest_at}"
            // );

            map.entry((mailbox_id, content_hash))
                .or_default()
                .push(DedupEntry {
                    ingest_at,
                    email_id,
                });
        }
    }

    // ── Phase 2: delete duplicate emails and their attachments ───────────────
    let attachment_fields = SchemaTools::attachment_fields();
    let mut deleted = 0u64;

    for (_key, mut entries) in map {
        if entries.len() <= 1 {
            // No duplicates in this group
            continue;
        }

        // Sort descending: the most recently ingested document comes first.
        // This ensures we keep the copy whose uid reflects the current
        // uidvalidity, which is required for correct incremental sync.
        entries.sort_by_key(|e| std::cmp::Reverse(e.ingest_at));

        tracing::debug!(
            "dedup: account={} mailbox={} hash={}: {} copies, keeping eid={} ingest_at={}, deleting {}",
            account_id,
            _key.0,
            &_key.1,
            entries.len(),
            &entries[0].email_id,
            entries[0].ingest_at,
            entries.len() - 1
        );

        // eprintln!(
        //     "DEBUG Phase2: key={_key:?} kept={} deleting={}",
        //     entries[0].email_id,
        //     entries.len() - 1
        // );
        // Keep entries[0], soft-delete everything else via term query on f_id
        for entry in &entries[1..] {
            eprintln!(
                "DEBUG Phase2: delete_term f_id={:?} text=\"{}\"",
                fields.f_id, &entry.email_id
            );
            // Remove the duplicate email from the email index
            let email_term = Term::from_field_text(fields.f_id, &entry.email_id);
            email_writer.delete_term(email_term);

            // Cascade: remove all attachments belonging to this email.
            // f_envelope_id in the attachment index mirrors f_id in the email index.
            let envelope_term =
                Term::from_field_text(attachment_fields.f_envelope_id, &entry.email_id);
            attachment_writer.delete_term(envelope_term);

            deleted += 1;
        }
    }

    // Commit both indexes once per account so the DedupMap memory for this
    // account can be reclaimed before the next account is processed.
    if deleted > 0 {
        email_writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        attachment_writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        tracing::info!("dedup: account={} removed={}", account_id, deleted);
    }

    Ok(deleted)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tantivy::fields::AttachmentFields;
    use crate::store::tantivy::fields::EmailFields;
    use crate::store::tantivy::schema::SchemaTools;
    use crate::store::tantivy::tokenizers::EuroTokenizer;
    use std::collections::HashSet;
    use std::fmt::Write;
    use std::fs;
    use tantivy::Index;
    use tantivy::TantivyDocument;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("bichon-dedup-test")
            .join(prefix)
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_index(dir: &std::path::Path, schema: tantivy::schema::Schema) -> Index {
        let index = Index::create_in_dir(dir, schema).unwrap();
        index.tokenizers().register("euro", EuroTokenizer::new());
        index
    }

    /// Collect non-deleted f_id values from the email index.
    fn surviving_email_ids(reader: &IndexReader) -> HashSet<String> {
        reader.reload().expect("reader reload failed");
        let searcher = reader.searcher();
        let mut ids = HashSet::new();
        let segments = searcher.segment_readers();
        eprintln!(
            "DEBUG surviving_email_ids: segment_count={}",
            segments.len()
        );
        for (seg_idx, seg) in segments.iter().enumerate() {
            let id_col = seg
                .fast_fields()
                .str(F_ID)
                .unwrap()
                .expect("FAST str column 'id' missing");
            let max_doc = seg.max_doc();
            eprintln!("DEBUG surviving_email_ids: seg={seg_idx} max_doc={max_doc}");
            for doc_id in 0..max_doc {
                let is_del = seg.is_deleted(doc_id);
                let ord = id_col
                    .ords()
                    .values_for_doc(doc_id as u32)
                    .next()
                    .unwrap_or(0);
                let mut buf = String::new();
                id_col.ord_to_str(ord, &mut buf).unwrap();
                eprintln!("DEBUG surviving_email_ids: seg={seg_idx} doc_id={doc_id} is_deleted={is_del} ord={ord} buf={buf:?}");
                if !is_del {
                    ids.insert(buf);
                }
            }
        }
        ids
    }

    /// Collect non-deleted f_id values from the attachment index.
    fn surviving_attachment_ids(reader: &IndexReader) -> HashSet<String> {
        let searcher = reader.searcher();
        let mut ids = HashSet::new();
        for seg in searcher.segment_readers() {
            let id_col = seg
                .fast_fields()
                .str(F_ID)
                .unwrap()
                .expect("FAST str column 'id' missing in attachment index");
            let max_doc = seg.max_doc();
            for doc_id in 0..max_doc {
                if seg.is_deleted(doc_id) {
                    continue;
                }
                let ord = id_col
                    .ords()
                    .values_for_doc(doc_id as u32)
                    .next()
                    .unwrap_or(0);
                let mut buf = String::new();
                id_col.ord_to_str(ord, &mut buf).unwrap();
                ids.insert(buf);
            }
        }
        ids
    }

    fn add_email(
        f: &EmailFields,
        w: &mut IndexWriter,
        id: &str,
        account: u64,
        mailbox: u64,
        hash: &str,
        ingest_at: i64,
    ) {
        add_email_with_shard(f, w, id, account, mailbox, hash, ingest_at, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn add_email_with_shard(
        f: &EmailFields,
        w: &mut IndexWriter,
        id: &str,
        account: u64,
        mailbox: u64,
        hash: &str,
        ingest_at: i64,
        shard_id: u64,
    ) {
        let mut doc = TantivyDocument::new();
        doc.add_text(f.f_id, id);
        doc.add_u64(f.f_account_id, account);
        doc.add_u64(f.f_mailbox_id, mailbox);
        doc.add_text(f.f_content_hash, hash);
        doc.add_i64(f.f_ingest_at, ingest_at);
        doc.add_u64(f.f_shard_id, shard_id);
        w.add_document(doc).unwrap();
    }

    fn add_attachment(
        f: &AttachmentFields,
        w: &mut IndexWriter,
        id: &str,
        envelope_id: &str,
        account: u64,
        mailbox: u64,
    ) {
        let mut doc = TantivyDocument::new();
        doc.add_text(f.f_id, id);
        doc.add_text(f.f_envelope_id, envelope_id);
        doc.add_u64(f.f_account_id, account);
        doc.add_u64(f.f_mailbox_id, mailbox);
        w.add_document(doc).unwrap();
    }

    /// Prevent segment merges during dedup so delete operations are isolated
    /// and test assertions target the exact expected document set.
    fn apply_no_merge_policy(w: &mut IndexWriter) {
        let mut mp = tantivy::indexer::LogMergePolicy::default();
        mp.set_min_num_segments(500);
        mp.set_max_docs_before_merge(1_000_000);
        w.set_merge_policy(Box::new(mp));
    }

    struct Harness;

    impl Harness {
        async fn run<F>(
            case: &str,
            populate: F,
            expected_emails: &[&str],
            expected_attachments: &[&str],
        ) where
            F: FnOnce(&EmailFields, &mut IndexWriter, &AttachmentFields, &mut IndexWriter),
        {
            let email_schema = SchemaTools::email_schema();
            let attach_schema = SchemaTools::attachment_schema();
            let email_f = SchemaTools::email_fields();
            let attach_f = SchemaTools::attachment_fields();

            let email_idx = make_index(&temp_dir(case), email_schema);
            let attach_idx = make_index(&temp_dir(case), attach_schema);

            let mut email_w = email_idx.writer_with_num_threads(1, 50_000_000).unwrap();
            let mut attach_w = attach_idx.writer_with_num_threads(1, 50_000_000).unwrap();
            apply_no_merge_policy(&mut email_w);
            apply_no_merge_policy(&mut attach_w);

            populate(&email_f, &mut email_w, &attach_f, &mut attach_w);

            email_w.commit().unwrap();
            attach_w.commit().unwrap();
            drop(email_w);
            drop(attach_w);

            let mut email_w2 = email_idx.writer_with_num_threads(1, 50_000_000).unwrap();
            let mut attach_w2 = attach_idx.writer_with_num_threads(1, 50_000_000).unwrap();
            apply_no_merge_policy(&mut email_w2);
            apply_no_merge_policy(&mut attach_w2);

            let email_r = email_idx.reader().unwrap();
            dedup_task(&email_r, &mut email_w2, &mut attach_w2)
                .await
                .unwrap();

            let email_r = email_idx.reader().unwrap();
            let survivors = surviving_email_ids(&email_r);
            let expected: HashSet<String> = expected_emails.iter().map(|s| s.to_string()).collect();
            assert_eq!(survivors, expected, "[{case}] email survivors mismatch");

            let attach_r = attach_idx.reader().unwrap();
            let att_survivors = surviving_attachment_ids(&attach_r);
            let att_expected: HashSet<String> =
                expected_attachments.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                att_survivors, att_expected,
                "[{case}] attachment survivors mismatch"
            );
        }
    }

    #[tokio::test]
    async fn dedup_removes_duplicates_and_cascades_to_attachments() {
        Harness::run(
            "basic",
            |ef, ew, af, aw| {
                add_email(ef, ew, "dup-old", 1, 200, "hash-dup", 1000);
                add_email(ef, ew, "dup-new", 1, 200, "hash-dup", 3000);
                add_email(ef, ew, "unique", 1, 200, "hash-uniq", 1000);
                add_attachment(af, aw, "att-old", "dup-old", 1, 200);
                add_attachment(af, aw, "att-new", "dup-new", 1, 200);
            },
            &["dup-new", "unique"],
            &["att-new"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_preserves_distinct_uidonly_records_with_identical_raw() {
        Harness::run(
            "uidonly-identical-raw",
            |ef, ew, af, aw| {
                add_email_with_shard(ef, ew, "epoch-7-uid-1", 1, 2, "same", 1, UIDONLY_SHARD_BIT);
                add_email_with_shard(ef, ew, "epoch-7-uid-2", 1, 2, "same", 2, UIDONLY_SHARD_BIT);
                add_attachment(af, aw, "att-1", "epoch-7-uid-1", 1, 2);
                add_attachment(af, aw, "att-2", "epoch-7-uid-2", 1, 2);
            },
            &["epoch-7-uid-1", "epoch-7-uid-2"],
            &["att-1", "att-2"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_keeps_latest_among_many_duplicates() {
        Harness::run(
            "many-dups",
            |ef, ew, af, aw| {
                for (i, ts) in [50, 100, 400, 200, 300].iter().enumerate() {
                    let id = format!("dup-{i}");
                    add_email(ef, ew, &id, 1, 1, "H", *ts);
                    add_attachment(af, aw, &format!("att-{i}"), &id, 1, 1);
                }
            },
            &["dup-2"], // ingest_at=400, the latest
            &["att-2"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_no_duplicates_is_noop() {
        Harness::run(
            "no-dups",
            |ef, ew, af, aw| {
                add_email(ef, ew, "a", 1, 1, "hash-a", 100);
                add_email(ef, ew, "b", 1, 1, "hash-b", 200);
                add_email(ef, ew, "c", 1, 1, "hash-c", 300);
                add_attachment(af, aw, "att-a", "a", 1, 1);
                add_attachment(af, aw, "att-b", "b", 1, 1);
                add_attachment(af, aw, "att-c", "c", 1, 1);
            },
            &["a", "b", "c"],
            &["att-a", "att-b", "att-c"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_isolates_accounts() {
        // Same hash, same mailbox, DIFFERENT accounts → no dedup
        Harness::run(
            "cross-account",
            |ef, ew, af, aw| {
                add_email(ef, ew, "acc1-a", 1, 1, "hash-same", 100);
                add_email(ef, ew, "acc1-b", 1, 1, "hash-same", 200);
                add_email(ef, ew, "acc2-a", 2, 1, "hash-same", 100);
                add_email(ef, ew, "acc2-b", 2, 1, "hash-same", 200);
                add_attachment(af, aw, "att-1a", "acc1-a", 1, 1);
                add_attachment(af, aw, "att-1b", "acc1-b", 1, 1);
                add_attachment(af, aw, "att-2a", "acc2-a", 2, 1);
                add_attachment(af, aw, "att-2b", "acc2-b", 2, 1);
            },
            // Each account keeps its latest: acc1 keeps acc1-b (200>100), acc2 keeps acc2-b
            &["acc1-b", "acc2-b"],
            &["att-1b", "att-2b"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_isolates_mailboxes() {
        // Same hash, same account, DIFFERENT mailboxes → no dedup
        Harness::run(
            "cross-mailbox",
            |ef, ew, af, aw| {
                add_email(ef, ew, "mb1-a", 1, 1, "hash-same", 100);
                add_email(ef, ew, "mb2-a", 1, 2, "hash-same", 100);
                add_email(ef, ew, "mb1-b", 1, 1, "hash-same", 200);
                add_email(ef, ew, "mb2-b", 1, 2, "hash-same", 200);
                add_attachment(af, aw, "att-1a", "mb1-a", 1, 1);
                add_attachment(af, aw, "att-1b", "mb1-b", 1, 1);
                add_attachment(af, aw, "att-2a", "mb2-a", 1, 2);
                add_attachment(af, aw, "att-2b", "mb2-b", 1, 2);
            },
            &["mb1-b", "mb2-b"],
            &["att-1b", "att-2b"],
        )
        .await;
    }

    #[tokio::test]
    async fn dedup_multiple_attachments_per_email() {
        // Deleting an email cascades all its attachments, not just one
        Harness::run(
            "multi-att",
            |ef, ew, af, aw| {
                add_email(ef, ew, "old", 1, 1, "H", 100);
                add_email(ef, ew, "new", 1, 1, "H", 200);
                // The old email has 3 attachments — all should be removed
                add_attachment(af, aw, "att1", "old", 1, 1);
                add_attachment(af, aw, "att2", "old", 1, 1);
                add_attachment(af, aw, "att3", "old", 1, 1);
                // The kept email has 2 attachments — both should survive
                add_attachment(af, aw, "att4", "new", 1, 1);
                add_attachment(af, aw, "att5", "new", 1, 1);
            },
            &["new"],
            &["att4", "att5"],
        )
        .await;
    }

    /// Inspects the production email index and reports duplicate counts.
    ///
    /// A "duplicate" is defined as two or more emails sharing the same
    /// (account_id, mailbox_id, content_hash) tuple.
    ///
    /// This test is read-only — it does not modify the index.
    #[test]
    fn inspect_production_duplicates() {
        let index_path = r"E:\bichon-data\bichon-indices\mail_metadata";
        let report_path = std::path::PathBuf::from(r"E:\bichon\dedup_report.txt");

        let mut report = String::new();
        let _ = writeln!(report, "opening index at {index_path}...");

        let index = match Index::open_in_dir(index_path) {
            Ok(idx) => {
                let _ = writeln!(report, "index opened successfully");
                idx
            }
            Err(e) => {
                let _ = writeln!(report, "Failed to open index at {index_path}: {e}");
                let _ = std::fs::write(&report_path, &report);
                return;
            }
        };

        let reader = match index.reader() {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(report, "Failed to create reader: {e}");
                let _ = std::fs::write(&report_path, &report);
                return;
            }
        };

        reader.reload().expect("reader reload failed");
        let searcher = reader.searcher();

        let mut total_docs = 0u64;
        let mut groups: std::collections::HashMap<
            u64,
            std::collections::HashMap<(u64, String), u64>,
        > = std::collections::HashMap::new();

        for segment_reader in searcher.segment_readers() {
            let account_col = segment_reader.fast_fields().u64(F_ACCOUNT_ID).unwrap();
            let mailbox_col = segment_reader.fast_fields().u64(F_MAILBOX_ID).unwrap();
            let hash_col = match segment_reader.fast_fields().str(F_CONTENT_HASH).unwrap() {
                Some(c) => c,
                None => {
                    let _ = writeln!(
                        report,
                        "Segment has no FAST str column for content_hash, skipping"
                    );
                    continue;
                }
            };

            let max_doc = segment_reader.max_doc();
            for doc_id in 0..max_doc {
                if segment_reader.is_deleted(doc_id) {
                    continue;
                }

                let account_id = account_col.values.get_val(doc_id);
                let mailbox_id = mailbox_col.values.get_val(doc_id);

                let hash_ord = hash_col
                    .ords()
                    .values_for_doc(doc_id as u32)
                    .next()
                    .unwrap_or(0);
                let mut hash_buf = String::new();
                hash_col.ord_to_str(hash_ord, &mut hash_buf).unwrap();
                let content_hash = hash_buf;

                total_docs += 1;
                groups
                    .entry(account_id)
                    .or_default()
                    .entry((mailbox_id, content_hash))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            }
        }

        // ── Summarize ──────────────────────────────────────────────────────────
        let mut total_duplicate_groups = 0u64;
        let mut total_duplicate_emails = 0u64;

        for (account_id, account_groups) in &groups {
            let mut account_dup_groups = 0u64;
            let mut account_dup_emails = 0u64;
            for ((_mailbox_id, _hash), count) in account_groups {
                if *count > 1 {
                    account_dup_groups += 1;
                    account_dup_emails += count - 1;
                }
            }
            if account_dup_groups > 0 {
                let _ = writeln!(
                    report,
                    "account={account_id}: {account_dup_groups} duplicate groups, {account_dup_emails} redundant emails"
                );
            }
            total_duplicate_groups += account_dup_groups;
            total_duplicate_emails += account_dup_emails;
        }

        let _ = writeln!(
            report,
            "─── Summary ───\n\
             total_docs          = {total_docs}\n\
             accounts            = {}\n\
             duplicate_groups    = {total_duplicate_groups}\n\
             redundant_emails    = {total_duplicate_emails}\n\
             unique_after_dedup  = {}",
            groups.len(),
            total_docs - total_duplicate_emails,
        );

        std::fs::write(&report_path, &report).unwrap();
        println!("report written to {}", report_path.display());
    }
}
