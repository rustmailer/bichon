//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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

use std::{
    collections::{HashMap, HashSet},
    ops::Bound,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::{
    account::{migration::AccountModel, stats::AccountStats},
    backup::gate::{BACKUP_ACQUIRE_TIMEOUT, WRITE_GATE},
    common::{paginated::DataPage, signal::SIGNAL_MANAGER},
    dashboard::{DashboardStats, Group, LargestEmail, TimeBucket},
    error::{code::ErrorCode, BichonResult},
    ext::timestamp::LeafRecord,
    message::{
        search::{EmailSearchFilter, SortBy},
        tags::{TagAction, TagCount, TagsRequest},
    },
    raise_error,
    settings::dir::DATA_DIR_MANAGER,
    store::{
        blob::BLOB_MANAGER,
        envelope::Envelope,
        tantivy::{
            attachment::ATTACHMENT_MANAGER,
            dedup_cache::DEDUP_CACHE,
            fatal_commit,
            fields::{
                F_ACCOUNT_ID, F_DATE, F_FROM, F_ID, F_INGEST_AT, F_INTERNAL_DATE,
                F_REGULAR_ATTACHMENT_COUNT, F_SIZE, F_TAGS, F_THREAD_ID, F_UID,
            },
            model::{extract_contacts, EnvelopeWithAttachments},
            schema::SchemaTools,
            tokenizers::EuroTokenizer,
            WriterSlot,
        },
    },
    utc_now,
    utils::html::extract_text,
};

use chrono::Utc;
use mail_parser::MessageParser;
use serde_json::json;
use tantivy::{
    aggregation::{
        agg_req::Aggregations,
        agg_result::{
            AggregationResult, AggregationResults, BucketEntries, BucketResult, MetricResult,
        },
        AggregationCollector, Key,
    },
    collector::{Count, DocSetCollector, FacetCollector, TopDocs},
    indexer::{LogMergePolicy, UserOperation},
    query::{AllQuery, BooleanQuery, EmptyQuery, Occur, Query, QueryParser, RangeQuery, TermQuery},
    schema::{IndexRecordOption, Value},
    DocAddress, Index, IndexReader, IndexWriter, Order, TantivyDocument, Term,
};
use tantivy::{schema::Facet, Searcher};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task::{self, JoinHandle},
};
use tracing::{info, warn};

pub static ENVELOPE_MANAGER: LazyLock<IndexManager> = LazyLock::new(IndexManager::new);

/// Lightweight snapshot of a single envelope, used as the local side of
/// the gap-fill diff without materializing full `Envelope` structs.
///
/// Loads every document of the mailbox into memory at once — the caller
/// must not use this for mailboxes too large to hold in a full in-memory
/// pass. Documents without a message-id are excluded by
/// `get_envelope_snapshots_for_mailbox` since the diff relies on
/// message-id / fingerprint matching.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EnvelopeSnapshot {
    pub message_id: String,
    pub uid: u64,
    pub size: u64,
    /// Epoch millis (internal date).
    pub internal_date: i64,
    //pub subject: String,
}

pub struct IndexManager {
    index: Arc<Index>,
    index_writer: Arc<Mutex<WriterSlot>>,
    sender: mpsc::Sender<TantivyDocument>,
    /// One-shot ack channel used by `prepare_for_backup` to force the writer
    /// loop to commit everything queued and wait for merges before replying.
    flush_tx: mpsc::Sender<oneshot::Sender<()>>,
    reader: IndexReader,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl IndexManager {
    pub(crate) fn index_writer(&self) -> &Arc<Mutex<WriterSlot>> {
        &self.index_writer
    }

    pub fn create_reader(&self) -> BichonResult<IndexReader> {
        self.index
            .reader()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    }

    pub async fn shutdown(&self) {
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.take() {
            let _ = handle.await;
        }
    }
    /// Creates the envelope index writer with Bichon's merge policy. Used at
    /// startup and again after a backup flush consumes the writer while waiting
    /// for in-flight merges to drain.
    fn create_index_writer(index: &Index) -> BichonResult<IndexWriter> {
        let mut merge_policy = LogMergePolicy::default();
        merge_policy.set_min_num_segments(25);
        merge_policy.set_min_layer_size(10_000);
        merge_policy.set_max_docs_before_merge(100_000);

        let index_writer = index.writer_with_num_threads(4, 67_108_864).map_err(|e| {
            raise_error!(
                format!("Failed to create IndexWriter: {e:#?}"),
                ErrorCode::InternalError
            )
        })?;
        index_writer.set_merge_policy(Box::new(merge_policy));
        Ok(index_writer)
    }

    pub fn new() -> Self {
        let index = Arc::new(Self::open_or_create_index(&DATA_DIR_MANAGER.envelope_dir));
        index.tokenizers().register("euro", EuroTokenizer::new());

        let index_writer = Self::create_index_writer(&index).unwrap_or_else(|e| {
            panic!(
                "Failed to create IndexWriter with 4 threads and 64MB buffer for {:?}: {}",
                &DATA_DIR_MANAGER.envelope_dir, e
            )
        });
        let index_writer = Arc::new(Mutex::new(WriterSlot::new(index_writer)));
        let reader = index.reader().unwrap_or_else(|e| {
            panic!(
                "Failed to create IndexReader for {:?}: {}",
                &DATA_DIR_MANAGER.envelope_dir, e
            )
        });

        let (sender, mut receiver) = mpsc::channel::<TantivyDocument>(100);
        let (flush_tx, mut flush_rx) = mpsc::channel::<oneshot::Sender<()>>(4);

        let writer = index_writer.clone();
        let index_bg = Arc::clone(&index);
        let handler = task::spawn(async move {
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            let mut commit_interval = tokio::time::interval(Duration::from_secs(60));
            let mut pending_count = 0;
            let commit_threshold = 1000;
            loop {
                tokio::select! {
                    flush = flush_rx.recv() => {
                        match flush {
                            Some(done) => {
                                let mut writer = writer.lock().await;
                                // Drain any queued-but-not-yet-added docs so the
                                // commit below includes everything sent before
                                // the flush request.
                                let mut added = 0;
                                while let Ok(next_doc) = receiver.try_recv() {
                                    match writer.add_document(next_doc) {
                                        Ok(_) => added += 1,
                                        Err(e) => {
                                            eprintln!("[ERROR] Failed to add document: {e:?}");
                                            tracing::error!("Tantivy: Failed to add document: {e:?}");
                                        }
                                    }
                                }
                                pending_count += added;
                                if pending_count > 0 {
                                    tracing::info!(
                                        "Tantivy: backup flush committing {} docs",
                                        pending_count
                                    );
                                    tokio::task::block_in_place(|| fatal_commit(&mut writer));
                                    pending_count = 0;
                                    commit_interval.reset();
                                }
                                // Deterministically drain every in-flight merge so
                                // no segment file changes while the backup copies
                                // the index. `wait_merging_threads` takes ownership
                                // of the writer, so take it out of the slot and
                                // install a fresh writer once merges have drained.
                                // The write gate is paused and we hold the writer
                                // lock, so no other code path can observe the slot
                                // as empty.
                                let writer_owned = writer.take();
                                if let Err(e) = tokio::task::block_in_place(|| {
                                    writer_owned.wait_merging_threads()
                                }) {
                                    // Index is still consistent (all commits are
                                    // durable); a failed merge only means segments
                                    // were not re-packed.
                                    tracing::error!(
                                        "Tantivy: wait_merging_threads failed, index stays consistent: {e:#?}"
                                    );
                                }
                                let fresh = loop {
                                    match IndexManager::create_index_writer(&index_bg) {
                                        Ok(w) => break w,
                                        Err(e) => {
                                            tracing::error!(
                                                "Tantivy: failed to recreate index writer after backup flush: {e:#?}; retrying"
                                            );
                                            tokio::time::sleep(Duration::from_millis(500)).await;
                                        }
                                    }
                                };
                                writer.put(fresh);
                                let _ = done.send(());
                            }
                            None => {
                                tracing::info!("Tantivy: flush channel closed.");
                            }
                        }
                    }
                    maybe_msg = receiver.recv() => {
                        match maybe_msg {
                            Some(doc) => {
                                let mut writer = writer.lock().await;
                                let mut batch_count = 0;
                                match writer.add_document(doc) {
                                    Ok(_) => {
                                        batch_count += 1;
                                    }
                                    Err(e) => {
                                        eprintln!("[ERROR] Failed to add document: {e:?}");
                                        tracing::error!("Tantivy: Failed to add document: {e:?}");
                                    }
                                }
                                while let Ok(next_doc) = receiver.try_recv() {
                                    match writer.add_document(next_doc) {
                                        Ok(_) => batch_count += 1,
                                        Err(e) => {
                                            eprintln!("[ERROR] Failed to add document: {e:?}");
                                            tracing::error!("Tantivy: Failed to add document: {e:?}");
                                        }
                                    }
                                }
                                if batch_count > 0 {
                                    pending_count += batch_count;
                                }
                                if pending_count >= commit_threshold {
                                    tracing::info!(
                                        "Tantivy: Reached threshold ({} docs), committing...",
                                        pending_count
                                    );
                                    tokio::task::block_in_place(|| fatal_commit(&mut writer));
                                    tracing::debug!(
                                        "Tantivy: committed {} docs, pending reset to 0",
                                        pending_count
                                    );
                                    pending_count = 0;
                                    commit_interval.reset();
                                }
                            }
                            None => {
                                tracing::info!("Tantivy: Receiver closed. Finalizing...");
                                if pending_count > 0 {
                                    let mut writer = writer.lock().await;
                                    tokio::task::block_in_place(|| fatal_commit(&mut writer));
                                }
                                break;
                            },
                        }
                    }
                    _ = commit_interval.tick() => {
                        if pending_count > 0 {
                            let mut writer = writer.lock().await;
                            tracing::debug!(
                                "Tantivy: periodic commit ({} docs pending)",
                                pending_count
                            );
                            tokio::task::block_in_place(|| fatal_commit(&mut writer));
                            pending_count = 0;
                        }
                    }
                    _ = shutdown.recv() => {
                        tracing::info!("Tantivy: Shutdown signal received. Performing final commit...");
                        if pending_count > 0 {
                            let mut writer = writer.lock().await;
                            tokio::task::block_in_place(|| fatal_commit(&mut writer));
                        }
                        tracing::info!("Tantivy: Shutdown cleanup complete.");
                        break;
                    }
                }
            }
        });
        Self {
            index,
            index_writer,
            sender,
            flush_tx,
            reader,
            handle: Mutex::new(Some(handler)),
        }
    }

    /// Commit every queued document and wait for in-progress merges to finish,
    /// leaving the index directory in a copy-safe state. Called by the backup
    /// manager inside a write window (nothing is queued while we wait, but
    /// anything sent just before the window closed is still flushed here).
    pub async fn prepare_for_backup(&self) -> BichonResult<()> {
        let (done_tx, done_rx) = oneshot::channel();
        self.flush_tx
            .send(done_tx)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        done_rx
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    }

    pub async fn queue(&self, doc: TantivyDocument) {
        if let Err(e) = self.sender.send(doc).await {
            tracing::warn!(error = %e, "Failed to queue document into Tantivy writer channel");
        }
    }

    fn open_or_create_index(index_dir: &PathBuf) -> Index {
        let need_create = !index_dir.exists()
            || index_dir
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true);
        if need_create {
            info!(
                "Email index not found or empty, creating new index at {}",
                index_dir.display()
            );
            std::fs::create_dir_all(&index_dir).unwrap_or_else(|e| {
                panic!("Failed to create index directory {:?}: {}", index_dir, e)
            });
            Index::create_in_dir(&index_dir, SchemaTools::email_schema())
                .unwrap_or_else(|e| panic!("Failed to create index in {:?}: {}", index_dir, e))
        } else {
            info!("Opening existing email index at {}", index_dir.display());
            Self::open(&index_dir)
        }
    }

    fn open(index_dir: &PathBuf) -> Index {
        Index::open_in_dir(index_dir)
            .unwrap_or_else(|e| panic!("Failed to open index in {:?}: {}", index_dir, e))
    }

    fn account_query(&self, account_id: u64) -> Box<TermQuery> {
        let account_term =
            Term::from_field_u64(SchemaTools::email_fields().f_account_id, account_id);
        Box::new(TermQuery::new(account_term, IndexRecordOption::Basic))
    }

    pub(crate) fn mailbox_query(&self, account_id: u64, mailbox_id: u64) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::email_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let mailbox_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::email_fields().f_mailbox_id, mailbox_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(mailbox_query)),
        ]);
        Box::new(boolean_query)
    }

    /// Return all Message-IDs stored in Tantivy for a given mailbox.
    /// Prefer `mailbox_contains_message_id` for existence checks on large
    /// mailboxes — this method loads everything into a HashSet.
    pub fn get_message_ids_for_mailbox(
        &self,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<HashSet<String>> {
        let query = self.mailbox_query(account_id, mailbox_id);
        let fields = SchemaTools::email_fields();
        let searcher = self.create_searcher()?;

        let docs = searcher
            .search(&query, &DocSetCollector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut result = HashSet::new();
        for doc_address in docs {
            let doc = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            if let Some(v) = doc.get_first(fields.f_message_id) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        result.insert(s.to_string());
                    }
                }
            }
        }
        Ok(result)
    }

    /// Returns lightweight snapshots of every envelope stored for a mailbox:
    /// message-id, uid, size, internal date (epoch millis), subject.
    /// Used by gap-fill to compute the local side of the diff without
    /// materializing full `Envelope` structs.
    pub fn get_envelope_snapshots_for_mailbox(
        &self,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<Vec<EnvelopeSnapshot>> {
        let query = self.mailbox_query(account_id, mailbox_id);
        let fields = SchemaTools::email_fields();
        let searcher = self.create_searcher()?;

        let docs = searcher
            .search(&query, &DocSetCollector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut snapshots = Vec::with_capacity(docs.len());
        for doc_address in docs {
            let doc = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let message_id = doc
                .get_first(fields.f_message_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Skip documents without a message-id: they are useless for
            // gap-fill (the diff matches on message-id / fingerprint) and
            // would otherwise surface as spurious "missing" entries in the
            // difference set. Other fields keep their fallback defaults.
            if message_id.is_empty() {
                continue;
            }
            let uid = doc
                .get_first(fields.f_uid)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let size = doc
                .get_first(fields.f_size)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let internal_date = doc
                .get_first(fields.f_internal_date)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            // let subject = doc
            //     .get_first(fields.f_subject)
            //     .and_then(|v| v.as_str())
            //     .unwrap_or("")
            //     .to_string();
            snapshots.push(EnvelopeSnapshot {
                message_id,
                uid,
                size,
                internal_date,
                //subject,
            });
        }
        Ok(snapshots)
    }

    /// Check whether a specific Message-ID exists in a mailbox.
    /// Uses a TermQuery — O(1) per call, no allocation proportional to
    /// mailbox size. Suitable for large mailboxes where
    /// `get_message_ids_for_mailbox` would allocate too much memory.
    pub fn mailbox_contains_message_id(
        &self,
        account_id: u64,
        mailbox_id: u64,
        message_id: &str,
    ) -> BichonResult<bool> {
        let fields = SchemaTools::email_fields();
        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(fields.f_account_id, account_id),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(fields.f_mailbox_id, mailbox_id),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(fields.f_message_id, message_id),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        let searcher = self.create_searcher()?;
        let count = searcher
            .search(&query, &Count)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(count > 0)
    }

    fn envelope_query(&self, account_id: u64, eid: &str) -> Box<dyn Query> {
        let account_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::email_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let envelope_id_query = TermQuery::new(
            Term::from_field_text(SchemaTools::email_fields().f_id, eid),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_id_query)),
            (Occur::Must, Box::new(envelope_id_query)),
        ]);
        Box::new(boolean_query)
    }

    fn filter_query(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: EmailSearchFilter,
    ) -> BichonResult<Box<dyn Query>> {
        let f = SchemaTools::email_fields();
        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        if let Some(authorized_ids) = accounts {
            if authorized_ids.is_empty() {
                let term = Term::from_field_u64(f.f_account_id, u64::MAX);
                subqueries.push((
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            } else {
                let mut account_must_queries = Vec::new();
                for id in authorized_ids {
                    let term = Term::from_field_u64(f.f_account_id, id);
                    account_must_queries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                subqueries.push((
                    Occur::Must,
                    Box::new(BooleanQuery::new(account_must_queries)),
                ));
            }
        }

        if let Some(ref text) = filter.text {
            let query_parser =
                QueryParser::for_index(&self.index, SchemaTools::email_default_fields());

            let query = query_parser
                .parse_query(text)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref subject_val) = filter.subject {
            let query_parser = QueryParser::for_index(&self.index, vec![f.f_subject]);
            let q = query_parser
                .parse_query(subject_val)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, q));
        }

        if let Some(ref body_val) = filter.body {
            let query_parser = QueryParser::for_index(&self.index, vec![f.f_body]);

            let q = query_parser
                .parse_query(body_val)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, q));
        }

        if let Some(ref tags) = filter.tags {
            if !tags.is_empty() {
                let mut should_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

                for tag in tags {
                    let facet = Facet::from_text(tag).map_err(|e| {
                        raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter)
                    })?;

                    let term = Term::from_facet(f.f_tags, &facet);

                    should_queries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                    ));
                }
                subqueries.push((Occur::Must, Box::new(BooleanQuery::new(should_queries))));
            }
        }

        for (field, opt_value) in [
            (f.f_from_text, &filter.from),
            (f.f_to_text, &filter.to),
            (f.f_cc_text, &filter.cc),
            (f.f_bcc_text, &filter.bcc),
        ] {
            if let Some(ref v) = opt_value {
                let query_parser = QueryParser::for_index(&self.index, vec![field]);
                let q = query_parser
                    .parse_query(v)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
                subqueries.push((Occur::Must, q));
            }
        }

        // any_recipient: OR across to, cc, bcc
        if let Some(ref v) = filter.any_recipient {
            let mut recipient_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for field in [f.f_to_text, f.f_cc_text, f.f_bcc_text] {
                let query_parser = QueryParser::for_index(&self.index, vec![field]);
                if let Ok(q) = query_parser.parse_query(v) {
                    recipient_queries.push((Occur::Should, q));
                }
            }
            if !recipient_queries.is_empty() {
                subqueries.push((Occur::Must, Box::new(BooleanQuery::new(recipient_queries))));
            }
        }

        // any_participant: OR across from, to, cc, bcc
        if let Some(ref v) = filter.any_participant {
            let mut participant_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for field in [f.f_from_text, f.f_to_text, f.f_cc_text, f.f_bcc_text] {
                let query_parser = QueryParser::for_index(&self.index, vec![field]);
                if let Ok(q) = query_parser.parse_query(v) {
                    participant_queries.push((Occur::Should, q));
                }
            }
            if !participant_queries.is_empty() {
                subqueries.push((
                    Occur::Must,
                    Box::new(BooleanQuery::new(participant_queries)),
                ));
            }
        }

        if let Some(has) = filter.has_attachment {
            let lower: Bound<Term>;
            let upper: Bound<Term>;

            if has {
                lower = Bound::Included(Term::from_field_u64(f.f_regular_attachment_count, 1));
                upper =
                    Bound::Included(Term::from_field_u64(f.f_regular_attachment_count, u64::MAX));
            } else {
                lower = Bound::Included(Term::from_field_u64(f.f_regular_attachment_count, 0));
                upper = Bound::Included(Term::from_field_u64(f.f_regular_attachment_count, 0));
            }

            subqueries.push((Occur::Must, Box::new(RangeQuery::new(lower, upper))));
        }

        if let Some(ref name) = filter.attachment_name {
            if name.contains('.') {
                let term = Term::from_field_text(f.f_attachment_name_exact, name);
                let exact_query = TermQuery::new(term, IndexRecordOption::Basic);

                let query_parser =
                    QueryParser::for_index(&self.index, vec![f.f_attachment_name_text]);
                let q: Box<dyn Query> = query_parser
                    .parse_query(name)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
                let query = BooleanQuery::new(vec![
                    (Occur::Should, Box::new(exact_query)),
                    (Occur::Should, Box::new(q)),
                ]);
                subqueries.push((Occur::Must, Box::new(query)));
            } else {
                let query_parser =
                    QueryParser::for_index(&self.index, vec![f.f_attachment_name_text]);
                let q = query_parser
                    .parse_query(name)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
                subqueries.push((Occur::Must, q));
            }
        }

        if let Some(ref id) = filter.id {
            let term = Term::from_field_text(f.f_id, id);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref extension) = filter.attachment_extension {
            let term = Term::from_field_text(f.f_attachment_ext, extension);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref category) = filter.attachment_category {
            let term = Term::from_field_text(f.f_attachment_category, category);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref content_type) = filter.attachment_content_type {
            let term = Term::from_field_text(f.f_attachment_content_type, content_type);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        let start_bound = if let Some(from) = filter.since {
            Bound::Included(Term::from_field_i64(f.f_date, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.before {
            Bound::Included(Term::from_field_i64(f.f_date, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        let start_bound = if let Some(from) = filter.internal_date_since {
            Bound::Included(Term::from_field_i64(f.f_internal_date, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.internal_date_before {
            Bound::Included(Term::from_field_i64(f.f_internal_date, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        let start_bound = if let Some(from) = filter.ingest_since {
            Bound::Included(Term::from_field_i64(f.f_ingest_at, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.ingest_before {
            Bound::Included(Term::from_field_i64(f.f_ingest_at, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        if let Some(account_ids) = filter.account_ids {
            let mut should_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for id in account_ids {
                let term = Term::from_field_u64(f.f_account_id, id);
                should_queries.push((
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            }
            subqueries.push((Occur::Must, Box::new(BooleanQuery::new(should_queries))));
        }

        if let Some(mailbox_ids) = filter.mailbox_ids {
            let mut should_queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for id in mailbox_ids {
                let term = Term::from_field_u64(f.f_mailbox_id, id);
                should_queries.push((
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            }
            subqueries.push((Occur::Must, Box::new(BooleanQuery::new(should_queries))));
        }

        let start_bound = if let Some(from) = filter.min_size {
            Bound::Included(Term::from_field_u64(f.f_size, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.max_size {
            Bound::Included(Term::from_field_u64(f.f_size, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        if let Some(ref msg_id) = filter.message_id {
            let term = Term::from_field_text(f.f_message_id, msg_id);
            subqueries.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        if subqueries.is_empty() {
            return Ok(Box::new(AllQuery));
        }

        Ok(Box::new(BooleanQuery::new(subqueries)))
    }

    fn thread_query(&self, account_id: u64, thread_id: &str) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::email_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let thread_query = TermQuery::new(
            Term::from_field_text(SchemaTools::email_fields().f_thread_id, thread_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(thread_query)),
        ]);
        Box::new(boolean_query)
    }

    pub fn get_envelope_by_id(
        &self,
        account_id: u64,
        envelope_id: &str,
    ) -> BichonResult<Option<EnvelopeWithAttachments>> {
        let searcher = self.create_searcher()?;
        let f = SchemaTools::email_fields();

        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, account_id),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_id, envelope_id),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);

        let docs: Vec<(f32, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if let Some((_, doc_address)) = docs.first() {
            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = EnvelopeWithAttachments::from_tantivy_doc(&doc)?;
            Ok(Some(envelope))
        } else {
            Ok(None)
        }
    }

    pub fn top_10_largest_emails(
        &self,
        accounts: &Option<HashSet<u64>>,
    ) -> BichonResult<Vec<LargestEmail>> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term = Term::from_field_u64(SchemaTools::email_fields().f_account_id, id);
                    subqueries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                Box::new(BooleanQuery::new(subqueries))
            }
            Some(_) => Box::new(EmptyQuery),
            None => Box::new(AllQuery),
        };

        let mailbox_docs: Vec<(Option<u64>, DocAddress)> = searcher
            .search(
                &query,
                &TopDocs::with_limit(10).order_by_fast_field(F_SIZE, Order::Desc),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in mailbox_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = LargestEmail::from_tantivy_doc(&doc)?;
            result.push(envelope);
        }
        Ok(result)
    }

    /// How many messages live in these mailboxes of this account.
    ///
    /// Used to tell an approver the blast radius of a mailbox deletion before
    /// they sign off on it. Takes the whole subtree at once rather than one
    /// mailbox at a time because the delete is recursive: counting only the
    /// named mailbox would understate a folder with subfolders by however much
    /// they hold, and the approver would be approving a number that is wrong
    /// in the direction that matters.
    pub fn count_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_ids: &[u64],
    ) -> BichonResult<u64> {
        if mailbox_ids.is_empty() {
            return Ok(0);
        }
        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(mailbox_ids.len());
        for mailbox_id in mailbox_ids {
            subqueries.push((
                Occur::Should,
                Box::new(self.mailbox_query(account_id, *mailbox_id)),
            ));
        }
        let query = Box::new(BooleanQuery::new(subqueries)) as Box<dyn Query>;
        let searcher = self.create_searcher()?;
        let count = searcher
            .search(&query, &Count)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(count as u64)
    }

    pub fn total_emails(&self, accounts: &Option<HashSet<u64>>) -> BichonResult<u64> {
        let searcher = self.create_searcher()?;

        match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term = Term::from_field_u64(SchemaTools::email_fields().f_account_id, id);
                    subqueries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                let query = Box::new(BooleanQuery::new(subqueries)) as Box<dyn Query>;
                let count = searcher
                    .search(&query, &Count)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                Ok(count as u64)
            }
            Some(_) => Ok(0),
            None => Ok(searcher.num_docs()),
        }
    }

    pub fn get_max_uid(&self, account_id: u64, mailbox_id: u64) -> BichonResult<Option<u64>> {
        let searcher = self.create_searcher()?;

        let query = self.mailbox_query(account_id, mailbox_id);
        let agg_req: Aggregations = serde_json::from_value(json!({
            "max_uid": {
                "max": {
                    "field": F_UID
                }
            }
        }))
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let collector = AggregationCollector::from_aggs(agg_req, Default::default());

        let agg_res = searcher
            .search(query.as_ref(), &collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let result = Self::extract_max_uid(&agg_res);
        tracing::debug!(
            "[account {}][mailbox {}] get_max_uid = {:?} (num_docs in searcher = {})",
            account_id,
            mailbox_id,
            result,
            searcher.num_docs()
        );
        Ok(result)
    }

    pub fn get_account_stats(&self, account_id: u64) -> BichonResult<AccountStats> {
        let searcher = self.create_searcher()?;
        let query = self.account_query(account_id);

        let agg_req: Aggregations = serde_json::from_value(json!({
            "total_count": {
                "value_count": {
                    "field": F_ID
                }
            },
            "total_size": {
                "sum": {
                    "field": F_SIZE
                }
            }
        }))
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let collector = AggregationCollector::from_aggs(agg_req, Default::default());
        let agg_res = searcher
            .search(query.as_ref(), &collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut stats = AccountStats::default();

        stats.total_count = Self::extract_value_count(&agg_res, "total_count")?;

        let result = agg_res.0.get("total_size").ok_or_else(|| {
            raise_error!(
                "missing 'total_size' aggregation result".into(),
                ErrorCode::InternalError
            )
        })?;

        if let AggregationResult::MetricResult(MetricResult::Sum(v)) = result {
            stats.total_size = v.value.map(|v| v as u64).ok_or_else(|| {
                raise_error!(
                    "'total_size' sum metric has no value".into(),
                    ErrorCode::InternalError
                )
            })?;
        }
        Ok(stats)
    }

    fn extract_max_uid(agg_res: &AggregationResults) -> Option<u64> {
        agg_res.0.get("max_uid").and_then(|result| match result {
            AggregationResult::MetricResult(MetricResult::Max(max)) => {
                max.value.and_then(|value| {
                    (value >= 0.0 && value <= u64::MAX as f64).then(|| value.trunc() as u64)
                })
            }
            _ => None,
        })
    }

    pub async fn delete_account_envelopes(&self, account_id: u64) -> BichonResult<()> {
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
        let query = self.account_query(account_id);
        let (eml_content_hashes, attachments_content_hashes, leafs) =
            self.collect_content_hashes(query)?;

        let query = self.account_query(account_id);

        let mut writer = self.index_writer.lock().await;
        writer
            .delete_query(query)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        ATTACHMENT_MANAGER
            .delete_account_attachments(account_id)
            .await?;

        if !eml_content_hashes.is_empty() || !attachments_content_hashes.is_empty() {
            self.cleanup_unused_content(
                &mut writer,
                eml_content_hashes,
                attachments_content_hashes,
            )?;
        }

        DEDUP_CACHE.remove_by_account(account_id);
        crate::ext::timestamp::record_deletions(leafs, crate::utc_now!());
        Ok(())
    }

    pub async fn delete_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_ids: Vec<u64>,
    ) -> BichonResult<()> {
        if mailbox_ids.is_empty() {
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        let mut eml_content_hashes: HashSet<String> = HashSet::new();
        let mut attachments_content_hashes: HashSet<String> = HashSet::new();
        let mut leafs: Vec<LeafRecord> = Vec::new();

        for mailbox_id in &mailbox_ids {
            let query = self.mailbox_query(account_id, *mailbox_id);
            let (eml_hashes, attachment_hashes, mailbox_leafs) =
                self.collect_content_hashes(query)?;
            eml_content_hashes.extend(eml_hashes);
            attachments_content_hashes.extend(attachment_hashes);
            leafs.extend(mailbox_leafs);
        }

        let mut queries: Vec<Box<dyn Query>> = Vec::with_capacity(mailbox_ids.len());
        for mailbox_id in &mailbox_ids {
            queries.push(self.mailbox_query(account_id, *mailbox_id));
        }
        let mut writer = self.index_writer.lock().await;
        for query in queries {
            writer
                .delete_query(query)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if !eml_content_hashes.is_empty() || !attachments_content_hashes.is_empty() {
            self.cleanup_unused_content(
                &mut writer,
                eml_content_hashes,
                attachments_content_hashes,
            )?;
        }

        for mailbox_id in mailbox_ids {
            DEDUP_CACHE.remove_by_mailbox(mailbox_id);
        }

        crate::ext::timestamp::record_deletions(leafs, crate::utc_now!());
        Ok(())
    }

    /// Purge every envelope of an account whose effective date
    /// (`min(date, internal_date)`) is strictly older than `cutoff_ms`.
    ///
    /// Retention semantics (see `retention.rs`): a message is out of window when
    /// the *older* of its two timestamps is before the cutoff, with `0` treated
    /// as "unknown" (never counted as old). Messages whose timestamps are all
    /// unknown are never purged: retention cannot know how old they are, so it
    /// leaves them alone.
    ///
    /// Returns the number of envelopes purged.
    pub async fn delete_envelopes_before(
        &self,
        account_id: u64,
        cutoff_ms: i64,
    ) -> BichonResult<u64> {
        let count = {
            let searcher = self.create_searcher()?;
            searcher
                .search(&self.retention_purge_query(account_id, cutoff_ms), &Count)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        };
        if count == 0 {
            return Ok(0);
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        let (eml_hashes_with_mailbox, attachments_content_hashes, leafs) =
            self.collect_content_hashes_with_mailbox(self.retention_purge_query(
                account_id,
                cutoff_ms,
            ))?;

        let mut writer = self.index_writer.lock().await;
        writer
            .delete_query(self.retention_purge_query(account_id, cutoff_ms))
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let eml_content_hashes: HashSet<String> = eml_hashes_with_mailbox
            .iter()
            .map(|(hash, _)| hash.clone())
            .collect();
        if !eml_content_hashes.is_empty() || !attachments_content_hashes.is_empty() {
            self.cleanup_unused_content(
                &mut writer,
                eml_content_hashes,
                attachments_content_hashes,
            )?;
        }

        for (hash, mailbox_id) in eml_hashes_with_mailbox {
            DEDUP_CACHE.remove(account_id, mailbox_id, &hash);
        }

        crate::ext::timestamp::record_deletions(leafs, crate::utc_now!());
        Ok(count as u64)
    }

    /// Boolean query matching envelopes of `account_id` whose effective date is
    /// strictly older than `cutoff_ms`.
    ///
    /// Retention semantics (see `retention.rs`): a message is out of window when
    /// the *older* of its two timestamps is before the cutoff, with `0` treated
    /// as "unknown" (never counted as old). So the predicate is
    /// `(0 < date < cutoff) OR (0 < internal_date < cutoff)` — a message whose
    /// only known timestamp is recent is kept, and one with no timestamp at all
    /// is kept, mirroring `retention::effective_date_ms`.
    fn retention_purge_query(&self, account_id: u64, cutoff_ms: i64) -> Box<dyn Query> {
        let fields = SchemaTools::email_fields();

        let date_before = RangeQuery::new(
            Bound::Excluded(Term::from_field_i64(fields.f_date, 0)),
            Bound::Excluded(Term::from_field_i64(fields.f_date, cutoff_ms)),
        );
        let internal_date_before = RangeQuery::new(
            Bound::Excluded(Term::from_field_i64(fields.f_internal_date, 0)),
            Bound::Excluded(Term::from_field_i64(fields.f_internal_date, cutoff_ms)),
        );

        let older_than_cutoff = BooleanQuery::new(vec![
            (Occur::Should, Box::new(date_before)),
            (Occur::Should, Box::new(internal_date_before)),
        ]);

        Box::new(BooleanQuery::new(vec![
            (Occur::Must, self.account_query(account_id)),
            (Occur::Must, Box::new(older_than_cutoff)),
        ]))
    }

    fn collect_content_hashes(
        &self,
        query: Box<dyn Query>,
    ) -> BichonResult<(HashSet<String>, HashSet<String>, Vec<LeafRecord>)> {
        let (eml_with_mailbox, attachments_content_hashes, leafs) =
            self.collect_content_hashes_with_mailbox(query)?;

        let eml_content_hashes = eml_with_mailbox
            .into_iter()
            .map(|(hash, _mailbox_id)| hash)
            .collect();

        Ok((eml_content_hashes, attachments_content_hashes, leafs))
    }

    fn collect_content_hashes_with_mailbox(
        &self,
        query: Box<dyn Query>,
    ) -> BichonResult<(HashSet<(String, u64)>, HashSet<String>, Vec<LeafRecord>)> {
        let mut eml_content_hashes = HashSet::new();
        let mut attachments_content_hashes = HashSet::new();
        let mut leafs: Vec<LeafRecord> = Vec::new();

        let fields = SchemaTools::email_fields();
        let searcher = self.create_searcher()?;

        let docs = searcher
            .search(&query, &DocSetCollector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        for doc_address in docs {
            let doc = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            let mailbox_id = doc.get_first(fields.f_mailbox_id).and_then(|v| v.as_u64());

            // Capture the deletion-leaf identity (Enterprise anchor tree): an
            // email removed before the watermark scan ever saw it still gets a
            // leaf so the archive stays complete.
            let envelope_id = doc.get_first(fields.f_id).and_then(|v| v.as_str());
            let ingest_at = doc.get_first(fields.f_ingest_at).and_then(|v| v.as_i64());

            // Extract content_hash
            if let Some(content_hash_value) = doc.get_first(fields.f_content_hash) {
                if let (Some(hash_str), Some(mailbox_id)) =
                    (content_hash_value.as_str(), mailbox_id)
                {
                    eml_content_hashes.insert((hash_str.to_string(), mailbox_id));
                }
                if let (Some(hash_str), Some(envelope_id), Some(ingest_at)) =
                    (content_hash_value.as_str(), envelope_id, ingest_at)
                {
                    leafs.push(LeafRecord {
                        envelope_id: envelope_id.to_string(),
                        content_hash: hash_str.to_string(),
                        ingest_at,
                    });
                }
            }

            // Extract attachment_content_hash
            let attachment_hash_values = doc.get_all(fields.f_attachment_content_hash);
            for hash_value in attachment_hash_values {
                if let Some(str) = hash_value.as_str() {
                    attachments_content_hashes.insert(str.to_string());
                }
            }
        }

        Ok((eml_content_hashes, attachments_content_hashes, leafs))
    }

    fn cleanup_unused_content(
        &self,
        writer: &mut IndexWriter,
        eml_content_hashes: HashSet<String>,
        attachments_content_hashes: HashSet<String>,
    ) -> BichonResult<()> {
        // Reference-count barrier: commit the writer and reload the reader so the
        // `Count` below is evaluated against a fully committed, freshly-reloaded
        // index state. Without this, an envelope that shares a content hash but
        // is still sitting uncommitted in the writer buffer (e.g. added by the
        // background ingest task before this delete acquired the writer lock)
        // would be invisible to the searcher, the count would read 0, and a
        // still-referenced blob would be deleted.
        fatal_commit(writer);
        let searcher = self.create_searcher()?;
        let fields = SchemaTools::email_fields();

        // Email blobs and attachment blobs share one key space, so a single key
        // can be referenced both as an email content hash and as an attachment
        // content hash (e.g. a nested `.eml` attachment that was also archived
        // standalone). Only delete a key when no remaining document references
        // it in either field; otherwise the shared blob would be deleted while
        // still in use by the other reference type.
        let mut candidates: HashSet<String> = eml_content_hashes;
        candidates.extend(attachments_content_hashes);

        let mut to_delete: HashSet<String> = HashSet::new();
        for content_hash in candidates {
            // Check whether any remaining email still references this content hash.
            let email_term = Term::from_field_text(fields.f_content_hash, &content_hash);
            let email_query = TermQuery::new(email_term, IndexRecordOption::Basic);
            let email_count = searcher
                .search(&email_query, &Count)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            // Check whether any remaining email still references it as an attachment.
            let attachment_term =
                Term::from_field_text(fields.f_attachment_content_hash, &content_hash);
            let attachment_query = TermQuery::new(attachment_term, IndexRecordOption::Basic);
            let attachment_count = searcher
                .search(&attachment_query, &Count)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            // Only delete when neither reference type still uses the key.
            if email_count == 0 && attachment_count == 0 {
                to_delete.insert(content_hash);
            }
        }

        BLOB_MANAGER.delete(&to_delete, &to_delete)
    }

    pub async fn delete_envelopes_multi_account(
        &self,
        deletes: HashMap<u64, Vec<String>>,
    ) -> BichonResult<()> {
        if deletes.is_empty() {
            tracing::warn!("delete_envelopes_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        let mut eml_content_hash_triples: HashSet<(u64, u64, String)> = HashSet::new();
        let mut attachments_content_hashes: HashSet<String> = HashSet::new();
        let mut leafs: Vec<LeafRecord> = Vec::new();

        for (account_id, envelope_ids) in &deletes {
            let unique_ids: HashSet<&String> = envelope_ids.iter().collect();
            if unique_ids.is_empty() {
                continue;
            }

            for eid in unique_ids {
                let query = self.envelope_query(*account_id, eid);
                let (eml_hashes_with_mailbox, attachment_hashes, envelope_leafs) =
                    self.collect_content_hashes_with_mailbox(query)?;

                eml_content_hash_triples.extend(
                    eml_hashes_with_mailbox
                        .into_iter()
                        .map(|(hash, mailbox_id)| (*account_id, mailbox_id, hash)),
                );
                attachments_content_hashes.extend(attachment_hashes);
                leafs.extend(envelope_leafs);
            }
        }

        let mut writer = self.index_writer.lock().await;

        for (account_id, envelope_ids) in deletes {
            let unique_ids: HashSet<&String> = envelope_ids.iter().collect();
            if unique_ids.is_empty() {
                continue;
            }
            for eid in unique_ids {
                let query = self.envelope_query(account_id, eid);
                writer
                    .delete_query(query)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            }
        }
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if !eml_content_hash_triples.is_empty() || !attachments_content_hashes.is_empty() {
            let eml_content_hashes: HashSet<String> = eml_content_hash_triples
                .iter()
                .map(|(_, _, hash)| hash.clone())
                .collect();

            self.cleanup_unused_content(
                &mut writer,
                eml_content_hashes,
                attachments_content_hashes,
            )?;
        }

        for (aid, mid, hash) in eml_content_hash_triples {
            DEDUP_CACHE.remove(aid, mid, &hash);
        }

        crate::ext::timestamp::record_deletions(leafs, crate::utc_now!());
        Ok(())
    }

    fn collect_facets_recursive(
        query: &dyn Query,
        searcher: &Searcher,
        parent_facet: &str,
        all_facets: &mut Vec<TagCount>,
    ) -> BichonResult<()> {
        let mut facet_collector = FacetCollector::for_field(F_TAGS);
        facet_collector.add_facet(parent_facet);

        let facet_counts = searcher
            .search(query, &facet_collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        for (facet, count) in facet_counts.get(parent_facet) {
            all_facets.push(TagCount {
                tag: facet.to_string(),
                count,
            });
            Self::collect_facets_recursive(query, searcher, &facet.to_string(), all_facets)?;
        }

        Ok(())
    }

    pub fn get_all_tags(&self, accounts: Option<HashSet<u64>>) -> BichonResult<Vec<TagCount>> {
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term = Term::from_field_u64(SchemaTools::email_fields().f_account_id, id);
                    subqueries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                Box::new(BooleanQuery::new(subqueries))
            }
            Some(_) => Box::new(EmptyQuery),
            None => Box::new(AllQuery),
        };

        let mut all_facets = Vec::new();
        Self::collect_facets_recursive(&query, &searcher, "/", &mut all_facets)?;
        Ok(all_facets)
    }

    pub fn get_all_contacts(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<HashSet<String>> {
        let searcher = self.create_searcher()?;

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term = Term::from_field_u64(SchemaTools::email_fields().f_account_id, id);
                    subqueries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                Box::new(BooleanQuery::new(subqueries))
            }
            Some(_) => Box::new(EmptyQuery),
            None => Box::new(AllQuery),
        };

        let mut contacts_set: HashSet<String> = HashSet::new();

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1_000_000).order_by_score())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let contacts = extract_contacts(&doc)?;
            for value in contacts {
                contacts_set.insert(value);
            }
        }
        Ok(contacts_set)
    }

    pub async fn update_envelope_tags(&self, request: TagsRequest) -> BichonResult<()> {
        if request.updates.is_empty() {
            tracing::warn!("update_envelope_tags: request is empty, nothing to update");
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        // Legal hold: tag edits rewrite the envelope index for the account.
        // A held account's messages must stay findable and classified exactly
        // as they were at the hold date — folder/tag filters are part of how
        // compliance locates and produces evidence — so refuse metadata edits
        // for any held account. In the community edition `legal_hold` is always
        // false, so this guard is a no-op there.
        for account_id in request.updates.keys() {
            let account = AccountModel::get(*account_id)?;
            if account.is_on_hold() {
                return Err(raise_error!(
                    format!(
                        "Account {} ({}) is under a legal hold; editing message metadata (tags) is disabled while the hold is active",
                        account.id, account.email
                    ),
                    ErrorCode::Forbidden
                ));
            }
        }

        let searcher = self.create_searcher()?;
        let mut writer = self.index_writer.lock().await;

        let f = SchemaTools::email_fields();
        let f_tags = f.f_tags;
        let f_id = f.f_id;
        let deduplicated_updates: HashMap<u64, HashSet<String>> = request
            .updates
            .into_iter()
            .map(|(account_id, envelope_ids)| (account_id, envelope_ids.into_iter().collect()))
            .collect();

        let mut operations = Vec::new();

        for (account_id, envelope_ids) in &deduplicated_updates {
            for eid in envelope_ids {
                let query = self.envelope_query(*account_id, eid);
                let docs = searcher
                    .search(query.as_ref(), &TopDocs::with_limit(1).order_by_score())
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                if let Some((_, doc_address)) = docs.first() {
                    let old_doc: TantivyDocument = searcher
                        .doc(*doc_address)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                    let mut current_tags: HashSet<String> = old_doc
                        .get_all(f_tags)
                        .filter_map(|val| val.as_facet())
                        .map(|facet| facet.to_string())
                        .collect();

                    match request.action {
                        TagAction::Add => {
                            for tag in &request.tags {
                                current_tags.insert(tag.clone());
                            }
                        }
                        TagAction::Remove => {
                            for tag in &request.tags {
                                current_tags.remove(tag);
                            }
                        }
                        TagAction::Overwrite => {
                            current_tags = request.tags.iter().cloned().collect();
                        }
                    }

                    let mut new_doc = TantivyDocument::new();

                    // Copy stored fields, excluding f_tags (handled separately).
                    for (field, value) in old_doc.field_values() {
                        if field != f_tags {
                            new_doc.add_field_value(field, value);
                        }
                    }

                    // Reconstruct non-stored text-search fields from their
                    // stored counterparts. f_from_text / f_to_text / f_cc_text /
                    // f_bcc_text carry the same content as f_from / f_to / f_cc / f_bcc.
                    for val in old_doc.get_all(f.f_from) {
                        if let Some(s) = val.as_str() {
                            new_doc.add_text(f.f_from_text, s);
                        }
                    }
                    for val in old_doc.get_all(f.f_to) {
                        if let Some(s) = val.as_str() {
                            new_doc.add_text(f.f_to_text, s);
                        }
                    }
                    for val in old_doc.get_all(f.f_cc) {
                        if let Some(s) = val.as_str() {
                            new_doc.add_text(f.f_cc_text, s);
                        }
                    }
                    for val in old_doc.get_all(f.f_bcc) {
                        if let Some(s) = val.as_str() {
                            new_doc.add_text(f.f_bcc_text, s);
                        }
                    }

                    // Reconstruct attachment-name fields from the stored
                    // f_attachments JSON blob.
                    if let Some(attrs_val) = old_doc.get_first(f.f_attachments) {
                        if let Some(json_str) = attrs_val.as_str() {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str)
                            {
                                if let Some(arr) = parsed.as_array() {
                                    for att in arr {
                                        let is_inline = att
                                            .get("inline")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        let has_cid = att
                                            .get("content_id")
                                            .and_then(|v| v.as_str())
                                            .map(|s| !s.is_empty())
                                            .unwrap_or(false);
                                        if is_inline && has_cid {
                                            continue;
                                        }
                                        if let Some(filename) = att
                                            .get("filename")
                                            .and_then(|v| v.as_str())
                                            .filter(|s| !s.is_empty())
                                        {
                                            new_doc.add_text(f.f_attachment_name_text, filename);
                                            new_doc.add_text(f.f_attachment_name_exact, filename);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Reconstruct body text from the original EML stored in the
                    // blob store, referenced by f_content_hash.
                    if let Some(hash_val) = old_doc.get_first(f.f_content_hash) {
                        if let Some(content_hash) = hash_val.as_str() {
                            match BLOB_MANAGER.get_email(content_hash) {
                                Ok(Some(eml_bytes)) => {
                                    if let Some(message) = MessageParser::new().parse(&eml_bytes) {
                                        let text = message
                                            .body_text(0)
                                            .map(|cow| cow.into_owned())
                                            .or_else(|| {
                                                message
                                                    .body_html(0)
                                                    .map(|cow| extract_text(cow.into_owned()))
                                            })
                                            .unwrap_or_default();
                                        let body_text =
                                            text.split_whitespace().collect::<Vec<_>>().join(" ");
                                        if !body_text.is_empty() {
                                            new_doc.add_text(f.f_body, &body_text);
                                        }
                                    }
                                }
                                Ok(None) => {
                                    tracing::warn!(
                                        content_hash,
                                        "EML not found in blob store during tag update"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        content_hash,
                                        error = %e,
                                        "Failed to fetch EML during tag update"
                                    );
                                }
                            }
                        }
                    }

                    for tag in &current_tags {
                        new_doc.add_facet(f_tags, tag);
                    }

                    let delete_term = Term::from_field_text(f_id, eid);
                    operations.push(UserOperation::Delete(delete_term));
                    operations.push(UserOperation::Add(new_doc));
                }
            }
        }

        writer
            .run(operations)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // commit
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(())
    }

    pub fn search(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: EmailSearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
        sort_by: SortBy,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        let query = self.filter_query(accounts, filter)?;
        let searcher = self.create_searcher()?;
        let total = searcher
            .search(&query, &Count)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            as u64;

        if total == 0 {
            return Ok(DataPage {
                current_page: Some(page),
                page_size: Some(page_size),
                total_items: 0,
                items: vec![],
                total_pages: Some(0),
            });
        }
        let offset = (page - 1) * page_size;
        let total_pages = total.div_ceil(page_size);
        if offset > total {
            return Ok(DataPage {
                current_page: Some(page),
                page_size: Some(page_size),
                total_items: total,
                items: vec![],
                total_pages: Some(total_pages),
            });
        }

        let order = if desc { Order::Desc } else { Order::Asc };
        let mailbox_docs: Vec<DocAddress>;

        match sort_by {
            SortBy::DATE => {
                let date_docs: Vec<(Option<i64>, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_fast_field(F_DATE, order),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                mailbox_docs = date_docs.into_iter().map(|(_, addr)| addr).collect();
            }
            SortBy::SIZE => {
                let size_docs: Vec<(Option<u64>, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_fast_field(F_SIZE, order),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                mailbox_docs = size_docs.into_iter().map(|(_, addr)| addr).collect();
            }
            SortBy::InternalDate => {
                let internal_date_docs: Vec<(Option<i64>, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_fast_field(F_INTERNAL_DATE, order),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                mailbox_docs = internal_date_docs
                    .into_iter()
                    .map(|(_, addr)| addr)
                    .collect();
            }
            SortBy::IngestAt => {
                let ingest_at_docs: Vec<(Option<i64>, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_fast_field(F_INGEST_AT, order),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                mailbox_docs = ingest_at_docs.into_iter().map(|(_, addr)| addr).collect();
            }
            SortBy::Relevance => {
                // BM25 relevance: Tantivy scores the whole query. Always
                // descending (a higher score = more relevant); the `desc`
                // flag has no meaning here.
                let score_docs: Vec<(f32, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_score(),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                mailbox_docs = score_docs.into_iter().map(|(_, addr)| addr).collect();
            }
        }

        let mut result = Vec::new();

        for doc_address in mailbox_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = EnvelopeWithAttachments::from_tantivy_doc(&doc)?.envelope;
            result.push(envelope);
        }
        Ok(DataPage {
            current_page: Some(page),
            page_size: Some(page_size),
            total_items: total,
            items: result,
            total_pages: Some(total_pages),
        })
    }

    fn create_searcher(&self) -> BichonResult<Searcher> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(self.reader.searcher())
    }

    pub fn num_messages_in_thread(
        &self,
        searcher: &Searcher,
        account_id: u64,
        thread_id: &str,
    ) -> BichonResult<u64> {
        let query = self.thread_query(account_id, thread_id);

        let agg_req: Aggregations = serde_json::from_value(json!({
            "thread_count": {
                "value_count": {
                    "field": F_THREAD_ID
                }
            }
        }))
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let collector = AggregationCollector::from_aggs(agg_req, Default::default());

        let agg_res = searcher
            .search(query.as_ref(), &collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Self::extract_value_count(&agg_res, "thread_count")
    }

    fn extract_value_count(agg_res: &AggregationResults, name: &str) -> BichonResult<u64> {
        let Some(result) = agg_res.0.get(name) else {
            return Err(raise_error!(
                format!("Missing aggregation result: '{}'", name),
                ErrorCode::InternalError
            ));
        };

        match result {
            AggregationResult::MetricResult(MetricResult::Count(count)) => {
                Ok(count.value.map(|v| v as u64).ok_or_else(|| {
                    raise_error!(
                        "Failed to get count value from aggregation result: value is None".into(),
                        ErrorCode::InternalError
                    )
                })?)
            }
            other => Err(raise_error!(
                format!("Unexpected aggregation result type: {other:?}"),
                ErrorCode::InternalError
            )),
        }
    }

    pub fn list_thread_envelopes(
        &self,
        account_id: u64,
        thread_id: &str,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        let searcher = self.create_searcher()?;
        let total = self.num_messages_in_thread(&searcher, account_id, thread_id)?;
        if total == 0 {
            return Ok(DataPage {
                current_page: Some(page),
                page_size: Some(page_size),
                total_items: 0,
                items: vec![],
                total_pages: Some(0),
            });
        }
        let offset = (page - 1) * page_size;
        let total_pages = total.div_ceil(page_size);
        if offset > total {
            return Ok(DataPage {
                current_page: Some(page),
                page_size: Some(page_size),
                total_items: total,
                items: vec![],
                total_pages: Some(total_pages),
            });
        }

        let query = self.thread_query(account_id, thread_id);

        let order = if desc { Order::Desc } else { Order::Asc };
        let thread_docs: Vec<(Option<i64>, DocAddress)> = searcher
            .search(
                query.as_ref(),
                &TopDocs::with_limit(page_size as usize)
                    .and_offset(offset as usize)
                    .order_by_fast_field(F_DATE, order),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in thread_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = EnvelopeWithAttachments::from_tantivy_doc(&doc)?.envelope;
            result.push(envelope);
        }
        Ok(DataPage {
            current_page: Some(page),
            page_size: Some(page_size),
            total_items: total,
            items: result,
            total_pages: Some(total_pages),
        })
    }

    pub fn get_dashboard_stats(
        &self,
        accounts: &Option<HashSet<u64>>,
    ) -> BichonResult<DashboardStats> {
        let searcher = self.create_searcher()?;
        let now_ms = utc_now!();
        let week_ago_ms = (Utc::now() - Duration::from_secs(60 * 60 * 24 * 30)).timestamp_millis();

        let aggregations: Aggregations = serde_json::from_value(json!({
            "total_size": {
                "sum": { "field": F_SIZE }
            },
            "recent_30d_histogram": {
                "histogram": {
                    "field": F_DATE,
                    "interval": 86400000,
                    "hard_bounds": {
                        "min": week_ago_ms,
                        "max": now_ms
                    }
                }
            },
            "top_from_values": {
                "terms": {
                    "field": F_FROM,
                    "size": 10
                }
            },
            "top_account_values": {
                "terms": {
                    "field": F_ACCOUNT_ID,
                    "size": 10
                }
            },
            "attachment_stats": {
                "range": {
                    "field": F_REGULAR_ATTACHMENT_COUNT,
                    "ranges": [
                        { "to": 1, "key": "no_attachment" },
                        { "from": 1, "key": "has_attachment" }
                    ]
                }
            }
        }))
        .unwrap();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term = Term::from_field_u64(SchemaTools::email_fields().f_account_id, id);
                    subqueries.push((
                        Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ));
                }
                Box::new(BooleanQuery::new(subqueries))
            }
            Some(_) => Box::new(EmptyQuery),
            None => Box::new(AllQuery),
        };

        let agg_collector = AggregationCollector::from_aggs(aggregations, Default::default());
        let agg_results = searcher
            .search(&query, &agg_collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut stats = DashboardStats::default();
        let total_size = agg_results.0.get("total_size").ok_or_else(|| {
            raise_error!(
                "missing 'total_size' aggregation result".into(),
                ErrorCode::InternalError
            )
        })?;

        if let AggregationResult::MetricResult(MetricResult::Sum(v)) = total_size {
            let total_size = v.value.map(|v| v as u64).ok_or_else(|| {
                raise_error!(
                    "'total_size' sum metric has no value".into(),
                    ErrorCode::InternalError
                )
            })?;
            stats.total_size_bytes = total_size;
        }

        let recent_30d_histogram = agg_results.0.get("recent_30d_histogram").ok_or_else(|| {
            raise_error!(
                "missing 'recent_30d_histogram' aggregation result".into(),
                ErrorCode::InternalError
            )
        })?;

        let mut recent_activity = Vec::with_capacity(31);
        if let AggregationResult::BucketResult(BucketResult::Histogram { buckets, .. }) =
            recent_30d_histogram
        {
            if let BucketEntries::Vec(bucket_list) = buckets {
                for entry in bucket_list {
                    if let Key::F64(ms) = entry.key {
                        recent_activity.push(TimeBucket {
                            timestamp_ms: ms as i64,
                            count: entry.doc_count,
                        });
                    }
                }
            }
        }
        stats.recent_activity = recent_activity;
        let mut top_senders = Vec::with_capacity(11);
        let top_from_values = agg_results.0.get("top_from_values").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) =
            top_from_values
        {
            for entry in buckets {
                if let Key::Str(sender) = &entry.key {
                    top_senders.push(Group {
                        key: sender.clone(),
                        count: entry.doc_count,
                        account_id: None,
                    });
                }
            }
        }
        stats.top_senders = top_senders;

        let mut top_accounts = Vec::with_capacity(11);
        let top_account_values = agg_results.0.get("top_account_values").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) =
            top_account_values
        {
            for entry in buckets {
                if let Key::U64(account_id) = &entry.key {
                    match AccountModel::get(*account_id) {
                        Ok(account) => {
                            top_accounts.push(Group {
                                key: account.email,
                                account_id: Some(*account_id),
                                count: entry.doc_count,
                            });
                        }
                        Err(e) => {
                            warn!(
                                account_id = account_id,
                                error = %e,
                                "orphaned account index detected, scheduling cleanup"
                            );
                            let account_id = *account_id;
                            tokio::spawn(async move {
                                if let Err(e) =
                                    ENVELOPE_MANAGER.delete_account_envelopes(account_id).await
                                {
                                    tracing::error!(
                                        account_id = account_id,
                                        error = %e,
                                        "failed to cleanup envelope index"
                                    );
                                }
                            });
                        }
                    }
                }
            }
        }
        stats.top_accounts = top_accounts;

        let attachment_stats = agg_results.0.get("attachment_stats").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Range { buckets, .. }) =
            attachment_stats
        {
            match buckets {
                BucketEntries::Vec(bucket_vec) => {
                    for entry in bucket_vec {
                        match entry.key.to_string().as_str() {
                            "no_attachment" => {
                                stats.without_attachment_count = entry.doc_count;
                            }
                            "has_attachment" => {
                                stats.with_attachment_count = entry.doc_count;
                            }
                            _ => {}
                        }
                    }
                }
                BucketEntries::HashMap(bucket_map) => {
                    if let Some(entry) = bucket_map.get("no_attachment") {
                        stats.without_attachment_count = entry.doc_count;
                    }
                    if let Some(entry) = bucket_map.get("has_attachment") {
                        stats.with_attachment_count = entry.doc_count;
                    }
                }
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tantivy::tokenizers::EuroTokenizer;
    use serde_json::json;
    use tantivy::{
        collector::Count,
        query::{QueryParser, TermQuery},
        schema::IndexRecordOption,
        Index, Term,
    };

    /// Build a complete test document with known values across all
    /// stored and non-stored fields so we can verify reconstruction.
    fn build_test_doc() -> TantivyDocument {
        let f = SchemaTools::email_fields();
        let mut doc = TantivyDocument::new();

        doc.add_text(f.f_id, "test-eid-001");
        doc.add_text(f.f_message_id, "<test@msg.id>");
        doc.add_u64(f.f_account_id, 1);
        doc.add_u64(f.f_mailbox_id, 10);
        doc.add_u64(f.f_uid, 100);
        doc.add_text(f.f_subject, "Test Subject Line");
        doc.add_text(f.f_body, "the quick brown fox jumps over the lazy dog");
        doc.add_text(f.f_preview, "the quick brown fox...");
        doc.add_text(f.f_content_hash, "test-content-hash-001");
        // f_from / f_from_text carry the same data
        doc.add_text(f.f_from, "alice@example.com");
        doc.add_text(f.f_from_text, "alice@example.com");
        doc.add_text(f.f_to, "bob@example.com");
        doc.add_text(f.f_to_text, "bob@example.com");
        doc.add_text(f.f_cc, "carol@example.com");
        doc.add_text(f.f_cc_text, "carol@example.com");
        doc.add_text(f.f_bcc, "dave@example.com");
        doc.add_text(f.f_bcc_text, "dave@example.com");
        doc.add_i64(f.f_date, 1_700_000_000_000);
        doc.add_i64(f.f_internal_date, 1_700_000_000_000);
        doc.add_i64(f.f_ingest_at, 1_700_000_000_000);
        doc.add_u64(f.f_size, 999);
        doc.add_text(f.f_thread_id, "thread-xyz");

        // Attachment metadata (stored as JSON).
        let atts = json!([{
            "filename": "invoice.pdf",
            "file_type": "application/pdf",
            "inline": false,
            "size": 5000,
            "content_id": null,
            "content_hash": "att-hash-pdf",
            "is_message": false
        }]);
        doc.add_text(f.f_attachments, atts.to_string());
        doc.add_text(f.f_attachment_name_text, "invoice.pdf");
        doc.add_text(f.f_attachment_name_exact, "invoice.pdf");
        doc.add_text(f.f_attachment_ext, "pdf");
        doc.add_text(f.f_attachment_category, "document");
        doc.add_text(f.f_attachment_content_type, "application/pdf");
        doc.add_text(f.f_attachment_content_hash, "att-hash-pdf");
        doc.add_u64(f.f_attachment_count, 1);
        doc.add_u64(f.f_regular_attachment_count, 1);
        doc.add_u64(f.f_shard_id, 0);

        // Initial tags.
        doc.add_facet(f.f_tags, "/inbox");
        doc.add_facet(f.f_tags, "/unread");

        doc
    }

    /// Reconstruct a new tantivy document from `old_doc`, preserving all
    /// fields (including non-stored ones) and replacing tags with
    /// `new_tags`.  Body text is reconstructed from the supplied `eml_cache`
    /// (a stand-in for the blob store) rather than from
    /// `old_doc.field_values()` because `f_body` is not STORED.
    fn reconstruct_for_test(
        old_doc: &TantivyDocument,
        new_tags: &HashSet<String>,
        eml_cache: &HashMap<String, Vec<u8>>,
    ) -> TantivyDocument {
        let f = SchemaTools::email_fields();
        let mut new_doc = TantivyDocument::new();

        // ── stored fields (except f_tags) ──────────────────────────
        for (field, value) in old_doc.field_values() {
            if field != f.f_tags {
                new_doc.add_field_value(field, value);
            }
        }

        // ── non-stored text-search fields ──────────────────────────
        for val in old_doc.get_all(f.f_from) {
            if let Some(s) = val.as_str() {
                new_doc.add_text(f.f_from_text, s);
            }
        }
        for val in old_doc.get_all(f.f_to) {
            if let Some(s) = val.as_str() {
                new_doc.add_text(f.f_to_text, s);
            }
        }
        for val in old_doc.get_all(f.f_cc) {
            if let Some(s) = val.as_str() {
                new_doc.add_text(f.f_cc_text, s);
            }
        }
        for val in old_doc.get_all(f.f_bcc) {
            if let Some(s) = val.as_str() {
                new_doc.add_text(f.f_bcc_text, s);
            }
        }

        // ── attachment-name fields ─────────────────────────────────
        if let Some(attrs_val) = old_doc.get_first(f.f_attachments) {
            if let Some(json_str) = attrs_val.as_str() {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if let Some(arr) = parsed.as_array() {
                        for att in arr {
                            let is_inline =
                                att.get("inline").and_then(|v| v.as_bool()).unwrap_or(false);
                            let has_cid = att
                                .get("content_id")
                                .and_then(|v| v.as_str())
                                .map(|s| !s.is_empty())
                                .unwrap_or(false);
                            if is_inline && has_cid {
                                continue;
                            }
                            if let Some(filename) = att
                                .get("filename")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                new_doc.add_text(f.f_attachment_name_text, filename);
                                new_doc.add_text(f.f_attachment_name_exact, filename);
                            }
                        }
                    }
                }
            }
        }

        // ── body text (from eml cache – stands in for BLOB_MANAGER) ──
        if let Some(hash_val) = old_doc.get_first(f.f_content_hash) {
            if let Some(content_hash) = hash_val.as_str() {
                if let Some(eml_bytes) = eml_cache.get(content_hash) {
                    if let Some(message) = MessageParser::new().parse(eml_bytes) {
                        let text = message
                            .body_text(0)
                            .map(|cow| cow.into_owned())
                            .or_else(|| {
                                message
                                    .body_html(0)
                                    .map(|cow| extract_text(cow.into_owned()))
                            })
                            .unwrap_or_default();
                        let body_text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !body_text.is_empty() {
                            new_doc.add_text(f.f_body, &body_text);
                        }
                    }
                }
            }
        }

        // ── updated tags ───────────────────────────────────────────
        for tag in new_tags {
            new_doc.add_facet(f.f_tags, tag);
        }

        new_doc
    }

    #[test]
    fn update_tags_preserves_non_stored_fields() {
        let f = SchemaTools::email_fields();

        // ---- setup: in-memory index + document --------------------
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");
            let doc = build_test_doc();
            writer.add_document(doc).unwrap();
            writer.commit().unwrap();
        } // drop writer so the next one can acquire the lock

        // ---- build a minimal EML so body reconstruction works ------
        let eml = b"From: alice@example.com\r\n\
                         To: bob@example.com\r\n\
                         Subject: Test\r\n\
                         Date: Thu, 01 Jan 2023 00:00:00 +0000\r\n\
                         Message-ID: <test@msg.id>\r\n\
                         \r\n\
                         the quick brown fox jumps over the lazy dog\r\n";
        let mut eml_cache = HashMap::new();
        eml_cache.insert("test-content-hash-001".to_string(), eml.to_vec());

        // ---- read old doc, reconstruct, delete + add --------------
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(f.f_id, "test-eid-001"),
            IndexRecordOption::Basic,
        );
        let hits = searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .unwrap();
        assert_eq!(hits.len(), 1);

        let old_doc: TantivyDocument = searcher.doc(hits[0].1).unwrap();

        let mut new_tags = HashSet::new();
        new_tags.insert("/important".to_string());
        new_tags.insert("/inbox".to_string());

        let new_doc = reconstruct_for_test(&old_doc, &new_tags, &eml_cache);

        let mut writer2 = index
            .writer_with_num_threads(1, 15_000_000)
            .expect("writer2");
        writer2.delete_term(Term::from_field_text(f.f_id, "test-eid-001"));
        writer2.add_document(new_doc).unwrap();
        writer2.commit().unwrap();

        // ---- verify: search for non-stored fields still works -----
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        // Body text (tokenized via "euro")
        let body_parser = QueryParser::for_index(&index, vec![f.f_body]);
        let body_hits = searcher
            .search(&body_parser.parse_query("quick brown fox").unwrap(), &Count)
            .unwrap();
        assert_eq!(body_hits, 1, "body text should survive tag update");

        // from_text (tokenized via "euro")
        let from_parser = QueryParser::for_index(&index, vec![f.f_from_text]);
        let from_hits = searcher
            .search(
                &from_parser.parse_query("alice@example.com").unwrap(),
                &Count,
            )
            .unwrap();
        assert_eq!(from_hits, 1, "from_text should survive tag update");

        // to_text
        let to_parser = QueryParser::for_index(&index, vec![f.f_to_text]);
        let to_hits = searcher
            .search(&to_parser.parse_query("bob@example.com").unwrap(), &Count)
            .unwrap();
        assert_eq!(to_hits, 1, "to_text should survive tag update");

        // attachment_name_exact (STRING — not tokenized)
        let att_hits = searcher
            .search(
                &TermQuery::new(
                    Term::from_field_text(f.f_attachment_name_exact, "invoice.pdf"),
                    IndexRecordOption::Basic,
                ),
                &Count,
            )
            .unwrap();
        assert_eq!(
            att_hits, 1,
            "attachment_name_exact should survive tag update"
        );

        // Updated tags
        let tags_hits = searcher
            .search(
                &TermQuery::new(
                    Term::from_facet(f.f_tags, &Facet::from_text("/important").unwrap()),
                    IndexRecordOption::Basic,
                ),
                &Count,
            )
            .unwrap();
        assert_eq!(tags_hits, 1, "new tag /important should be present");

        // Old tag /unread should be gone since we overwrote with new_tags
        let old_tag_hits = searcher
            .search(
                &TermQuery::new(
                    Term::from_facet(f.f_tags, &Facet::from_text("/unread").unwrap()),
                    IndexRecordOption::Basic,
                ),
                &Count,
            )
            .unwrap();
        assert_eq!(old_tag_hits, 0, "old tag /unread should have been removed");
    }

    #[test]
    fn body_reconstruction_from_eml_cache() {
        // Verify the EML → body_text extraction used inside
        // reconstruct_for_test (and therefore update_envelope_tags).
        let eml = b"From: x@y\r\n\
                         Subject: testing\r\n\
                         Date: Thu, 01 Jan 2023 00:00:00 +0000\r\n\
                         \r\n\
                         hello world from the test suite\r\n";

        let mut cache = HashMap::new();
        cache.insert("hash-abc".to_string(), eml.to_vec());

        let f = SchemaTools::email_fields();
        let mut old = TantivyDocument::new();
        old.add_text(f.f_content_hash, "hash-abc");

        let reconstructed = reconstruct_for_test(&old, &HashSet::new(), &cache);

        // The body should have been extracted from the EML and added
        // back to the document.  Search for it.
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());
        let mut writer = index
            .writer_with_num_threads(1, 15_000_000)
            .expect("writer");
        writer.add_document(reconstructed).unwrap();
        writer.commit().unwrap();

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&index, vec![f.f_body]);
        let hits = searcher
            .search(&parser.parse_query("hello world").unwrap(), &Count)
            .unwrap();
        assert_eq!(hits, 1, "body text should be reconstructed from EML");
    }

    #[test]
    fn body_reconstruction_missing_eml_is_graceful() {
        // When the EML is not in the cache (simulating a blob-store
        // miss), the document should still be produced without body.
        let f = SchemaTools::email_fields();
        let mut old = TantivyDocument::new();
        old.add_text(f.f_content_hash, "nonexistent-hash");

        let cache = HashMap::new(); // empty
        let reconstructed = reconstruct_for_test(&old, &HashSet::new(), &cache);

        // The document exists but has no body field.
        let body_vals: Vec<_> = reconstructed.get_all(f.f_body).collect();
        assert!(
            body_vals.is_empty(),
            "body should be absent when EML is missing"
        );
    }

    // ── get_message_ids_for_mailbox ─────────────────────────────────

    #[test]
    fn get_message_ids_returns_stored_ids() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        // Insert two docs for mailbox 10, one for mailbox 20
        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            let mut doc1 = TantivyDocument::new();
            doc1.add_u64(f.f_account_id, 1);
            doc1.add_u64(f.f_mailbox_id, 10);
            doc1.add_text(f.f_message_id, "<msg-a@test>");
            doc1.add_text(f.f_id, "id-a");
            doc1.add_u64(f.f_uid, 1);
            doc1.add_text(f.f_content_hash, "hash-a");
            writer.add_document(doc1).unwrap();

            let mut doc2 = TantivyDocument::new();
            doc2.add_u64(f.f_account_id, 1);
            doc2.add_u64(f.f_mailbox_id, 10);
            doc2.add_text(f.f_message_id, "<msg-b@test>");
            doc2.add_text(f.f_id, "id-b");
            doc2.add_u64(f.f_uid, 2);
            doc2.add_text(f.f_content_hash, "hash-b");
            writer.add_document(doc2).unwrap();

            let mut doc3 = TantivyDocument::new();
            doc3.add_u64(f.f_account_id, 1);
            doc3.add_u64(f.f_mailbox_id, 20);
            doc3.add_text(f.f_message_id, "<msg-c@test>");
            doc3.add_text(f.f_id, "id-c");
            doc3.add_u64(f.f_uid, 3);
            doc3.add_text(f.f_content_hash, "hash-c");
            writer.add_document(doc3).unwrap();

            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        // We can't easily call ENVELOPE_MANAGER.get_message_ids_for_mailbox
        // because it reads from ENVELOPE_MANAGER's own index, not our in-memory one.
        // Instead, test the query pattern directly.
        let query: Box<dyn Query> = {
            let account_query = TermQuery::new(
                Term::from_field_u64(f.f_account_id, 1),
                IndexRecordOption::Basic,
            );
            let mailbox_query = TermQuery::new(
                Term::from_field_u64(f.f_mailbox_id, 10),
                IndexRecordOption::Basic,
            );
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, Box::new(account_query)),
                (Occur::Must, Box::new(mailbox_query)),
            ]))
        };

        let docs = searcher.search(&query, &DocSetCollector).unwrap();

        let mut ids: Vec<String> = Vec::new();
        for addr in docs {
            let doc: TantivyDocument = searcher.doc(addr).unwrap();
            if let Some(v) = doc.get_first(f.f_message_id) {
                if let Some(s) = v.as_str() {
                    ids.push(s.to_string());
                }
            }
        }
        ids.sort();

        assert_eq!(ids, vec!["<msg-a@test>", "<msg-b@test>"]);
    }

    #[test]
    fn get_message_ids_empty_mailbox_returns_empty() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            // Doc for a different mailbox
            let mut doc = TantivyDocument::new();
            doc.add_u64(f.f_account_id, 1);
            doc.add_u64(f.f_mailbox_id, 99);
            doc.add_text(f.f_message_id, "<other@test>");
            doc.add_text(f.f_id, "id-other");
            doc.add_u64(f.f_uid, 1);
            doc.add_text(f.f_content_hash, "hash-other");
            writer.add_document(doc).unwrap();
            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        let query: Box<dyn Query> = {
            let account_query = TermQuery::new(
                Term::from_field_u64(f.f_account_id, 1),
                IndexRecordOption::Basic,
            );
            let mailbox_query = TermQuery::new(
                Term::from_field_u64(f.f_mailbox_id, 10),
                IndexRecordOption::Basic,
            );
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, Box::new(account_query)),
                (Occur::Must, Box::new(mailbox_query)),
            ]))
        };

        let docs = searcher.search(&query, &DocSetCollector).unwrap();
        assert!(docs.is_empty());
    }

    // ── mailbox_contains_message_id ───────────────────────────────

    #[test]
    fn mailbox_contains_message_id_finds_existing() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            let mut doc = TantivyDocument::new();
            doc.add_u64(f.f_account_id, 1);
            doc.add_u64(f.f_mailbox_id, 10);
            doc.add_text(f.f_message_id, "abc@example.com");
            doc.add_text(f.f_id, "id-1");
            doc.add_u64(f.f_uid, 1);
            doc.add_text(f.f_content_hash, "hash-1");
            writer.add_document(doc).unwrap();
            writer.commit().unwrap();
        }

        // We test the query pattern directly (can't call ENVELOPE_MANAGER
        // which uses a different index).
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, 1),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_mailbox_id, 10),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_message_id, "abc@example.com"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);

        let count = searcher.search(&query, &Count).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn mailbox_contains_message_id_returns_zero_for_missing() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            let mut doc = TantivyDocument::new();
            doc.add_u64(f.f_account_id, 1);
            doc.add_u64(f.f_mailbox_id, 10);
            doc.add_text(f.f_message_id, "existing@example.com");
            doc.add_text(f.f_id, "id-1");
            doc.add_u64(f.f_uid, 1);
            doc.add_text(f.f_content_hash, "hash-1");
            writer.add_document(doc).unwrap();
            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        let query = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, 1),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_mailbox_id, 10),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_message_id, "nonexistent@example.com"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);

        let count = searcher.search(&query, &Count).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn mailbox_contains_message_id_respects_mailbox_boundary() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            // Same Message-ID in mailbox 10
            let mut doc1 = TantivyDocument::new();
            doc1.add_u64(f.f_account_id, 1);
            doc1.add_u64(f.f_mailbox_id, 10);
            doc1.add_text(f.f_message_id, "shared@example.com");
            doc1.add_text(f.f_id, "id-1");
            doc1.add_u64(f.f_uid, 1);
            doc1.add_text(f.f_content_hash, "hash-1");
            writer.add_document(doc1).unwrap();

            // Same Message-ID in mailbox 20 (different mailbox)
            let mut doc2 = TantivyDocument::new();
            doc2.add_u64(f.f_account_id, 1);
            doc2.add_u64(f.f_mailbox_id, 20);
            doc2.add_text(f.f_message_id, "shared@example.com");
            doc2.add_text(f.f_id, "id-2");
            doc2.add_u64(f.f_uid, 2);
            doc2.add_text(f.f_content_hash, "hash-2");
            writer.add_document(doc2).unwrap();
            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        // Query mailbox 10: should find 1
        let q10 = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, 1),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_mailbox_id, 10),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_message_id, "shared@example.com"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        assert_eq!(searcher.search(&q10, &Count).unwrap(), 1);

        // Query mailbox 20: should find 1
        let q20 = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, 1),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_mailbox_id, 20),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_message_id, "shared@example.com"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        assert_eq!(searcher.search(&q20, &Count).unwrap(), 1);

        // Query mailbox 99 (no docs): should find 0
        let q99 = BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_account_id, 1),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(f.f_mailbox_id, 99),
                    IndexRecordOption::Basic,
                )),
            ),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f.f_message_id, "shared@example.com"),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
        assert_eq!(searcher.search(&q99, &Count).unwrap(), 0);
    }

    // ── IMAP UID concept: order_by_fast_field(ingest_at) ─────────────

    #[test]
    fn stable_uid_ordering_by_ingest_at() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        let mailbox_id = 42u64;

        // 10 emails, some sharing the same ingest_at (simulating batch import)
        #[rustfmt::skip]
        let test_data: Vec<(u64, u64, i64)> = vec![
            // (account_id, uid, ingest_at)
            (1, 101, 1_700_000_000_000), // epoch #1
            (1, 102, 1_700_000_000_000), // epoch #1 — same ms, different uid
            (1, 201, 1_700_000_000_100), // epoch #2
            (1, 202, 1_700_000_000_100), // epoch #2 — same ms
            (1, 203, 1_700_000_000_100), // epoch #2 — 3 in same ms
            (1, 301, 1_700_000_000_200),
            (1, 401, 1_700_000_000_300),
            (2, 501, 1_700_000_000_300), // different account, same ms — different mailbox
            (1, 402, 1_700_000_000_400),
            (1, 501, 1_700_000_000_500),
        ];

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            for (i, (account_id, uid, ingest_at)) in test_data.iter().enumerate() {
                let mut doc = TantivyDocument::new();
                doc.add_text(f.f_id, format!("eid-{}", i));
                doc.add_text(f.f_message_id, format!("<msg-{}@test>", i));
                doc.add_u64(f.f_account_id, *account_id);
                doc.add_u64(f.f_mailbox_id, mailbox_id);
                doc.add_u64(f.f_uid, *uid);
                doc.add_i64(f.f_ingest_at, *ingest_at);
                doc.add_i64(f.f_date, *ingest_at);
                doc.add_i64(f.f_internal_date, *ingest_at);
                doc.add_u64(f.f_size, 100);
                doc.add_text(f.f_subject, "test");
                doc.add_text(f.f_preview, "preview");
                doc.add_text(f.f_content_hash, format!("hash-{}", i));
                doc.add_text(f.f_from, "a@b.com");
                doc.add_text(f.f_thread_id, "t1");
                writer.add_document(doc).unwrap();
            }
            writer.commit().unwrap();
        }

        let reader = index.reader().expect("reader");
        let searcher = reader.searcher();

        let query = TermQuery::new(
            Term::from_field_u64(f.f_mailbox_id, mailbox_id),
            IndexRecordOption::Basic,
        );

        // Primary sort: Tantivy fast field on ingest_at
        let top_docs: Vec<(Option<i64>, DocAddress)> = searcher
            .search(
                &query,
                &tantivy::collector::TopDocs::with_limit(100)
                    .order_by_fast_field::<i64>(F_INGEST_AT, tantivy::Order::Asc),
            )
            .unwrap();

        assert_eq!(top_docs.len(), 10, "all 10 docs should be returned");

        // Read each doc, extract (ingest_at, uid) for app-level tie-break
        let mut results: Vec<(i64, u64)> = Vec::new();
        for (_, addr) in &top_docs {
            let doc: TantivyDocument = searcher.doc(*addr).unwrap();
            let ingest_at = doc
                .get_first(f.f_ingest_at)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let uid = doc.get_first(f.f_uid).and_then(|v| v.as_u64()).unwrap_or(0);
            results.push((ingest_at, uid));
        }

        // Verify primary sort by ingest_at is correct
        for w in results.windows(2) {
            assert!(w[0].0 <= w[1].0, "ingest_at must be non-decreasing");
        }

        // Verify deterministic: run again, same order
        let top_docs2: Vec<DocAddress> = searcher
            .search(
                &query,
                &tantivy::collector::TopDocs::with_limit(100)
                    .order_by_fast_field::<i64>(F_INGEST_AT, tantivy::Order::Asc),
            )
            .unwrap()
            .into_iter()
            .map(|(_, addr)| addr)
            .collect();

        for (i, addr) in top_docs.iter().map(|(_, a)| *a).enumerate() {
            assert_eq!(addr, top_docs2[i], "order must be stable across queries");
        }

        // Demonstrate tie-break: same ingest_at groups MUST be ordered by uid
        let mut current_group: Vec<u64> = Vec::new();
        for w in results.windows(2) {
            current_group.push(w[0].1);
            if w[0].0 != w[1].0 {
                current_group.push(w[0].1);
                let mut sorted = current_group.clone();
                sorted.sort();
                assert_eq!(
                    current_group, sorted,
                    "within same ingest_at, uids must be ascending"
                );
                current_group = Vec::new();
            }
        }

        println!("IMAP UID mapping (position → ingest_at, uid):");
        for (pos, (ingest_at, uid)) in results.iter().enumerate() {
            println!(
                "  UID {} → (ingest_at={}, original_uid={})",
                pos + 1,
                ingest_at,
                uid
            );
        }
    }

    // ── retention purge query ────────────────────────────────────────
    //
    // Replicates `retention_purge_query` against an in-RAM index to verify the
    // Tantivy semantics: matches `account_id AND ((0 < date < cutoff) OR (0 <
    // internal_date < cutoff))` — the older of the two known timestamps, never
    // counting an unknown `0` as old — and never crosses account boundaries.

    #[test]
    fn retention_purge_query_matches_older_of_two_dates() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        let cutoff = 1_700_000_000_000i64;
        let old = cutoff - 1_000;
        let new = cutoff + 1_000;

        // (account, date, internal_date) — see assertions below.
        let docs: Vec<(u64, i64, i64, &str)> = vec![
            (1, old, old, "d1-old-old"),    // min old → purge
            (1, old, new, "d2-old-new"),    // min old → purge
            (1, new, old, "d3-new-old"),    // min old → purge
            (1, new, new, "d4-new-new"),    // recent → keep
            (1, 0, new, "d5-no-date"),      // effective = new → keep
            (1, 0, old, "d6-no-date-old"),  // effective = old → purge
            (1, 0, 0, "d7-no-timestamps"),  // unknown → keep
            (2, old, old, "d8-other-account"), // other account → keep
        ];

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");
            for (account_id, date, internal_date, id) in &docs {
                let mut doc = TantivyDocument::new();
                doc.add_u64(f.f_account_id, *account_id);
                doc.add_u64(f.f_mailbox_id, 10);
                doc.add_text(f.f_id, *id);
                doc.add_u64(f.f_uid, 1);
                doc.add_text(f.f_content_hash, format!("hash-{id}"));
                doc.add_i64(f.f_date, *date);
                doc.add_i64(f.f_internal_date, *internal_date);
                writer.add_document(doc).unwrap();
            }
            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        let date_before = RangeQuery::new(
            Bound::Excluded(Term::from_field_i64(f.f_date, 0)),
            Bound::Excluded(Term::from_field_i64(f.f_date, cutoff)),
        );
        let internal_date_before = RangeQuery::new(
            Bound::Excluded(Term::from_field_i64(f.f_internal_date, 0)),
            Bound::Excluded(Term::from_field_i64(f.f_internal_date, cutoff)),
        );
        let older_than_cutoff = BooleanQuery::new(vec![
            (Occur::Should, Box::new(date_before)),
            (Occur::Should, Box::new(internal_date_before)),
        ]);
        let account_query = TermQuery::new(
            Term::from_field_u64(f.f_account_id, 1),
            IndexRecordOption::Basic,
        );
        let query: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(older_than_cutoff)),
        ]));

        let docs = searcher.search(&query, &DocSetCollector).unwrap();
        let matched: HashSet<String> = docs
            .iter()
            .map(|addr| {
                let doc: TantivyDocument = searcher.doc(*addr).unwrap();
                doc.get_first(f.f_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        assert_eq!(matched.len(), 4, "expected d1,d2,d3,d6 to match");
        for id in ["d1-old-old", "d2-old-new", "d3-new-old", "d6-no-date-old"] {
            assert!(matched.contains(id), "{id} should be matched, got {matched:?}");
        }
        for id in ["d4-new-new", "d5-no-date", "d7-no-timestamps", "d8-other-account"] {
            assert!(!matched.contains(id), "{id} should NOT be matched, got {matched:?}");
        }
    }

    #[tokio::test]
    async fn update_envelope_tags_guard_fires_for_held_account() {
        use std::sync::Once;

        use crate::account::migration::{AccountModel, AccountType};
        use crate::account::payload::AccountCreateRequest;
        use crate::database::{insert_impl, manager::DB_MANAGER};
        use crate::message::tags::{TagAction, TagsRequest};
        use crate::settings::cli::SETTINGS;
        use crate::settings::dir::DATA_DIR_MANAGER;

        static TEST_ENV: Once = Once::new();
        TEST_ENV.call_once(|| {
            let root = std::env::temp_dir().join(format!(
                "bichon-core-test-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
        });

        let request = AccountCreateRequest {
            email: "hold-tags@test.example".to_string(),
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
            retention_days: None,
        };
        let account = AccountModel::new(9021, request).unwrap();
        insert_impl(DB_MANAGER.db(), account.clone()).unwrap();

        let req = TagsRequest {
            updates: HashMap::from([(account.id, vec!["some-eid".to_string()])]),
            tags: vec!["important".to_string()],
            action: TagAction::Add,
        };

        // No hold: the guard passes and the call returns Ok (the envelope id
        // simply does not exist) — proving the guard is what blocks the held case.
        assert!(
            ENVELOPE_MANAGER.update_envelope_tags(req.clone()).await.is_ok(),
            "tag update for a non-held account should reach the index path"
        );

        // Under a hold: refused with Forbidden before touching the index.
        AccountModel::place_legal_hold(account.id, 7, Some("freeze".into())).unwrap();
        let err = ENVELOPE_MANAGER.update_envelope_tags(req).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Forbidden);
    }

    // ── BM25 relevance vs DATE desc ordering ──────────────────────
    //
    // Regression scenario from the "RELEVANCE sort" feature request: a
    // multi-term text query is OR'ed across the default fields, and results
    // are ordered by DATE desc (the API default), so the mail matching all
    // terms gets buried under a newer, one-term-only hit. The BM25 score that
    // Tantivy already computes internally ranks the both-terms match first —
    // it is just never surfaced or used for ordering.
    #[test]
    fn bm25_relevance_ranks_both_terms_match_first_while_date_desc_buries_it() {
        let f = SchemaTools::email_fields();
        let index = Index::create_in_ram(SchemaTools::email_schema());
        index.tokenizers().register("euro", EuroTokenizer::new());

        {
            let mut writer = index
                .writer_with_num_threads(1, 15_000_000)
                .expect("writer");

            // The mail the user is actually looking for: 2025-01-16, subject
            // "Bienvenue chez Primagaz !" — contains BOTH query terms.
            let mut target = TantivyDocument::new();
            target.add_u64(f.f_account_id, 1);
            target.add_u64(f.f_mailbox_id, 10);
            target.add_text(f.f_message_id, "<target@test>");
            target.add_text(f.f_id, "id-target");
            target.add_u64(f.f_uid, 1);
            target.add_text(f.f_content_hash, "hash-target");
            target.add_text(f.f_subject, "Bienvenue chez Primagaz !");
            target.add_text(f.f_body, "Bienvenue chez Primagaz, votre compte est prêt.");
            target.add_i64(f.f_date, 1_736_985_600_000); // 2025-01-16
            writer.add_document(target).unwrap();

            // Newer mail (2025-09-01) that only mentions "bienvenue": it
            // matches the OR query but is irrelevant to "primagaz".
            let mut recent = TantivyDocument::new();
            recent.add_u64(f.f_account_id, 1);
            recent.add_u64(f.f_mailbox_id, 10);
            recent.add_text(f.f_message_id, "<recent@test>");
            recent.add_text(f.f_id, "id-recent");
            recent.add_u64(f.f_uid, 2);
            recent.add_text(f.f_content_hash, "hash-recent");
            recent.add_text(f.f_subject, "Welcome to our September newsletter");
            recent.add_text(f.f_body, "bienvenue à tous nos nouveaux abonnés");
            recent.add_i64(f.f_date, 1_755_705_600_000); // 2025-09-01
            writer.add_document(recent).unwrap();

            writer.commit().unwrap();
        }

        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();

        // The exact query the search path builds for a text filter
        // (filter_query in this file: QueryParser over email_default_fields,
        // default OR conjunction).
        let query = QueryParser::for_index(&index, SchemaTools::email_default_fields())
            .parse_query("bienvenue primagaz")
            .unwrap();

        // ── Current behaviour: DATE desc (the API default) ─────────
        let date_top: Vec<(Option<i64>, DocAddress)> = searcher
            .search(
                &query,
                &TopDocs::with_limit(2).order_by_fast_field(F_DATE, Order::Desc),
            )
            .unwrap();
        let date_order: Vec<String> = date_top
            .iter()
            .map(|(_, addr)| {
                let doc: TantivyDocument = searcher.doc(*addr).unwrap();
                doc.get_first(f.f_message_id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            date_order[0], "<recent@test>",
            "DATE desc must put the newer one-term mail first, burying the best match"
        );

        // ── Desired: BM25 relevance ordering ───────────────────────
        let score_top: Vec<(f32, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(2).order_by_score())
            .unwrap();
        let score_order: Vec<String> = score_top
            .iter()
            .map(|(_, addr)| {
                let doc: TantivyDocument = searcher.doc(*addr).unwrap();
                doc.get_first(f.f_message_id)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            score_order[0], "<target@test>",
            "BM25 must rank the both-terms match first"
        );

        // The both-terms doc must genuinely outscore the one-term doc.
        assert!(
            score_top[0].0 > score_top[1].0,
            "target score {:.4} should exceed recent score {:.4}",
            score_top[0].0,
            score_top[1].0
        );
    }
}
