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

use crate::modules::message::tags::TagCount;
use crate::{
    modules::{
        account::migration::AccountModel,
        common::signal::SIGNAL_MANAGER,
        dashboard::{DashboardStats, Group, LargestEmail, TimeBucket},
        error::{code::ErrorCode, BichonResult},
        indexer::{
            envelope::Envelope,
            fields::{
                F_ACCOUNT_ID, F_FROM, F_HAS_ATTACHMENT, F_INTERNAL_DATE, F_MAILBOX_ID, F_SIZE,
                F_TAGS, F_THREAD_ID, F_UID,
            },
            schema::SchemaTools,
        },
        message::search::SearchFilter,
        rest::response::DataPage,
        settings::dir::DATA_DIR_MANAGER,
    },
    raise_error, utc_now,
};
use chrono::Utc;
use mail_parser::{MessageParser, MimeHeaders};
use serde_json::json;
use tantivy::{
    aggregation::{
        agg_req::Aggregations,
        agg_result::{
            AggregationResult, AggregationResults, BucketEntries, BucketResult, MetricResult,
        },
        AggregationCollector, Key,
    },
    collector::{Count, FacetCollector, TopDocs},
    query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery},
    schema::{Facet, IndexRecordOption, Value},
    store::{Compressor, ZstdCompressor},
    DocAddress, Index, IndexBuilder, IndexReader, IndexSettings, IndexWriter, Order,
    TantivyDocument, Term,
};
use tantivy::{indexer::UserOperation, Searcher};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    sync::{mpsc, Mutex},
    task,
};

pub static ENVELOPE_INDEX_MANAGER: LazyLock<EnvelopeIndexManager> =
    LazyLock::new(EnvelopeIndexManager::new);
pub static EML_INDEX_MANAGER: LazyLock<EmlIndexManager> = LazyLock::new(EmlIndexManager::new);

pub const ENVELOPE_BATCH_SIZE: usize = 1000;
pub const EML_BATCH_SIZE: usize = 200;

const MAX_BUFFER_DURATION: Duration = Duration::from_secs(30);

pub enum WriteMessage {
    Document((u64, TantivyDocument)),
    Shutdown,
}

pub struct EnvelopeIndexManager {
    index_writer: Arc<Mutex<IndexWriter>>,
    sender: mpsc::Sender<WriteMessage>,
    reader: IndexReader,
    query_parser: QueryParser,
}

impl EnvelopeIndexManager {
    pub fn new() -> Self {
        let index = Self::open_or_create_index(&DATA_DIR_MANAGER.envelope_dir);
        let index_writer = Arc::new(Mutex::new(
            index
                .writer_with_num_threads(8, 536_870_912)
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to create IndexWriter with 8 threads and 512MB buffer for {:?}: {}",
                        DATA_DIR_MANAGER.envelope_dir, e
                    )
                }),
        ));
        let reader = index.reader().unwrap_or_else(|e| {
            panic!(
                "Failed to create IndexReader for {:?}: {}",
                DATA_DIR_MANAGER.envelope_dir, e
            )
        });
        let mut query_parser =
            QueryParser::for_index(&index, SchemaTools::envelope_default_fields());
        query_parser.set_conjunction_by_default();

        let (sender, mut receiver) = mpsc::channel::<WriteMessage>(1000);
        task::spawn(async move {
            let mut buffer: HashMap<u64, TantivyDocument> =
                HashMap::with_capacity(ENVELOPE_BATCH_SIZE);
            let mut interval = tokio::time::interval(MAX_BUFFER_DURATION);
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            loop {
                tokio::select! {
                    maybe_msg = receiver.recv() => {
                        match maybe_msg {
                            Some(WriteMessage::Document((eid, doc))) => {
                                buffer.insert(eid, doc);
                                if buffer.len() >= ENVELOPE_BATCH_SIZE {
                                    ENVELOPE_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                }
                            }
                            Some(WriteMessage::Shutdown) => {
                                ENVELOPE_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = interval.tick() => {
                        if !buffer.is_empty() {
                            ENVELOPE_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                        }
                    }
                    _ = shutdown.recv() => {
                        let _ = ENVELOPE_INDEX_MANAGER.sender.send(WriteMessage::Shutdown).await;
                    }
                }
            }
        });
        Self {
            index_writer,
            sender,
            reader,
            query_parser,
        }
    }

    pub async fn add_document(&self, eid: u64, doc: TantivyDocument) {
        let _ = self.sender.send(WriteMessage::Document((eid, doc))).await;
    }

    async fn drain_and_commit(&self, buffer: &mut HashMap<u64, TantivyDocument>) {
        if buffer.is_empty() {
            return;
        }
        let mut writer = self.index_writer.lock().await;
        let mut operations = Vec::new();

        for (eid, doc) in buffer.drain() {
            let delete_term = Term::from_field_u64(SchemaTools::envelope_fields().f_id, eid);
            operations.push(UserOperation::Delete(delete_term));
            operations.push(UserOperation::Add(doc));
        }
        if let Err(e) = writer.run(operations) {
            eprintln!("[FATAL] Tantivy run failed: {e:?}");
            std::process::exit(1);
        }

        fatal_commit(&mut writer);
    }

    fn open_or_create_index(index_dir: &PathBuf) -> Index {
        if !index_dir.exists() {
            std::fs::create_dir_all(&index_dir).unwrap_or_else(|e| {
                panic!("Failed to create index directory {:?}: {}", index_dir, e)
            });
            Index::create_in_dir(&index_dir, SchemaTools::envelope_schema())
                .unwrap_or_else(|e| panic!("Failed to create index in {:?}: {}", index_dir, e))
        } else {
            open(&index_dir)
        }
    }

    pub fn total_emails(&self) -> BichonResult<u64> {
        let searcher = self.create_searcher()?;
        Ok(searcher.num_docs())
    }

    fn account_query(&self, account_id: u64) -> Box<TermQuery> {
        let account_term =
            Term::from_field_u64(SchemaTools::envelope_fields().f_account_id, account_id);
        Box::new(TermQuery::new(account_term, IndexRecordOption::Basic))
    }

    fn mailbox_query(&self, account_id: u64, mailbox_id: u64) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let mailbox_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_mailbox_id, mailbox_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(mailbox_query)),
        ]);
        Box::new(boolean_query)
    }

    fn filter_query(
        &self,
        filter: SearchFilter,
        parser: QueryParser,
    ) -> BichonResult<Box<dyn Query>> {
        let f = SchemaTools::envelope_fields();
        let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        if let Some(ref text) = filter.text {
            let query = parser
                .parse_query(text)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
            subqueries.push((Occur::Must, Box::new(query)));
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
            (f.f_from, &filter.from),
            (f.f_to, &filter.to),
            (f.f_cc, &filter.cc),
            (f.f_bcc, &filter.bcc),
        ] {
            if let Some(ref v) = opt_value {
                let term = Term::from_field_text(field, v);
                subqueries.push((
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            }
        }

        if let Some(has) = filter.has_attachment {
            if has {
                subqueries.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        Term::from_field_bool(f.f_has_attachment, true.into()),
                        IndexRecordOption::Basic,
                    )),
                ));
            }
        }

        if let Some(ref name) = filter.attachment_name {
            let term = Term::from_field_text(f.f_attachments, name);
            subqueries.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        let start_bound = if let Some(from) = filter.since {
            Bound::Included(Term::from_field_i64(f.f_internal_date, from))
        } else {
            Bound::Unbounded
        };

        let end_bound = if let Some(to) = filter.before {
            Bound::Included(Term::from_field_i64(f.f_internal_date, to))
        } else {
            Bound::Unbounded
        };

        if start_bound != Bound::Unbounded || end_bound != Bound::Unbounded {
            let q = RangeQuery::new(start_bound, end_bound);
            subqueries.push((Occur::Must, Box::new(q)));
        }

        if let Some(account_id) = filter.account_id {
            let term = Term::from_field_u64(f.f_account_id, account_id);
            subqueries.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }

        if let Some(mailbox_id) = filter.mailbox_id {
            let term = Term::from_field_u64(f.f_mailbox_id, mailbox_id);
            subqueries.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
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

    fn thread_query(&self, account_id: u64, thread_id: u64) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let thread_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_thread_id, thread_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(thread_query)),
        ]);
        Box::new(boolean_query)
    }

    fn envelope_query(&self, account_id: u64, eid: u64) -> Box<dyn Query> {
        let account_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let envelope_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::envelope_fields().f_id, eid),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_id_query)),
            (Occur::Must, Box::new(envelope_id_query)),
        ]);
        Box::new(boolean_query)
    }

    pub async fn delete_account_envelopes(&self, account_id: u64) -> BichonResult<()> {
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

    pub async fn delete_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_ids: Vec<u64>,
    ) -> BichonResult<()> {
        if mailbox_ids.is_empty() {
            tracing::warn!("delete_mailbox_envelopes: mailbox_ids is empty, nothing to delete");
            return Ok(());
        }
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

    fn collect_facets_recursive(
        searcher: &Searcher,
        parent_facet: &str,
        all_facets: &mut Vec<TagCount>,
    ) -> BichonResult<()> {
        let mut facet_collector = FacetCollector::for_field(F_TAGS);
        facet_collector.add_facet(parent_facet);
        let facet_counts = searcher
            .search(&AllQuery, &facet_collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        for (facet, count) in facet_counts.get(parent_facet) {
            all_facets.push(TagCount {
                tag: facet.to_string(),
                count,
            });
            Self::collect_facets_recursive(searcher, &facet.to_string(), all_facets)?;
        }

        Ok(())
    }

    pub async fn get_all_tags(&self) -> BichonResult<Vec<TagCount>> {
        let searcher = self.reader.searcher();
        let mut all_facets = Vec::new();
        Self::collect_facets_recursive(&searcher, "/", &mut all_facets)?;
        Ok(all_facets)
    }

    pub async fn delete_envelopes_multi_account(
        &self,
        deletes: &HashMap<u64, Vec<u64>>, // HashMap<account_id, envelope_ids>
    ) -> BichonResult<()> {
        if deletes.is_empty() {
            tracing::warn!("delete_envelopes_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }

        let mut writer = self.index_writer.lock().await;

        for (account_id, envelope_ids) in deletes {
            let unique_ids: HashSet<u64> = envelope_ids.iter().copied().collect();
            if unique_ids.is_empty() {
                continue;
            }
            for eid in unique_ids {
                let query = self.envelope_query(*account_id, eid);
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

    pub async fn update_envelope_tags(
        &self,
        updates: HashMap<u64, Vec<u64>>, // HashMap<account_id, envelope_ids>
        tags: Vec<String>,
    ) -> BichonResult<()> {
        if updates.is_empty() {
            tracing::warn!("update_envelope_tags: updates is empty, nothing to update");
            return Ok(());
        }

        let searcher = self.create_searcher()?;
        let mut writer = self.index_writer.lock().await;

        let f_tags = SchemaTools::envelope_fields().f_tags;
        let f_id = SchemaTools::envelope_fields().f_id;
        let deduplicated_updates: HashMap<u64, HashSet<u64>> = updates
            .into_iter()
            .map(|(account_id, envelope_ids)| (account_id, envelope_ids.into_iter().collect()))
            .collect();

        let mut operations = Vec::new();

        for (account_id, envelope_ids) in &deduplicated_updates {
            for eid in envelope_ids {
                let query = self.envelope_query(*account_id, *eid);
                let docs = searcher
                    .search(query.as_ref(), &TopDocs::with_limit(1))
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                if let Some((_, doc_address)) = docs.first() {
                    let old_doc: TantivyDocument = searcher
                        .doc_async(*doc_address)
                        .await
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

                    let mut new_doc = TantivyDocument::new();
                    for (field, value) in old_doc.field_values() {
                        if field != f_tags {
                            new_doc.add_field_value(field, value);
                        }
                    }
                    for tag in &tags {
                        new_doc.add_facet(f_tags, tag);
                    }

                    let delete_term = Term::from_field_u64(f_id, *eid);
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

    pub async fn search(
        &self,
        filter: SearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        let query = self.filter_query(filter, self.query_parser.clone())?;
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
        let mailbox_docs: Vec<(i64, DocAddress)> = searcher
            .search(
                &query,
                &TopDocs::with_limit(page_size as usize)
                    .and_offset(offset as usize)
                    .order_by_fast_field(F_INTERNAL_DATE, order),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in mailbox_docs {
            let doc: TantivyDocument = searcher
                .doc_async(doc_address)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = Envelope::from_tantivy_doc(&doc).await?;
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

    pub async fn list_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_id: u64,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        let searcher = self.create_searcher()?;
        let total = self
            .num_messages_in_mailbox(&searcher, account_id, mailbox_id)
            .await?;
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
        let query = self.mailbox_query(account_id, mailbox_id);
        let order = if desc { Order::Desc } else { Order::Asc };
        let mailbox_docs: Vec<(i64, DocAddress)> = searcher
            .search(
                query.as_ref(),
                &TopDocs::with_limit(page_size as usize)
                    .and_offset(offset as usize)
                    .order_by_fast_field(F_INTERNAL_DATE, order),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in mailbox_docs {
            let doc: TantivyDocument = searcher
                .doc_async(doc_address)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = Envelope::from_tantivy_doc(&doc).await?;
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

    pub async fn list_thread_envelopes(
        &self,
        account_id: u64,
        thread_id: u64,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        let searcher = self.create_searcher()?;
        let total = self
            .num_messages_in_thread(&searcher, account_id, thread_id)
            .await?;
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
        let thread_docs: Vec<(i64, DocAddress)> = searcher
            .search(
                query.as_ref(),
                &TopDocs::with_limit(page_size as usize)
                    .and_offset(offset as usize)
                    .order_by_fast_field(F_INTERNAL_DATE, order),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in thread_docs {
            let doc: TantivyDocument = searcher
                .doc_async(doc_address)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = Envelope::from_tantivy_doc(&doc).await?;
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

    pub async fn get_envelope_by_id(
        &self,
        account_id: u64,
        message_id: u64,
    ) -> BichonResult<Option<Envelope>> {
        let searcher = self.create_searcher()?;
        let f = SchemaTools::envelope_fields();

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
                    Term::from_field_u64(f.f_id, message_id),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);

        let docs: Vec<(f32, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if let Some((_, doc_address)) = docs.first() {
            let doc: TantivyDocument = searcher
                .doc_async(*doc_address)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = Envelope::from_tantivy_doc(&doc).await?;
            Ok(Some(envelope))
        } else {
            Ok(None)
        }
    }

    pub async fn top_10_largest_emails(&self) -> BichonResult<Vec<LargestEmail>> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let searcher = self.reader.searcher();

        let mailbox_docs: Vec<(u64, DocAddress)> = searcher
            .search(
                &AllQuery,
                &TopDocs::with_limit(10).order_by_fast_field(F_SIZE, Order::Desc),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let mut result = Vec::new();

        for (_, doc_address) in mailbox_docs {
            let doc: TantivyDocument = searcher
                .doc_async(doc_address)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let envelope = LargestEmail::from_tantivy_doc(&doc)?;
            result.push(envelope);
        }
        Ok(result)
    }

    pub async fn get_max_uid(&self, account_id: u64, mailbox_id: u64) -> BichonResult<Option<u64>> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let searcher = self.reader.searcher();

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
        Ok(Self::extract_max_uid(&agg_res))
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

    pub async fn num_messages_in_mailbox(
        &self,
        searcher: &Searcher,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<u64> {
        let query = self.mailbox_query(account_id, mailbox_id);

        let agg_req: Aggregations = serde_json::from_value(json!({
            "mailbox_count": {
                "value_count": {
                    "field": F_MAILBOX_ID
                }
            }
        }))
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let collector = AggregationCollector::from_aggs(agg_req, Default::default());

        let agg_res = searcher
            .search(query.as_ref(), &collector)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Self::extract_mailbox_count(&agg_res)
    }

    fn extract_mailbox_count(agg_res: &AggregationResults) -> BichonResult<u64> {
        let Some(result) = agg_res.0.get("mailbox_count") else {
            return Err(raise_error!(
                "Missing aggregation result: mailbox_count".into(),
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

    pub async fn num_messages_in_thread(
        &self,
        searcher: &Searcher,
        account_id: u64,
        thread_id: u64,
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
        Self::extract_thread_count(&agg_res)
    }

    fn extract_thread_count(agg_res: &AggregationResults) -> BichonResult<u64> {
        let Some(result) = agg_res.0.get("thread_count") else {
            return Err(raise_error!(
                "Missing aggregation result: thread_count".into(),
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

    fn create_searcher(&self) -> BichonResult<Searcher> {
        self.reader
            .reload()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(self.reader.searcher())
    }

    pub async fn get_dashboard_stats(&self) -> BichonResult<DashboardStats> {
        let searcher = self.create_searcher()?;
        let now_ms = utc_now!();
        let week_ago_ms = (Utc::now() - Duration::from_secs(60 * 60 * 24 * 30)).timestamp_millis();

        let aggregations: Aggregations = serde_json::from_value(json!({
            "total_size": {
                "sum": { "field": F_SIZE }
            },
            "recent_30d_histogram": {
                "histogram": {
                    "field": F_INTERNAL_DATE,
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
                "terms": {
                    "field": F_HAS_ATTACHMENT
                }
            }
        }))
        .unwrap();

        let query = AllQuery;
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
                    top_accounts.push(Group {
                        key: AccountModel::get(*account_id).await?.email,
                        count: entry.doc_count,
                    });
                }
            }
        }
        stats.top_accounts = top_accounts;

        let attachment_stats = agg_results.0.get("attachment_stats").unwrap();
        if let AggregationResult::BucketResult(BucketResult::Terms { buckets, .. }) =
            attachment_stats
        {
            for entry in buckets {
                if let Key::U64(key) = entry.key {
                    if key == 0 {
                        stats.without_attachment_count = entry.doc_count;
                    }
                    if key == 1 {
                        stats.with_attachment_count = entry.doc_count;
                    }
                }
            }
        }
        Ok(stats)
    }
}

pub struct EmlIndexManager {
    index_writer: Arc<Mutex<IndexWriter>>,
    sender: mpsc::Sender<WriteMessage>,
    reader: IndexReader,
}

impl EmlIndexManager {
    pub fn new() -> Self {
        let index = Self::open_or_create_index(&DATA_DIR_MANAGER.eml_dir);
        let index_writer = Arc::new(Mutex::new(
            index
                .writer_with_num_threads(8, 536_870_912)
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to create IndexWriter with 8 threads and 512MB buffer for {:?}: {}",
                        DATA_DIR_MANAGER.eml_dir, e
                    )
                }),
        ));
        let reader = index.reader().unwrap_or_else(|e| {
            panic!(
                "Failed to create IndexReader for {:?}: {}",
                DATA_DIR_MANAGER.eml_dir, e
            )
        });
        let (sender, mut receiver) = mpsc::channel::<WriteMessage>(100);
        task::spawn(async move {
            let mut buffer: HashMap<u64, TantivyDocument> = HashMap::with_capacity(EML_BATCH_SIZE);
            let mut interval = tokio::time::interval(MAX_BUFFER_DURATION);
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            loop {
                tokio::select! {
                    maybe_msg = receiver.recv() => {
                        match maybe_msg {
                            Some(WriteMessage::Document((eid, doc))) => {
                                buffer.insert(eid, doc);
                                if buffer.len() >= EML_BATCH_SIZE {
                                    EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                }
                            }
                            Some(WriteMessage::Shutdown) => {
                                EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = interval.tick() => {
                        if !buffer.is_empty() {
                            EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                        }
                    }
                    _ = shutdown.recv() => {
                        let _ = EML_INDEX_MANAGER.sender.send(WriteMessage::Shutdown).await;
                    }
                }
            }
        });
        Self {
            index_writer,
            sender,
            reader,
        }
    }

    pub async fn add_document(&self, eid: u64, doc: TantivyDocument) {
        let _ = self.sender.send(WriteMessage::Document((eid, doc))).await;
    }

    fn open_or_create_index(index_dir: &PathBuf) -> Index {
        if !index_dir.exists() {
            std::fs::create_dir_all(&index_dir).unwrap_or_else(|e| {
                panic!("Failed to create index directory {:?}: {}", index_dir, e)
            });
            IndexBuilder::new()
                .schema(SchemaTools::eml_schema())
                .settings(IndexSettings {
                    docstore_compression: Compressor::Zstd(ZstdCompressor {
                        compression_level: Some(6),
                    }),
                    docstore_compress_dedicated_thread: true,
                    docstore_blocksize: 2_097_152,
                })
                .create_in_dir(&index_dir)
                .unwrap_or_else(|e| panic!("Failed to create index in {:?}: {}", index_dir, e))
        } else {
            open(&index_dir)
        }
    }

    fn envelope_query(&self, account_id: u64, eid: u64) -> Box<dyn Query> {
        let account_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::eml_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let envelope_id_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::eml_fields().f_id, eid),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_id_query)),
            (Occur::Must, Box::new(envelope_id_query)),
        ]);
        Box::new(boolean_query)
    }

    pub async fn get(&self, account_id: u64, eid: u64) -> BichonResult<Option<Vec<u8>>> {
        let searcher = self.reader.searcher();
        let query = self.envelope_query(account_id, eid);
        let docs = searcher
            .search(query.as_ref(), &TopDocs::with_limit(1))
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if docs.is_empty() {
            return Ok(None);
        }

        let (_, doc_address) = docs.first().unwrap();
        let doc: TantivyDocument = searcher
            .doc_async(*doc_address)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let fields = SchemaTools::eml_fields();
        let value = doc.get_first(fields.f_eml).ok_or_else(|| {
            raise_error!(
                format!("miss '{}' field in tantivy document", stringify!(field)),
                ErrorCode::InternalError
            )
        })?;
        let bytes = value.as_bytes().ok_or_else(|| {
            raise_error!(
                format!("'{}' field is not a bytes", stringify!(field)),
                ErrorCode::InternalError
            )
        })?;

        Ok(Some(bytes.to_vec()))
    }

    pub async fn get_reader(&self, account_id: u64, eid: u64) -> BichonResult<File> {
        let data = self.get(account_id, eid).await?.ok_or_else(|| {
            raise_error!(
                format!("Email not found: account_id={}, eid={}", account_id, eid),
                ErrorCode::ResourceNotFound
            )
        })?;
        let mut path = DATA_DIR_MANAGER.temp_dir.clone();

        path.push(format!("{eid}.eml"));
        {
            let mut file = File::create(&path)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            file.write_all(&data)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        let file = File::open(&path)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(file)
    }

    pub async fn get_attachment(
        &self,
        account_id: u64,
        eid: u64,
        file_name: &str,
    ) -> BichonResult<File> {
        let data = self.get(account_id, eid).await?.ok_or_else(|| {
            raise_error!(
                format!("Email not found: account_id={}, eid={}", account_id, eid),
                ErrorCode::ResourceNotFound
            )
        })?;
        let message = MessageParser::default().parse(&data).ok_or_else(|| {
            raise_error!(
                format!(
                    "Failed to parse email: account_id={}, eid={}",
                    account_id, eid
                ),
                ErrorCode::InternalError
            )
        })?;
        let target_attachment = message
            .attachments()
            .find(|p| p.attachment_name().is_some_and(|name| name == file_name));
        let content = match target_attachment {
            Some(att) => att.contents(),
            None => {
                return Err(raise_error!(
                    "Attachment not found".into(),
                    ErrorCode::ResourceNotFound
                ))
            }
        };
        let mut path = DATA_DIR_MANAGER.temp_dir.clone();
        path.push(format!("{eid}.{file_name}.eml"));
        {
            let mut file = File::create(&path)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            file.write_all(content)
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        let file = File::open(&path)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(file)
    }

    fn account_query(&self, account_id: u64) -> Box<TermQuery> {
        let account_term = Term::from_field_u64(SchemaTools::eml_fields().f_account_id, account_id);
        Box::new(TermQuery::new(account_term, IndexRecordOption::Basic))
    }

    pub async fn delete_account_envelopes(&self, account_id: u64) -> BichonResult<()> {
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

    fn mailbox_query(&self, account_id: u64, mailbox_id: u64) -> Box<dyn Query> {
        let account_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::eml_fields().f_account_id, account_id),
            IndexRecordOption::Basic,
        );
        let mailbox_query = TermQuery::new(
            Term::from_field_u64(SchemaTools::eml_fields().f_mailbox_id, mailbox_id),
            IndexRecordOption::Basic,
        );
        let boolean_query = BooleanQuery::new(vec![
            (Occur::Must, Box::new(account_query)),
            (Occur::Must, Box::new(mailbox_query)),
        ]);
        Box::new(boolean_query)
    }

    pub async fn delete_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_ids: Vec<u64>,
    ) -> BichonResult<()> {
        if mailbox_ids.is_empty() {
            tracing::warn!("delete_mailbox_envelopes: mailbox_ids is empty, nothing to delete");
            return Ok(());
        }
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

    pub async fn delete_email_multi_account(
        &self,
        deletes: &HashMap<u64, Vec<u64>>, // HashMap<account_id, envelope_ids>
    ) -> BichonResult<()> {
        if deletes.is_empty() {
            tracing::warn!("delete_envelopes_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }

        let mut writer = self.index_writer.lock().await;

        for (account_id, envelope_ids) in deletes {
            let unique_ids: HashSet<u64> = envelope_ids.iter().copied().collect();
            if unique_ids.is_empty() {
                continue;
            }
            for eid in unique_ids {
                let query = self.envelope_query(*account_id, eid);
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

    async fn drain_and_commit(&self, buffer: &mut HashMap<u64, TantivyDocument>) {
        if buffer.is_empty() {
            return;
        }
        let mut writer = self.index_writer.lock().await;
        let mut operations = Vec::new();

        for (eid, doc) in buffer.drain() {
            let delete_term = Term::from_field_u64(SchemaTools::eml_fields().f_id, eid);
            operations.push(UserOperation::Delete(delete_term));
            operations.push(UserOperation::Add(doc));
        }
        if let Err(e) = writer.run(operations) {
            eprintln!("[FATAL] Tantivy run failed: {e:?}");
            std::process::exit(1);
        }

        fatal_commit(&mut writer);
    }
}

fn fatal_commit(writer: &mut IndexWriter) {
    const MAX_RETRIES: usize = 3;
    const RETRY_DELAY_MS: u64 = 1000;

    for attempt in 0..=MAX_RETRIES {
        match writer.commit() {
            Ok(_) => {
                if attempt > 0 {
                    eprintln!("[INFO] Commit succeeded on attempt {}", attempt + 1);
                }
                return;
            }
            Err(e) => match &e {
                tantivy::TantivyError::IoError(io_error) => {
                    if attempt < MAX_RETRIES {
                        eprintln!(
                            "[WARN] Commit failed (attempt {}/{}): {:?}. Retrying in {}ms...",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            io_error,
                            RETRY_DELAY_MS * (attempt as u64 + 1)
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            RETRY_DELAY_MS * (attempt as u64 + 1),
                        ));
                    } else {
                        eprintln!(
                            "[FATAL] Tantivy commit failed after {} attempts: {:?}",
                            MAX_RETRIES + 1,
                            io_error
                        );
                        std::process::exit(1);
                    }
                }
                _ => {
                    eprintln!("[FATAL] Tantivy commit failed with non-IO error: {e:?}");
                    std::process::exit(1);
                }
            },
        }
    }
}

fn open(index_dir: &PathBuf) -> Index {
    Index::open_in_dir(index_dir)
        .unwrap_or_else(|e| panic!("Failed to open index in {:?}: {}", index_dir, e))
}
