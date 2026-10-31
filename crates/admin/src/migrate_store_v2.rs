use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use bytes::Bytes;

use bichon_core::{
    envelope::extractor::extract_references, message::content::AttachmentInfo,
    store::tantivy::tokenizers::{tri_analyzer, EuroTokenizer}, utils::compute_content_hash,
};

use bichon_blob::{Codec, Config, Engine};
use mail_parser::MessageParser;
use tantivy::{indexer::NoMergePolicy, Index, IndexWriter, TantivyDocument};
use uuid::Uuid;

use bichon_core::{
    account::migration::AccountModel,
    common::AddrVec,
    envelope::extractor::{compute_thread_id, generate_message_id, resolve_sender},
    archive::imap::stable_uid::AccountUidAssigner,
    error::{code::ErrorCode, BichonResult},
    raise_error,
    store::envelope::Envelope,
    store::tantivy::{
        model::{AttachmentModel, EnvelopeWithAttachments},
        schema::SchemaTools,
    },
    utc_now,
};

pub struct LegacyDirs {
    pub envelope_dir: PathBuf,
    pub eml_dir: PathBuf,
}

pub struct NewDirs {
    pub envelope_dir: PathBuf,
    pub attachment_dir: PathBuf,
    pub storage_dir: PathBuf,
}

impl LegacyDirs {
    pub fn new(index: PathBuf, data: PathBuf) -> Self {
        Self {
            envelope_dir: index,
            eml_dir: data,
        }
    }
}

impl NewDirs {
    pub fn new(index: PathBuf, data: PathBuf) -> Self {
        Self {
            envelope_dir: index.join("mail_metadata"),
            attachment_dir: index.join("attachment_metadata"),
            storage_dir: data,
        }
    }
}

pub struct DetachOutput {
    pub infos: Vec<AttachmentInfo>,
    pub blobs: Vec<(String, Bytes)>,
}

fn hex_to_raw_key(hex: &str) -> BichonResult<[u8; 32]> {
    let mut key = [0u8; 32];
    hex::decode_to_slice(hex, &mut key).map_err(|e| {
        raise_error!(
            format!("invalid content hash: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    Ok(key)
}

pub fn detach_attachments_standalone(
    original_body: &[u8],
    message: &mail_parser::Message<'_>,
) -> (Vec<u8>, DetachOutput) {
    // Delegate to the single canonical implementation shared with the live
    // ingest path, so a migrated store always matches what the runtime
    // writes and reads back. No extraction filter: offline migration does
    // not run text extraction.
    let detached = bichon_core::envelope::extractor::strip_attachment_bodies(
        original_body,
        message,
        None,
    );
    (
        detached.stripped_eml,
        DetachOutput {
            infos: detached.infos,
            blobs: detached.blobs,
        },
    )
}

pub struct NewIndexWriterV2 {
    pub envelope_writer: Option<IndexWriter>,
    pub attachment_writer: Option<IndexWriter>,
    pub engine: Engine,
    envelope_dir: PathBuf,
    attachment_dir: PathBuf,
    pending: usize,
    email_buf: Vec<([u8; 32], Vec<u8>)>,
    attachment_buf: Vec<([u8; 32], Vec<u8>)>,
    /// Per-account stable IMAP UID counter for the documents written here. The
    /// legacy store had no such concept, so every document gets a fresh number.
    uid_assigner: AccountUidAssigner,
}

impl NewIndexWriterV2 {
    pub fn open(dirs: NewDirs) -> BichonResult<Self> {
        // ── envelope index ──────────────────────────────────────────────
        std::fs::create_dir_all(&dirs.envelope_dir)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        let envelope_index = if dirs
            .envelope_dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
        {
            Index::create_in_dir(&dirs.envelope_dir, SchemaTools::email_schema())
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
        } else {
            Index::open_in_dir(&dirs.envelope_dir)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
        };

        envelope_index
            .tokenizers()
            .register("euro", EuroTokenizer::new());
        envelope_index
            .tokenizers()
            .register("tri", tri_analyzer());

        let envelope_writer = envelope_index
            .writer_with_num_threads(3, 256 * 1024 * 1024)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        envelope_writer.set_merge_policy(Box::new(NoMergePolicy));
        // ── attachment index ─────────────────────────────────────────────
        std::fs::create_dir_all(&dirs.attachment_dir)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        let attachment_index = if dirs
            .attachment_dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
        {
            Index::create_in_dir(&dirs.attachment_dir, SchemaTools::attachment_schema())
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
        } else {
            Index::open_in_dir(&dirs.attachment_dir)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
        };

        attachment_index
            .tokenizers()
            .register("euro", EuroTokenizer::new());
        attachment_index
            .tokenizers()
            .register("tri", tri_analyzer());
        let attachment_writer = attachment_index
            .writer_with_num_threads(3, 256 * 1024 * 1024)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        attachment_writer.set_merge_policy(Box::new(NoMergePolicy));

        // ── blob store (bichon-blob, not fjall) ───────────────────────────
        std::fs::create_dir_all(&dirs.storage_dir)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        let blob_dir = dirs.storage_dir.join("blobs");

        let mut config = Config::default();
        config.default_codec = Codec::Zstd;
        config.compress_threshold = 1024;
        config.flush_interval_secs = 0;
        config.gc_interval_secs = 0;

        let engine = Engine::open(&blob_dir, config)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        Ok(Self {
            envelope_writer: Some(envelope_writer),
            attachment_writer: Some(attachment_writer),
            engine,
            envelope_dir: dirs.envelope_dir,
            attachment_dir: dirs.attachment_dir,
            pending: 0,
            email_buf: Vec::new(),
            attachment_buf: Vec::new(),
            uid_assigner: AccountUidAssigner::new(),
        })
    }

    pub fn ingest(
        &mut self,
        eml_bytes: &[u8],
        account_id: u64,
        mailbox_id: u64,
        uid: u32,
        internal_date: i64,
    ) -> BichonResult<()> {
        let email_content_hash = compute_content_hash(eml_bytes);
        let email_raw_key = hex_to_raw_key(&email_content_hash)?;

        let message = MessageParser::new()
            .parse(eml_bytes)
            .ok_or_else(|| raise_error!("failed to parse eml".into(), ErrorCode::InternalError))?;

        if message.parts.is_empty() {
            return Err(raise_error!(
                "Malformed or completely empty EML (no parts found)".into(),
                ErrorCode::InternalError
            ));
        }
        // ── text / preview ────────────────────────────────────────────────
        let text = message
            .body_text(0)
            .map(|c| c.into_owned())
            .or_else(|| {
                message
                    .body_html(0)
                    .map(|html| bichon_core::utils::html::extract_text(html.as_ref()))
            })
            .unwrap_or_default();
        let text = bichon_core::utils::normalize_whitespace(&text);
        let preview = if text.chars().count() > 100 {
            text.chars().take(100).collect::<String>() + "..."
        } else {
            text.clone()
        };

        // ── headers ───────────────────────────────────────────────────────
        let message_id = message
            .message_id()
            .map(String::from)
            .unwrap_or_else(generate_message_id);

        let in_reply_to = message.in_reply_to().as_text().map(String::from);
        let references = extract_references(&message);
        let thread_id = compute_thread_id(in_reply_to, references, &message_id);

        let subject = message.subject().map(String::from).unwrap_or_default();
        let date = message.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0);
        let internal_date = if internal_date == 0 {
            date
        } else {
            internal_date
        };

        let parse_addrs = |addrs: Option<&mail_parser::Address<'_>>| {
            addrs
                .map(|addr| {
                    AddrVec::from(addr)
                        .0
                        .into_iter()
                        .filter_map(|a| a.address)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let account = AccountModel::get(account_id).ok();
        let sender = resolve_sender(
            &message,
            account
                .as_ref()
                .and_then(|a| a.override_from_headers.as_deref()),
        );
        let from = sender
            .address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let to = parse_addrs(message.to());
        let cc = parse_addrs(message.cc());
        let bcc = parse_addrs(message.bcc());

        // ── detach attachments → blob ──────────────────────────────────────
        let (stripped_eml, attachment_output) = detach_attachments_standalone(eml_bytes, &message);

        // Buffer for bulk write — sorted + flushed later.
        // Key is the raw 32-byte hash (not the hex string).
        self.email_buf
            .push((email_raw_key, stripped_eml));
        for (hash, data) in &attachment_output.blobs {
            let raw_key = hex_to_raw_key(hash)?;
            self.attachment_buf.push((raw_key, data.to_vec()));
        }

        // ── build envelope doc ────────────────────────────────────────────
        let envelope_id = Uuid::new_v4().to_string();
        let now = utc_now!();

        let attachment_docs: Vec<TantivyDocument> = attachment_output
            .infos
            .iter()
            .filter(|a| !a.inline || a.content_id.is_none())
            .map(|a| {
                AttachmentModel {
                    id: Uuid::new_v4().to_string(),
                    envelope_id: envelope_id.clone(),
                    account_id,
                    account_email: None,
                    mailbox_id,
                    mailbox_name: None,
                    subject: subject.clone(),
                    content_hash: a.content_hash.clone(),
                    from: from.clone(),
                    from_name: sender.name.clone(),
                    wire_from: sender.wire_from.clone(),
                    date,
                    ingest_at: now,
                    size: a.size as u64,
                    ext: a.get_extension(),
                    category: a.get_category().to_string(),
                    content_type: a.file_type.clone(),
                    shard_id: 0,
                    text: None,
                    has_text: false,
                    is_ocr: false,
                    page_count: None,
                    is_indexed: false,
                    is_message: a.is_message,
                    name: a.filename.clone(),
                    tags: None,
                    auto_tags: None,
                }
                .into_document()
            })
            .collect();

        let envelope = Envelope {
            id: envelope_id,
            message_id,
            account_id,
            mailbox_id,
            uid,
            // The legacy store predates stable UIDs: assign one from the
            // per-account counter (write-once from here on).
            imap_uid: self.uid_assigner.assign(account_id)?,
            // The legacy store never tracked cleanup moves.
            cleanup_moved: false,
            subject,
            preview,
            from,
            from_name: sender.name,
            wire_from: sender.wire_from,
            to,
            cc,
            bcc,
            date,
            internal_date,
            ingest_at: now,
            size: eml_bytes.len() as u32,
            thread_id,
            attachment_count: message.attachment_count(),
            regular_attachment_count: attachment_docs.len(),
            tags: None,
            account_email: None,
            account_name: None,
            mailbox_name: None,
            content_hash: email_content_hash,
        };

        let ea = EnvelopeWithAttachments {
            envelope,
            attachments: Some(attachment_output.infos),
        };
        let envelope_doc = ea.to_document(&text, 0)?;

        self.envelope_writer
            .as_mut()
            .unwrap()
            .add_document(envelope_doc)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        for doc in attachment_docs {
            self.attachment_writer
                .as_mut()
                .unwrap()
                .add_document(doc)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        }

        self.pending += 1;

        Ok(())
    }

    /// Commit pending Tantivy documents (mid-stream) — frees the in-memory
    /// term dictionary / postings that accumulate in the IndexWriter.
    fn commit_tantivy(&mut self) -> BichonResult<()> {
        if self.pending == 0 {
            return Ok(());
        }
        println!("Tantivy committing... this may take 2-3 minutes, please wait.");
        let start = Instant::now();
        if let Some(writer) = self.envelope_writer.as_mut() {
            writer
                .commit()
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        }
        if let Some(writer) = self.attachment_writer.as_mut() {
            writer
                .commit()
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        }
        println!("tantivy commit elapsed: {:#?}", start.elapsed());
        tracing::info!(count = self.pending, "committed tantivy batch");
        self.pending = 0;
        Ok(())
    }

    /// Persist the per-account stable UID counters this migration produced.
    ///
    /// Call it after [`Self::finish_writers`] (the index must be durable before
    /// the counter claims the numbers in it) and before the engine shuts down.
    /// `root` is the migration's `--bichon-root-dir`, whose `memdb/` the
    /// metadata step already created.
    pub fn finish_uids(&mut self, root: &Path) -> BichonResult<()> {
        let db = bichon_core::admin::meta::open_database(root.join("memdb"))?;
        self.uid_assigner.finish(&db)
    }

    /// Final commit + segment merge for Tantivy writers (called once at end).
    pub fn finish_writers(&mut self) -> BichonResult<()> {
        self.commit_tantivy()?;

        for (name, writer_opt) in [
            ("envelope", &mut self.envelope_writer),
            ("attachment", &mut self.attachment_writer),
        ] {
            if let Some(writer) = writer_opt.as_mut() {
                let seg_ids = writer
                    .index()
                    .searchable_segment_ids()
                    .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
                println!("merging {} {} segments...", seg_ids.len(), name);
                if seg_ids.len() > 1 {
                    let _ = writer.merge(&seg_ids);
                }
            }

            if let Some(writer) = writer_opt.take() {
                println!("waiting for {} merge to finish...", name);
                let start = std::time::Instant::now();
                let _ = writer.wait_merging_threads();
                println!("{} merge done: {:#?}", name, start.elapsed());
            }
        }

        // Migration builds the indexes with the current binary, so stamp the
        // current index format version (otherwise the server would treat the
        // freshly built indexes as legacy on startup).
        for dir in [&self.envelope_dir, &self.attachment_dir] {
            if let Err(e) = bichon_core::store::tantivy::format::write_index_format_version(dir) {
                tracing::warn!(error = %e, "failed to write INDEX_FORMAT marker for {:?}", dir);
            }
        }

        Ok(())
    }

    /// Write buffered blobs to the bichon-blob engine.
    /// Also commits the Tantivy writers to bound their in-memory state.
    pub fn flush_blob_buffers(&mut self) -> BichonResult<()> {
        self.commit_tantivy()?;

        if !self.email_buf.is_empty() {
            let mut buf = std::mem::take(&mut self.email_buf);
            buf.sort_by(|a, b| a.0.cmp(&b.0));
            buf.dedup_by(|a, b| a.0 == b.0);

            let count = buf.len();
            let mut skipped = 0usize;
            let mut batch: Vec<([u8; 32], Vec<u8>, Codec)> = Vec::with_capacity(buf.len());
            for (key, data) in buf {
                if data.len() > 100 * 1024 * 1024 {
                    eprintln!(
                        "{}",
                        console::style(format!(
                            "WARN: skipping oversized email blob key={} ({} bytes)",
                            hex::encode(key),
                            data.len()
                        ))
                        .yellow()
                    );
                    skipped += 1;
                    continue;
                }
                batch.push((key, data, Codec::Zstd));
            }
            if !batch.is_empty() {
                self.engine.put_batch(&batch).map_err(|e| {
                    raise_error!(
                        format!("blob engine put_batch error: {e:#?}"),
                        ErrorCode::InternalError
                    )
                })?;
            }
            println!("flushed {} email blobs to engine", count - skipped);
            if skipped > 0 {
                eprintln!(
                    "{}",
                    console::style(format!("skipped {} oversized email blobs", skipped)).yellow()
                );
            }
        }

        if !self.attachment_buf.is_empty() {
            let mut buf = std::mem::take(&mut self.attachment_buf);
            buf.sort_by(|a, b| a.0.cmp(&b.0));
            buf.dedup_by(|a, b| a.0 == b.0);

            let count = buf.len();
            let mut skipped = 0usize;
            let mut batch: Vec<([u8; 32], Vec<u8>, Codec)> = Vec::with_capacity(buf.len());
            for (key, data) in buf {
                if data.len() > 100 * 1024 * 1024 {
                    eprintln!(
                        "{}",
                        console::style(format!(
                            "WARN: skipping oversized attachment blob key={} ({} bytes)",
                            hex::encode(key),
                            data.len()
                        ))
                        .yellow()
                    );
                    skipped += 1;
                    continue;
                }
                batch.push((key, data, Codec::Zstd));
            }
            if !batch.is_empty() {
                self.engine.put_batch(&batch).map_err(|e| {
                    raise_error!(
                        format!("blob engine put_batch error: {e:#?}"),
                        ErrorCode::InternalError
                    )
                })?;
            }
            println!("flushed {} attachment blobs to engine", count - skipped);
            if skipped > 0 {
                eprintln!(
                    "{}",
                    console::style(format!("skipped {} oversized attachment blobs", skipped))
                        .yellow()
                );
            }
        }

        Ok(())
    }

    /// Flush and shutdown the blob engine (called once at the very end).
    pub fn shutdown_engine(&mut self) -> BichonResult<()> {
        self.engine.flush().map_err(|e| {
            raise_error!(
                format!("engine flush error: {e:#?}"),
                ErrorCode::InternalError
            )
        })?;
        self.engine.shutdown().map_err(|e| {
            raise_error!(
                format!("engine shutdown error: {e:#?}"),
                ErrorCode::InternalError
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bichon_core::account::migration::{Account, AccountType};
    use bichon_core::database::MemDbModel;

    fn temp_root(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("bichon-migrate-v2-test")
            .join(prefix)
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new_dirs(root: &std::path::Path) -> NewDirs {
        NewDirs {
            envelope_dir: root.join("bichon-indices").join("mail_metadata"),
            attachment_dir: root.join("bichon-indices").join("attachment_metadata"),
            storage_dir: root.join("bichon-storage"),
        }
    }

    /// The stable UIDs the migrated documents carry, read from the new index.
    fn stored_imap_uids(dir: &std::path::Path, account_id: u64) -> Vec<u64> {
        use tantivy::collector::TopDocs;
        use tantivy::query::AllQuery;
        use tantivy::schema::Value;

        let index = Index::open_in_dir(dir).expect("open migrated envelope index");
        let reader = index.reader().expect("reader");
        reader.reload().expect("reload");
        let searcher = reader.searcher();
        let hits = searcher
            .search(&AllQuery, &TopDocs::with_limit(64).order_by_score())
            .expect("search");
        let fields = SchemaTools::email_fields();
        let mut uids: Vec<u64> = hits
            .iter()
            .filter_map(|(_, addr)| {
                let doc: TantivyDocument = searcher.doc(*addr).ok()?;
                if doc
                    .get_first(fields.f_account_id)
                    .and_then(|v| v.as_u64())
                    != Some(account_id)
                {
                    return None;
                }
                doc.get_first(fields.f_imap_uid)
                    .and_then(|v| v.as_u64())
            })
            .collect();
        uids.sort_unstable();
        uids
    }

    fn eml(message_id: &str) -> Vec<u8> {
        format!(
            "From: alice@example.com\r\n\
             To: bob@example.com\r\n\
             Subject: migrated\r\n\
             Date: Thu, 01 Jan 2023 00:00:00 +0000\r\n\
             Message-ID: <{message_id}>\r\n\
             \r\n\
             body of the legacy message\r\n"
        )
        .into_bytes()
    }

    /// The v0.3.7 migration writes documents by hand, so it is the one ingest
    /// path that does not go through the extractor. It must still give every
    /// document a distinct stable UID and leave the account counter above them —
    /// otherwise the first post-migration ingest would reissue a number.
    #[test]
    fn ingest_assigns_distinct_stable_uids_and_persists_the_counter() {
        let root = temp_root("uids");
        let account_id = 1u64;
        let mut writer = NewIndexWriterV2::open(new_dirs(&root)).expect("open writer");

        writer
            .ingest(&eml("migrate-1@x"), account_id, 10, 1, 0)
            .expect("ingest 1");
        writer
            .ingest(&eml("migrate-2@x"), account_id, 10, 2, 0)
            .expect("ingest 2");
        writer.finish_writers().expect("finish writers");

        assert_eq!(
            stored_imap_uids(&writer.envelope_dir, account_id),
            vec![1, 2],
            "every migrated document needs its own stable UID"
        );

        // The counter has nowhere to live without the account row the metadata
        // step creates, so seed it before finishing.
        let db_path = root.join("memdb");
        let seed = bichon_core::admin::meta::open_database(&db_path).unwrap();
        seed.collection(Account::collection())
            .upsert(
                &account_id.to_string(),
                &Account {
                    id: account_id,
                    account_type: AccountType::IMAP,
                    ..Default::default()
                },
            )
            .unwrap();

        writer.finish_uids(&root).expect("persist counters");

        let db = bichon_core::admin::meta::open_database(&db_path).unwrap();
        let account: Account = db
            .collection(Account::collection())
            .get(&account_id.to_string())
            .unwrap()
            .expect("account row");
        assert_eq!(
            account.imap_uid_next, 3,
            "the counter must sit above every UID the migration handed out"
        );

        writer.shutdown_engine().ok();
        let _ = std::fs::remove_dir_all(&root);
    }
}
