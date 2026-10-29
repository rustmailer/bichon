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
    account::access::ViewScopeGroup,
    backup::gate::{BACKUP_ACQUIRE_TIMEOUT, WRITE_GATE},
    common::{paginated::DataPage, signal::SIGNAL_MANAGER},
    dashboard::{Group, LargestAttachment},
    error::{code::ErrorCode, BichonResult},
    message::{
        attachment::AttachmentMetadata,
        search::{AttachmentSearchFilter, SortBy},
        tags::{TagAction, TagCount, TagsRequest},
    },
    raise_error,
    settings::dir::DATA_DIR_MANAGER,
    store::{
        blob::BLOB_MANAGER,
        tantivy::{
            envelope::ENVELOPE_MANAGER,
            fatal_commit,
            fields::{
                F_ATTACHMENT_CATEGORY, F_ATTACHMENT_CONTENT_TYPE, F_ATTACHMENT_EXT, F_DATE,
                F_INGEST_AT, F_SIZE, F_TAGS,
            },
            model::{extract_senders, AttachmentModel},
            schema::SchemaTools,
            tokenizers::{
                any_term_parser, text_parser, tri_analyzer, word_or_tri, word_or_tri_any,
                EuroTokenizer,
            },
            WriterSlot,
        },
    },
};

use serde_json::json;
use tantivy::{
    aggregation::{
        agg_req::Aggregations,
        agg_result::{AggregationResult, BucketResult},
        AggregationCollector, Key,
    },
    collector::{Count, FacetCollector, TopDocs},
    indexer::{LogMergePolicy, UserOperation},
    query::{AllQuery, BooleanQuery, EmptyQuery, Occur, Query, RangeQuery, TermQuery},
    schema::{Field, IndexRecordOption, Value},
    DocAddress, Index, IndexReader, IndexWriter, Order, TantivyDocument, Term,
};
use tantivy::{schema::Facet, Searcher};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task::{self, JoinHandle},
};
use tracing::info;

pub static ATTACHMENT_MANAGER: LazyLock<IndexManager> = LazyLock::new(IndexManager::new);

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

/// Re-extract an attachment's text from its blob, the way ingest did.
///
/// Needed whenever an attachment document is rebuilt (a tag edit): `f_text` is
/// not stored, so it cannot be copied. The blob key lives in the *parent* email
/// document's `f_attachments` JSON (`storage_hash()`), so this does a point
/// lookup there first. Returns `None` — "no text available" — when the extractor
/// is a noop (community edition), the parent or blob is missing, or the file
/// type/size is not extractable.
fn reextract_attachment_text(
    envelope_searcher: &Searcher,
    old_doc: &TantivyDocument,
    af: &crate::store::tantivy::fields::AttachmentFields,
) -> Option<crate::ext::text_extractor::ExtractedText> {
    use crate::ext::text_extractor;

    if text_extractor::is_noop() {
        return None;
    }
    let envelope_id = old_doc
        .get_first(af.f_envelope_id)
        .and_then(|v| v.as_str())?;
    let content_hash = old_doc
        .get_first(af.f_content_hash)
        .and_then(|v| v.as_str())?;
    let content_type = old_doc
        .get_first(af.f_content_type)
        .and_then(|v| v.as_str())?;
    let ext = old_doc
        .get_first(af.f_ext)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let size = old_doc
        .get_first(af.f_size)
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if !text_extractor::should_try_extract(content_type, ext) {
        return None;
    }
    if size > text_extractor::max_extract_bytes() as u64 {
        return None;
    }

    let ef = SchemaTools::email_fields();
    let term = Term::from_field_text(ef.f_id, envelope_id);
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    let hits = envelope_searcher
        .search(&query, &TopDocs::with_limit(1).order_by_score())
        .ok()?;
    let (_, addr) = hits.first()?;
    let parent: TantivyDocument = envelope_searcher.doc(*addr).ok()?;
    let json = parent.get_first(ef.f_attachments).and_then(|v| v.as_str())?;
    let infos: Vec<crate::message::content::AttachmentInfo> = serde_json::from_str(json).ok()?;
    let info = infos.iter().find(|a| a.content_hash == content_hash)?;
    let bytes = BLOB_MANAGER.get_attachment(info.storage_hash()).ok()??;
    text_extractor::extract_text(content_type, ext, &bytes)
}

/// Rebuild an attachment document from an existing one, replacing its tags.
///
/// `field_values()` only returns stored fields, so everything not stored has to
/// be re-derived: the address and name text (mirroring their stored
/// counterparts), their trigram siblings, and the extracted `f_text` — which,
/// when `envelope_searcher` is available, is re-extracted from the blob. When it
/// cannot be restored the text flags are reset, so the document never claims to
/// be indexed while its text is empty.
fn rebuild_attachment_doc(
    old_doc: &TantivyDocument,
    af: &crate::store::tantivy::fields::AttachmentFields,
    new_tags: impl IntoIterator<Item = String>,
    envelope_searcher: Option<&Searcher>,
) -> TantivyDocument {
    let mut new_doc = TantivyDocument::new();

    // Stored fields, except the tags (re-added last) and the text flags
    // (re-derived with `f_text`).
    for (field, value) in old_doc.field_values() {
        if field == af.f_tags
            || field == af.f_has_text
            || field == af.f_is_indexed
            || field == af.f_is_ocr
            || field == af.f_page_count
        {
            continue;
        }
        new_doc.add_field_value(field, value);
    }

    if let Some(s) = old_doc.get_first(af.f_subject).and_then(|v| v.as_str()) {
        new_doc.add_text(af.f_subject_tri, s);
    }
    for val in old_doc.get_all(af.f_from) {
        if let Some(s) = val.as_str() {
            new_doc.add_text(af.f_from_text, s);
            new_doc.add_text(af.f_from_text_tri, s);
        }
    }
    for field in [af.f_from_name, af.f_wire_from] {
        for val in old_doc.get_all(field) {
            if let Some(s) = val.as_str() {
                new_doc.add_text(af.f_from_text, s);
                new_doc.add_text(af.f_from_text_tri, s);
            }
        }
    }
    if let Some(s) = old_doc.get_first(af.f_name_exact).and_then(|v| v.as_str()) {
        new_doc.add_text(af.f_name_text, s);
        new_doc.add_text(af.f_name_text_tri, s);
    }

    let was_indexed = old_doc
        .get_first(af.f_is_indexed)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let extracted = if was_indexed {
        envelope_searcher.and_then(|searcher| reextract_attachment_text(searcher, old_doc, af))
    } else {
        None
    };

    match extracted {
        Some(t) => {
            new_doc.add_text(af.f_text, &t.text);
            new_doc.add_bool(af.f_has_text, true);
            new_doc.add_bool(af.f_is_indexed, true);
            new_doc.add_bool(af.f_is_ocr, t.is_ocr);
            if let Some(pc) = t.page_count {
                new_doc.add_u64(af.f_page_count, pc as u64);
            }
        }
        None => {
            new_doc.add_bool(af.f_has_text, false);
            new_doc.add_bool(af.f_is_indexed, false);
            new_doc.add_bool(af.f_is_ocr, false);
        }
    }

    for tag in new_tags {
        new_doc.add_facet(af.f_tags, &tag);
    }

    new_doc
}

impl IndexManager {
    pub(crate) fn index_writer(&self) -> &Arc<Mutex<WriterSlot>> {
        &self.index_writer
    }

    pub async fn shutdown(&self) {
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.take() {
            let _ = handle.await;
        }
    }
    /// Creates the attachment index writer with Bichon's merge policy. Used at
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
        let index = Arc::new(Self::open_or_create_index(&DATA_DIR_MANAGER.attachment_dir));
        index.tokenizers().register("euro", EuroTokenizer::new());
        index.tokenizers().register("tri", tri_analyzer());

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
                                        "Tantivy attach: backup flush committing {} docs",
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
                                        "Tantivy attach: wait_merging_threads failed, index stays consistent: {e:#?}"
                                    );
                                }
                                let fresh = loop {
                                    match IndexManager::create_index_writer(&index_bg) {
                                        Ok(w) => break w,
                                        Err(e) => {
                                            tracing::error!(
                                                "Tantivy attach: failed to recreate index writer after backup flush: {e:#?}; retrying"
                                            );
                                            tokio::time::sleep(Duration::from_millis(500)).await;
                                        }
                                    }
                                };
                                writer.put(fresh);
                                let _ = done.send(());
                            }
                            None => {
                                tracing::info!("Tantivy attach: flush channel closed.");
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
                                        "Tantivy attach: committed {} docs, pending reset to 0",
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
                                "Tantivy attach: periodic commit ({} docs pending)",
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
    /// manager inside a write window.
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
        let _ = self.sender.send(doc).await;
    }

    fn open_or_create_index(index_dir: &PathBuf) -> Index {
        let need_create = !index_dir.exists()
            || index_dir
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true);
        if need_create {
            info!(
                "Attachment index not found or empty, creating new index at {}",
                index_dir.display()
            );
            std::fs::create_dir_all(&index_dir).unwrap_or_else(|e| {
                panic!("Failed to create index directory {:?}: {}", index_dir, e)
            });
            let index = Index::create_in_dir(&index_dir, SchemaTools::attachment_schema())
                .unwrap_or_else(|e| panic!("Failed to create index in {:?}: {}", index_dir, e));
            // Fresh installs get stamped with the current index format version.
            // The open branch never writes — this keeps an old index from being
            // silently upgraded in place.
            if let Err(e) = crate::store::tantivy::format::write_index_format_version(index_dir) {
                tracing::warn!(error = %e, "failed to write INDEX_FORMAT marker for {:?}", index_dir);
            }
            index
        } else {
            info!(
                "Opening existing attachment index at {}",
                index_dir.display()
            );
            // Self-heal a marker lost to a crash between creating the index and
            // stamping it. Only an index whose schema already carries the trigram
            // sibling field is stamped; a legacy index never is, so it still hits
            // the reindex gate.
            if crate::store::tantivy::format::heal_index_format_marker(index_dir) {
                tracing::warn!(dir = ?index_dir, "restored a missing INDEX_FORMAT marker");
            }
            Self::open(&index_dir)
        }
    }

    fn open(index_dir: &PathBuf) -> Index {
        Index::open_in_dir(index_dir)
            .unwrap_or_else(|e| panic!("Failed to open index in {:?}: {}", index_dir, e))
    }

    fn account_query(&self, account_id: u64) -> Box<TermQuery> {
        let account_term =
            Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, account_id);
        Box::new(TermQuery::new(account_term, IndexRecordOption::Basic))
    }

    fn mailbox_query(&self, account_id: u64, mailbox_id: u64) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let mailbox_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::attachment_fields().f_mailbox_id, mailbox_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(mailbox_query)),
        ]);
        Box::new(boolean_query)
    }

    fn attachment_query(&self, account_id: u64, aid: &str) -> Box<dyn Query> {
        let account_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let envelope_id_query = TermQuery::new(
            Term::from_field_text(SchemaTools::attachment_fields().f_envelope_id, aid),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_id_query)),
            (Occur::Must, Box::new(envelope_id_query)),
        ]);
        Box::new(boolean_query)
    }

    pub fn total_attachments(&self, accounts: &Option<HashSet<u64>>) -> BichonResult<u64> {
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

    fn filter_query(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: AttachmentSearchFilter,
    ) -> BichonResult<Box<dyn Query>> {
        let f = SchemaTools::attachment_fields();
        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Default: space-separated terms are AND-ed. A legacy saved search
        // carries `match_all: false`, restoring the OR behaviour it was authored
        // under — for the word layer and the trigram layer alike.
        let match_all = filter.match_all.unwrap_or(true);
        let parse: fn(&Index, Vec<tantivy::schema::Field>) -> tantivy::query::QueryParser =
            if match_all { text_parser } else { any_term_parser };
        let combine: fn(Box<dyn Query>, &[tantivy::schema::Field], &str) -> Box<dyn Query> =
            if match_all { word_or_tri } else { word_or_tri_any };

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
                parse(&self.index, SchemaTools::attachment_default_fields());

            let word = query_parser
                .parse_query(text)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            let tri_fields = [f.f_subject_tri, f.f_from_text_tri, f.f_name_text_tri];
            subqueries.push((Occur::Must, combine(word, &tri_fields, text)));
        }

        if let Some(ref subject_val) = filter.subject {
            let query_parser = parse(&self.index, vec![f.f_subject]);
            let word = query_parser
                .parse_query(subject_val)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((
                Occur::Must,
                combine(word, &[f.f_subject_tri], subject_val),
            ));
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

        if let Some(from_query) = &filter.from {
            let query_parser = parse(&self.index, vec![f.f_from_text]);
            let word = query_parser
                .parse_query(from_query)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, combine(word, &[f.f_from_text_tri], from_query)))
        }

        if let Some(content_hash) = &filter.content_hash {
            let term = Term::from_field_text(f.f_content_hash, content_hash);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(id) = &filter.id {
            let term = Term::from_field_text(f.f_id, id);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref name) = filter.attachment_name {
            let query_parser =
                parse(&self.index, vec![f.f_name_text, f.f_name_exact]);

            let word = query_parser
                .parse_query(name)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, combine(word, &[f.f_name_text_tri], name)));
        }

        if let Some(ref extension) = filter.attachment_extension {
            let term = Term::from_field_text(f.f_ext, extension);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref category) = filter.attachment_category {
            let term = Term::from_field_text(f.f_category, category);
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            subqueries.push((Occur::Must, Box::new(query)));
        }

        if let Some(ref content_type) = filter.attachment_content_type {
            let term = Term::from_field_text(f.f_content_type, content_type);
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

        let mut add_bool_filter = |field: Field, value: Option<bool>| {
            if let Some(v) = value {
                let term = Term::from_field_bool(field, v);
                let query = TermQuery::new(term, IndexRecordOption::Basic);
                subqueries.push((Occur::Must, Box::new(query)));
            }
        };

        add_bool_filter(f.f_is_ocr, filter.is_ocr);
        add_bool_filter(f.f_is_message, filter.is_message);
        add_bool_filter(f.f_has_text, filter.has_text);

        let start_bound = if let Some(from) = filter.min_page_count {
            Bound::Included(Term::from_field_u64(f.f_page_count, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.max_page_count {
            Bound::Included(Term::from_field_u64(f.f_page_count, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        if subqueries.is_empty() {
            return Ok(Box::new(AllQuery));
        }

        Ok(Box::new(BooleanQuery::new(subqueries)))
    }

    pub fn get_attachment_by_id(
        &self,
        account_id: u64,
        id: &str,
    ) -> BichonResult<Option<AttachmentModel>> {
        let searcher = self.create_searcher()?;
        let f = SchemaTools::attachment_fields();

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
                    Term::from_field_text(f.f_id, id),
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
            let attachment = AttachmentModel::from_tantivy_doc(&doc)?;
            Ok(Some(attachment))
        } else {
            Ok(None)
        }
    }

    pub fn top_10_largest_attachments(
        &self,
        accounts: &Option<HashSet<u64>>,
    ) -> BichonResult<Vec<LargestAttachment>> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term =
                        Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, id);
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

        let attachment_docs: Vec<(Option<u64>, DocAddress)> = searcher
            .search(
                &query,
                &TopDocs::with_limit(200).order_by_fast_field(F_SIZE, Order::Desc),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut result = Vec::new();
        let mut seen_hashes = std::collections::HashSet::new();

        for (_, doc_address) in attachment_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let att = LargestAttachment::from_tantivy_doc(&doc)?;
            if seen_hashes.insert(att.content_hash.clone()) {
                result.push(att);
            }
            if result.len() >= 10 {
                break;
            }
        }
        Ok(result)
    }

    pub async fn delete_account_attachments(&self, account_id: u64) -> BichonResult<()> {
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
        let query = self.account_query(account_id);
        let mut writer = self.index_writer.lock().await;
        writer
            .delete_query(query)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub async fn delete_mailbox_attachments(
        &self,
        account_id: u64,
        mailbox_ids: Vec<u64>,
    ) -> BichonResult<()> {
        if mailbox_ids.is_empty() {
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        let mut queries: Vec<Box<dyn Query>> = Vec::with_capacity(mailbox_ids.len());
        for mailbox_id in mailbox_ids {
            queries.push(self.mailbox_query(account_id, mailbox_id));
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
        Ok(())
    }

    pub async fn delete_attachments_multi_account(
        &self,
        deletes: HashMap<u64, Vec<String>>,
    ) -> BichonResult<()> {
        if deletes.is_empty() {
            tracing::warn!("delete_envelopes_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;

        let mut writer = self.index_writer.lock().await;
        for (account_id, envelope_ids) in deletes {
            let unique_ids: HashSet<&String> = envelope_ids.iter().collect();
            if unique_ids.is_empty() {
                continue;
            }
            for eid in unique_ids {
                let query = self.attachment_query(account_id, eid);
                writer
                    .delete_query(query)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            }
        }
        writer
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    fn collect_facets_recursive(
        query: &dyn Query,
        searcher: &Searcher,
        parent_facet: &str,
        all_facets: &mut Vec<TagCount>,
        field_name: &str,
    ) -> BichonResult<()> {
        let mut facet_collector = FacetCollector::for_field(field_name);
        facet_collector.add_facet(parent_facet);

        let facet_counts = searcher
            .search(query, &facet_collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        for (facet, count) in facet_counts.get(parent_facet) {
            all_facets.push(TagCount {
                tag: facet.to_string(),
                count,
            });
            Self::collect_facets_recursive(
                query,
                searcher,
                &facet.to_string(),
                all_facets,
                field_name,
            )?;
        }

        Ok(())
    }

    pub fn get_all_tags(&self, accounts: Option<HashSet<u64>>) -> BichonResult<Vec<TagCount>> {
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term =
                        Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, id);
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
        Self::collect_facets_recursive(&query, &searcher, "/", &mut all_facets, F_TAGS)?;
        Ok(all_facets)
    }

    pub async fn update_attachment_tags(&self, request: TagsRequest) -> BichonResult<()> {

        if request.updates.is_empty() {
            tracing::warn!("update_attachment_tags: request is empty, nothing to update");
            return Ok(());
        }
        let _write_guard = WRITE_GATE.acquire(BACKUP_ACQUIRE_TIMEOUT).await?;
        let searcher = self.create_searcher()?;
        let mut writer = self.index_writer.lock().await;

        let af = SchemaTools::attachment_fields();
        let f_tags = af.f_tags;
        let f_id = af.f_id;
        let deduplicated_updates: HashMap<u64, HashSet<String>> = request
            .updates
            .into_iter()
            .map(|(account_id, envelope_ids)| (account_id, envelope_ids.into_iter().collect()))
            .collect();

        let mut operations = Vec::new();
        // Opened lazily, and only if a rebuilt attachment actually had extracted
        // text to restore.
        let mut envelope_searcher: Option<Searcher> = None;

        for (account_id, att_ids) in &deduplicated_updates {
            for aid in att_ids {
                let query = self.attachment_query(*account_id, aid);
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

                    // The text flags and `f_text` are re-derived together; the
                    // searcher is only opened when a document actually had text.
                    let was_indexed = old_doc
                        .get_first(af.f_is_indexed)
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if was_indexed && envelope_searcher.is_none() {
                        envelope_searcher = ENVELOPE_MANAGER.create_searcher().ok();
                        if envelope_searcher.is_none() {
                            tracing::warn!(
                                "attachment tag update: cannot open the envelope index; extracted attachment text will be dropped"
                            );
                        }
                    }
                    let new_doc =
                        rebuild_attachment_doc(&old_doc, af, current_tags, envelope_searcher.as_ref());

                    let delete_term = Term::from_field_text(f_id, aid);
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

    /// Search with a scope resolved from view accounts.
    ///
    /// Filtered sources are dropped — see
    /// `crate::account::access::unfiltered_source_accounts` — and the request's
    /// own `account_ids` are cleared because the scope already encodes them (a
    /// view id matches no attachment document).
    pub fn search_scoped(
        &self,
        scope: Option<Vec<ViewScopeGroup>>,
        mut filter: AttachmentSearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
        sort_by: SortBy,
    ) -> BichonResult<DataPage<AttachmentModel>> {
        filter.account_ids = None;
        let accounts: Option<HashSet<u64>> = scope.map(|groups| {
            crate::account::access::unfiltered_source_accounts(&groups)
                .into_iter()
                .collect()
        });
        self.search(accounts, filter, page, page_size, desc, sort_by)
    }

    pub fn search(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: AttachmentSearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
        sort_by: SortBy,
    ) -> BichonResult<DataPage<AttachmentModel>> {
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
        let attachment_docs: Vec<DocAddress>;

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
                attachment_docs = date_docs.into_iter().map(|(_, addr)| addr).collect();
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
                attachment_docs = size_docs.into_iter().map(|(_, addr)| addr).collect();
            }
            // Attachments carry no IMAP INTERNALDATE; fall back to the
            // attachment's own date field so the sort remains well defined.
            SortBy::InternalDate => {
                let date_docs: Vec<(Option<i64>, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_fast_field(F_DATE, order),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                attachment_docs = date_docs.into_iter().map(|(_, addr)| addr).collect();
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
                attachment_docs = ingest_at_docs.into_iter().map(|(_, addr)| addr).collect();
            }
            SortBy::Relevance => {
                // BM25 relevance: Tantivy scores the whole query. Always
                // descending (a higher score = more relevant); the `desc`
                // flag has no meaning here. Attachment-content hits (f_text)
                // score the same way as subject/name hits.
                let score_docs: Vec<(f32, DocAddress)> = searcher
                    .search(
                        &query,
                        &TopDocs::with_limit(page_size as usize)
                            .and_offset(offset as usize)
                            .order_by_score(),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                attachment_docs = score_docs.into_iter().map(|(_, addr)| addr).collect();
            }
        }

        let mut result = Vec::new();

        for doc_address in attachment_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = AttachmentModel::from_tantivy_doc(&doc)?;
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

    pub fn get_all_senders(&self, accounts: Option<HashSet<u64>>) -> BichonResult<HashSet<String>> {
        let searcher = self.create_searcher()?;

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term =
                        Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, id);
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
            let contacts = extract_senders(&doc)?;
            for value in contacts {
                contacts_set.insert(value);
            }
        }
        Ok(contacts_set)
    }

    pub fn collect_attachment_metadata(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<AttachmentMetadata> {
        let searcher = self.create_searcher()?;
        let aggregations: Aggregations = serde_json::from_value(json!({
            "exts": {
                "terms": {
                    "field": F_ATTACHMENT_EXT,
                    "size": 1000
                }
            },
            "cats": {
                "terms": {
                    "field": F_ATTACHMENT_CATEGORY,
                    "size": 1000
                }
            },
            "content_types": {
                "terms": {
                    "field": F_ATTACHMENT_CONTENT_TYPE,
                    "size": 1000
                }
            },
        }))
        .unwrap();

        let query: Box<dyn Query> = match accounts {
            Some(ref ids) if !ids.is_empty() => {
                let mut subqueries = Vec::new();
                for &id in ids {
                    let term =
                        Term::from_field_u64(SchemaTools::attachment_fields().f_account_id, id);
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

        let mut exts = Vec::with_capacity(20);
        let extensions = agg_results.0.get("exts").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) = extensions {
            for entry in buckets {
                if let Key::Str(ext) = &entry.key {
                    exts.push(Group {
                        key: ext.clone(),
                        count: entry.doc_count,
                        account_id: None,
                    });
                }
            }
        }

        let mut cats = Vec::with_capacity(20);
        let categories = agg_results.0.get("cats").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) = categories {
            for entry in buckets {
                if let Key::Str(cat) = &entry.key {
                    cats.push(Group {
                        key: cat.clone(),
                        count: entry.doc_count,
                        account_id: None,
                    });
                }
            }
        }

        let mut ctypes = Vec::with_capacity(20);
        let content_types = agg_results.0.get("content_types").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) = content_types
        {
            for entry in buckets {
                if let Key::Str(content_type) = &entry.key {
                    ctypes.push(Group {
                        key: content_type.clone(),
                        count: entry.doc_count,
                        account_id: None,
                    });
                }
            }
        }

        Ok(AttachmentMetadata {
            extensions: exts,
            categories: cats,
            content_types: ctypes,
        })
    }
}

#[cfg(test)]
mod rebuild_tests {
    use super::*;

    fn old_attachment_doc(
        af: &crate::store::tantivy::fields::AttachmentFields,
    ) -> TantivyDocument {
        let mut d = TantivyDocument::new();
        d.add_text(af.f_id, "att-1");
        d.add_text(af.f_envelope_id, "env-1");
        d.add_text(af.f_content_hash, "hash-1");
        d.add_text(af.f_subject, "Rechnung");
        d.add_text(af.f_subject_tri, "Rechnung");
        d.add_text(af.f_from, "alice@example.com");
        d.add_text(af.f_from_text, "alice@example.com");
        d.add_text(af.f_from_text_tri, "alice@example.com");
        d.add_text(af.f_name_exact, "invoice.pdf");
        d.add_text(af.f_name_text, "invoice.pdf");
        d.add_text(af.f_name_text_tri, "invoice.pdf");
        d.add_bool(af.f_has_text, false);
        d.add_bool(af.f_is_indexed, false);
        d.add_bool(af.f_is_ocr, false);
        d.add_bool(af.f_is_message, false);
        d.add_facet(af.f_tags, "/old");
        d
    }

    /// A rebuild (tag edit) must re-derive every non-stored field — otherwise
    /// the address/name text and the trigram siblings silently disappear.
    #[test]
    fn rebuild_restores_non_stored_fields_and_replaces_tags() {
        let af = SchemaTools::attachment_fields();
        let old = old_attachment_doc(af);
        let new_doc = rebuild_attachment_doc(&old, af, vec!["/new".to_string()], None);

        for (field, expected) in [
            (af.f_subject_tri, "Rechnung"),
            (af.f_from_text, "alice@example.com"),
            (af.f_from_text_tri, "alice@example.com"),
            (af.f_name_text, "invoice.pdf"),
            (af.f_name_text_tri, "invoice.pdf"),
        ] {
            assert_eq!(
                new_doc.get_first(field).and_then(|v| v.as_str()),
                Some(expected),
                "non-stored field must be re-derived"
            );
        }

        let tags: Vec<String> = new_doc
            .get_all(af.f_tags)
            .filter_map(|v| v.as_facet())
            .filter_map(|enc| Facet::from_encoded(enc.as_bytes().to_vec()).ok())
            .map(|f| f.to_string())
            .collect();
        assert_eq!(tags, vec!["/new".to_string()], "tags replaced");

        // No text is available without a searcher/extractor, so the flags must be
        // consistent rather than claiming an index that does not exist.
        assert_eq!(
            new_doc.get_first(af.f_is_indexed).and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            new_doc.get_first(af.f_has_text).and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(new_doc.get_first(af.f_text).is_none());
    }
}
