use std::{path::PathBuf, time::Instant};

use bytes::Bytes;
use mail_parser::MimeHeaders;

use bichon_core::{
    envelope::extractor::extract_references, message::content::AttachmentInfo,
    store::tantivy::tokenizers::EuroTokenizer, utils::compute_content_hash,
};

use bichon_blob::{Codec, Config, Engine};
use mail_parser::MessageParser;
use tantivy::{indexer::NoMergePolicy, Index, IndexWriter, TantivyDocument};
use uuid::Uuid;

use bichon_core::{
    common::AddrVec,
    envelope::extractor::{compute_thread_id, generate_message_id},
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
    let mut stripped_eml = original_body.to_vec();
    let mut infos = Vec::new();
    let mut blobs = Vec::new();

    let mut ranges: Vec<_> = message
        .attachments()
        .map(|att| {
            (
                att.raw_body_offset() as usize,
                att.raw_end_offset() as usize,
                att,
            )
        })
        .collect();
    ranges.sort_by(|a, b| b.0.cmp(&a.0));

    for (raw_start, raw_end, att) in ranges {
        let content_hash = compute_content_hash(att.contents());
        let body_len = original_body.len();
        let raw_start = raw_start.min(body_len);
        let raw_end = raw_end.min(body_len);
        let range_valid = raw_start < raw_end;

        let raw_content_hash =
            range_valid.then(|| bichon_core::utils::compute_raw_attachment_hash(&original_body[raw_start..raw_end]));
        if let Some(ref raw_hash) = raw_content_hash {
            blobs.push((
                raw_hash.clone(),
                Bytes::copy_from_slice(&original_body[raw_start..raw_end]),
            ));
        }

        if let Some(ref raw_hash) = raw_content_hash {
            let placeholder = format!("<<BICHON_DETACH_HASH:{}>>", raw_hash);
            stripped_eml.splice(raw_start..raw_end, placeholder.as_bytes().iter().cloned());
        }

        infos.push(AttachmentInfo {
            filename: att.attachment_name().map(|n| n.to_string()),
            size: att.contents().len(),
            inline: att
                .content_disposition()
                .map(|d| d.is_inline())
                .unwrap_or_else(|| att.content_id().is_some()),
            file_type: att
                .content_type()
                .map(|ct| {
                    format!(
                        "{}/{}",
                        ct.c_type.as_ref(),
                        ct.c_subtype.as_deref().unwrap_or("")
                    )
                })
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            content_id: att.content_id().map(|id| id.to_string()),
            content_hash,
            raw_content_hash,
            is_message: att.is_message(),
            extracted_text: None,
            extracted_page_count: None,
            extracted_is_ocr: false,
        });
    }

    (stripped_eml, DetachOutput { infos, blobs })
}

pub struct NewIndexWriterV2 {
    pub envelope_writer: Option<IndexWriter>,
    pub attachment_writer: Option<IndexWriter>,
    pub engine: Engine,
    pending: usize,
    email_buf: Vec<([u8; 32], Vec<u8>)>,
    attachment_buf: Vec<([u8; 32], Vec<u8>)>,
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
            pending: 0,
            email_buf: Vec::new(),
            attachment_buf: Vec::new(),
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
                    .map(|html| bichon_core::utils::html::extract_text(html.into_owned()))
            })
            .unwrap_or_default();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
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

        let from = message
            .from()
            .and_then(|addr| AddrVec::from(addr).0.into_iter().next())
            .and_then(|a| a.address)
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
            subject,
            preview,
            from,
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
