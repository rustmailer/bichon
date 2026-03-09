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
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::modules::{
    duckdb::init::duckdb,
    message::{content::AttachmentInfo, search::SortBy, tags::TagCount},
    settings::cli::SETTINGS,
    utils::create_hash,
};
use crate::{
    modules::{
        common::signal::SIGNAL_MANAGER,
        dashboard::{DashboardStats, LargestEmail},
        error::{code::ErrorCode, BichonResult},
        indexer::{envelope::Envelope, schema::SchemaTools},
        message::search::SearchFilter,
        rest::response::DataPage,
        settings::dir::DATA_DIR_MANAGER,
    },
    raise_error,
};

use mail_parser::{MessageParser, MimeHeaders};

use tantivy::indexer::{LogMergePolicy, UserOperation};
use tantivy::{
    collector::TopDocs,
    query::{BooleanQuery, Occur, Query, TermQuery},
    schema::{IndexRecordOption, Value},
    store::{Compressor, ZstdCompressor},
    Index, IndexBuilder, IndexReader, IndexSettings, IndexWriter, TantivyDocument, Term,
};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    sync::{mpsc, Mutex},
    task,
};
use tracing::info;

pub static ENVELOPE_INDEX_MANAGER: LazyLock<EnvelopeIndexManager> =
    LazyLock::new(EnvelopeIndexManager::new);
pub static EML_INDEX_MANAGER: LazyLock<EmlIndexManager> = LazyLock::new(EmlIndexManager::new);

pub const ENVELOPE_BATCH_SIZE: usize = 500;
pub const EML_BATCH_SIZE: usize = 100;

const MAX_BUFFER_DURATION: Duration = Duration::from_secs(10);

pub enum MetadataOp {
    Record((u64, (Envelope, Vec<AttachmentInfo>))),
    Shutdown,
}

pub enum DocumentOp {
    Document((u64, TantivyDocument)),
    Flush(tokio::sync::oneshot::Sender<()>),
    Shutdown,
}

pub struct EnvelopeIndexManager {
    sender: mpsc::Sender<MetadataOp>,
}

impl EnvelopeIndexManager {
    pub fn new() -> Self {
        let (sender, mut receiver) = mpsc::channel::<MetadataOp>(1000);
        task::spawn(async move {
            let mut buffer: HashMap<u64, (Envelope, Vec<AttachmentInfo>)> =
                HashMap::with_capacity(ENVELOPE_BATCH_SIZE);
            let mut interval = tokio::time::interval(MAX_BUFFER_DURATION);
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            loop {
                tokio::select! {
                    maybe_msg = receiver.recv() => {
                        match maybe_msg {
                            Some(MetadataOp::Record((eid, doc))) => {
                                buffer.insert(eid, doc);
                                if buffer.len() >= ENVELOPE_BATCH_SIZE {
                                    ENVELOPE_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                }
                            }
                            Some(MetadataOp::Shutdown) => {
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
                        let _ = ENVELOPE_INDEX_MANAGER.sender.send(MetadataOp::Shutdown).await;
                    }
                }
            }
        });
        Self { sender }
    }

    pub async fn add_document(&self, eid: u64, doc: (Envelope, Vec<AttachmentInfo>)) {
        let _ = self.sender.send(MetadataOp::Record((eid, doc))).await;
    }

    /// No-op flush — DuckDB commits are synchronous in drain_and_commit.
    pub async fn flush(&self) {}

    async fn drain_and_commit(&self, buffer: &mut HashMap<u64, (Envelope, Vec<AttachmentInfo>)>) {
        if buffer.is_empty() {
            return;
        }

        let items: Vec<(Envelope, Vec<AttachmentInfo>)> = buffer.drain().map(|(_, v)| v).collect();

        let result = (|| -> BichonResult<()> {
            duckdb()?.append_envelopes_with_attachments(&items)?;
            Ok(())
        })();

        if let Err(e) = result {
            tracing::error!("Failed to drain and commit envelopes to DuckDB: {:#?}", e);
        }
    }

    pub async fn total_emails(&self, accounts: Option<HashSet<u64>>) -> BichonResult<u64> {
        tokio::task::spawn_blocking(move || duckdb()?.total_emails(accounts))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn delete_account_envelopes(&self, account_id: u64) -> BichonResult<()> {
        let _ =
            tokio::task::spawn_blocking(move || duckdb()?.delete_envelopes_by_account(account_id))
                .await
                .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?;
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

        let _ = tokio::task::spawn_blocking(move || {
            duckdb()?.delete_mailbox_envelopes(account_id, mailbox_ids)
        })
        .await
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub async fn get_all_tags(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<Vec<TagCount>> {
        tokio::task::spawn_blocking(move || duckdb()?.get_all_tags(accounts))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn get_all_contacts(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<HashSet<String>> {
        tokio::task::spawn_blocking(move || duckdb()?.get_all_contacts(accounts))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn delete_envelopes_multi_account(
        &self,
        deletes: HashMap<u64, Vec<u64>>, // HashMap<account_id, envelope_ids>
    ) -> BichonResult<()> {
        if deletes.is_empty() {
            tracing::warn!("delete_envelopes_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }

        tokio::task::spawn_blocking(move || duckdb()?.delete_envelopes_multi_account(deletes))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
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
        tokio::task::spawn_blocking(move || duckdb()?.update_envelope_tags(updates, tags))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn search(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: SearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
        sort_by: SortBy,
    ) -> BichonResult<DataPage<Envelope>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");
        tokio::task::spawn_blocking(move || {
            duckdb()?.search(accounts, filter, page, page_size, desc, sort_by)
        })
        .await
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
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
        tokio::task::spawn_blocking(move || {
            duckdb()?.list_mailbox_envelopes(account_id, mailbox_id, page, page_size, desc)
        })
        .await
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
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
        tokio::task::spawn_blocking(move || {
            duckdb()?.list_thread_envelopes(account_id, thread_id, page, page_size, desc)
        })
        .await
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn get_envelope_by_id(
        &self,
        account_id: u64,
        envelope_id: u64,
    ) -> BichonResult<Option<Envelope>> {
        tokio::task::spawn_blocking(move || duckdb()?.get_envelope_by_id(account_id, envelope_id))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn top_10_largest_emails(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<Vec<LargestEmail>> {
        tokio::task::spawn_blocking(move || duckdb()?.top_10_largest_emails(accounts))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn get_max_uid(&self, account_id: u64, mailbox_id: u64) -> BichonResult<Option<u64>> {
        tokio::task::spawn_blocking(move || duckdb()?.get_max_uid(account_id, mailbox_id))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn num_messages_in_mailbox(
        &self,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<u64> {
        tokio::task::spawn_blocking(move || {
            duckdb()?.num_messages_in_mailbox(account_id, mailbox_id)
        })
        .await
        .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn num_messages_in_thread(
        &self,
        account_id: u64,
        thread_id: u64,
    ) -> BichonResult<u64> {
        tokio::task::spawn_blocking(move || duckdb()?.num_messages_in_thread(account_id, thread_id))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }

    pub async fn count_messages_in_mailbox(
        &self,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<u64> {
        self.num_messages_in_mailbox(account_id, mailbox_id).await
    }

    /// Return all UIDs stored locally for a given mailbox.
    pub fn get_all_uids(
        &self,
        account_id: u64,
        mailbox_id: u64,
    ) -> BichonResult<HashSet<u32>> {
        duckdb()?.get_all_uids(account_id, mailbox_id)
    }

    pub async fn get_dashboard_stats(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<DashboardStats> {
        tokio::task::spawn_blocking(move || duckdb()?.get_dashboard_stats(accounts))
            .await
            .map_err(|e| raise_error!(format!("{:?}", e), ErrorCode::InternalError))?
    }
}

pub struct EmlIndexManager {
    index_writer: Arc<Mutex<IndexWriter>>,
    sender: mpsc::Sender<DocumentOp>,
    reader: IndexReader,
}

impl EmlIndexManager {
    pub fn new() -> Self {
        let index = Self::open_or_create_index(&DATA_DIR_MANAGER.eml_dir);

        let writer: IndexWriter<TantivyDocument> = index
            .writer_with_num_threads(
                SETTINGS.bichon_tantivy_threads as usize,
                SETTINGS.bichon_tantivy_buffer_size,
            )
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to create IndexWriter (threads: {}, buffer: {}B) for {:?}: {}",
                    SETTINGS.bichon_tantivy_threads,
                    SETTINGS.bichon_tantivy_buffer_size,
                    DATA_DIR_MANAGER.eml_dir,
                    e
                )
            });

        let mut merge_policy = LogMergePolicy::default();
        merge_policy.set_min_num_segments(20);
        merge_policy.set_max_docs_before_merge(10_000);
        merge_policy.set_min_layer_size(1000);
        writer.set_merge_policy(Box::new(merge_policy));

        let index_writer = Arc::new(Mutex::new(writer));

        let reader = index.reader().unwrap_or_else(|e| {
            panic!(
                "Failed to create IndexReader for {:?}: {}",
                DATA_DIR_MANAGER.eml_dir, e
            )
        });
        let (sender, mut receiver) = mpsc::channel::<DocumentOp>(100);
        task::spawn(async move {
            let mut buffer: HashMap<u64, TantivyDocument> = HashMap::with_capacity(EML_BATCH_SIZE);
            let mut interval = tokio::time::interval(MAX_BUFFER_DURATION);
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            loop {
                tokio::select! {
                    maybe_msg = receiver.recv() => {
                        match maybe_msg {
                            Some(DocumentOp::Document((eid, doc))) => {
                                buffer.insert(eid, doc);
                                if buffer.len() >= EML_BATCH_SIZE {
                                    EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                }
                            }
                            Some(DocumentOp::Shutdown) => {
                                EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                break;
                            }
                            Some(DocumentOp::Flush(tx)) => {
                                EML_INDEX_MANAGER.drain_and_commit(&mut buffer).await;
                                let _ = tx.send(());
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
                        let _ = EML_INDEX_MANAGER.sender.send(DocumentOp::Shutdown).await;
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
    /// Adds a document to the indexer.
    ///
    /// # Parameters
    /// - `eid`: A hash derived from **Account ID + Message ID**.
    ///   This acts as a unique identifier for the EML content itself.
    ///
    /// - `doc`: The `TantivyDocument` representing the mail body/content.
    ///
    /// # Logical Design
    /// Unlike the `envelope_id` (which is a hash of Account + Folder + Message ID),
    /// this `eid` ignores the folder context. This ensures that while metadata
    /// (envelopes) can be duplicated across different folders, the physical
    /// EML/document storage remains de-duplicated and unique.
    pub async fn add_document(&self, eid: u64, doc: TantivyDocument) {
        let _ = self.sender.send(DocumentOp::Document((eid, doc))).await;
    }

    /// Flush the write buffer and commit all pending documents to the index.
    pub async fn flush(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.sender.send(DocumentOp::Flush(tx)).await;
        let _ = rx.await;
    }

    fn open_or_create_index(index_dir: &PathBuf) -> Index {
        let need_create = !index_dir.exists()
            || index_dir
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true);

        if need_create {
            info!(
                "Email data storage not found or empty, creating new mail storage at {}",
                index_dir.display()
            );
            std::fs::create_dir_all(&index_dir).unwrap_or_else(|e| {
                panic!("Failed to create index directory {:?}: {}", index_dir, e)
            });
            IndexBuilder::new()
                .schema(SchemaTools::eml_schema())
                .settings(IndexSettings {
                    docstore_compression: Compressor::Zstd(ZstdCompressor {
                        compression_level: Some(SETTINGS.bichon_eml_compression_level as i32),
                    }),
                    docstore_compress_dedicated_thread: true,
                    docstore_blocksize: SETTINGS.bichon_eml_blocksize,
                })
                .create_in_dir(&index_dir)
                .unwrap_or_else(|e| panic!("Failed to create index in {:?}: {}", index_dir, e))
        } else {
            info!(
                "Opening existing email data storage at {}",
                index_dir.display()
            );
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

    pub async fn get(&self, account_id: u64, eml_id: u64) -> BichonResult<Option<Vec<u8>>> {
        let searcher = self.reader.searcher();
        let query = self.envelope_query(account_id, eml_id);
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
        let envelope = duckdb()?
            .get_envelope_by_id(account_id, eid)?
            .ok_or_else(|| {
                raise_error!(
                    format!(
                        "Email envelope not found: account_id={} id={}",
                        account_id, eid
                    ),
                    ErrorCode::ResourceNotFound
                )
            })?;
        let eml_id = create_hash(account_id, &envelope.message_id);
        let data = self.get(account_id, eml_id).await?.ok_or_else(|| {
            raise_error!(
                format!("Eml not found: account_id={}, eid={}", account_id, eid),
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
        let envelope = duckdb()?
            .get_envelope_by_id(account_id, eid)?
            .ok_or_else(|| {
                raise_error!(
                    format!(
                        "Email envelope not found: account_id={} id={}",
                        account_id, eid
                    ),
                    ErrorCode::ResourceNotFound
                )
            })?;
        let eml_id = create_hash(account_id, &envelope.message_id);
        let data = self.get(account_id, eml_id).await?.ok_or_else(|| {
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
            tracing::warn!("delete_email_multi_account: deletes is empty, nothing to delete");
            return Ok(());
        }

        let mut writer = self.index_writer.lock().await;

        for (account_id, envelope_ids) in deletes {
            let unique_ids: Vec<u64> = envelope_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            if unique_ids.is_empty() {
                continue;
            }

            for chunk in unique_ids.chunks(100) {
                let envelopes = duckdb()?.get_envelopes_by_ids(*account_id, chunk)?;
                let found_ids_set: HashSet<u64> = envelopes.iter().map(|e| e.id).collect();
                for &original_id in chunk {
                    if !found_ids_set.contains(&original_id) {
                        tracing::warn!(
                        "delete_email_multi_account: envelope not found in DB, skipping tantivy delete. account_id: {}, envelope_id: {}", 
                        account_id, original_id
                    );
                    }
                }

                for envelope in envelopes {
                    let hashed_id = create_hash(*account_id, &envelope.message_id);
                    let query = self.envelope_query(*account_id, hashed_id);

                    writer
                        .delete_query(query)
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                }
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
