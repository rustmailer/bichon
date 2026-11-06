use std::{collections::HashMap, path::PathBuf};

use bichon_core::{
    error::{code::ErrorCode, BichonResult},
    migrate::{is_tantivy_index_dir, write_storage_version},
    raise_error,
};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use indicatif::{ProgressBar, ProgressStyle};
use tantivy::{
    collector::TopDocs,
    columnar::Column,
    query::TermQuery,
    schema::{IndexRecordOption, Value},
    DocAddress, Index, TantivyDocument, Term,
};

use crate::legacy::schema::SchemaTools;
use crate::migrate_store_v2::{NewDirs, NewIndexWriterV2};

pub struct LegacyDirs {
    pub envelope_dir: PathBuf,
    pub eml_dir: PathBuf,
}

impl LegacyDirs {
    pub fn new(index: PathBuf, data: PathBuf) -> Self {
        Self {
            envelope_dir: index,
            eml_dir: data,
        }
    }
}

pub fn is_legacy_data_layout_with_paths(
    envelope_dir: &PathBuf,
    eml_dir: &PathBuf,
) -> std::io::Result<bool> {
    let envelope_result = is_tantivy_index_dir(envelope_dir)?;
    let eml_result = is_tantivy_index_dir(eml_dir)?;
    Ok(envelope_result || eml_result)
}

/// Return the number of segments in the legacy EML Tantivy index.
pub fn count_eml_segments(legacy: &LegacyDirs) -> BichonResult<usize> {
    let eml_index = Index::open_in_dir(&legacy.eml_dir)
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    let reader = eml_index
        .reader()
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    let searcher = reader.searcher();
    Ok(searcher.segment_readers().len())
}

pub fn handle_migration_v037(theme: &ColorfulTheme) {
    println!(
        "\n{}",
        style("MIGRATION: Bichon v0.3.7 Storage → v2.x (bichon-blob)")
            .bold()
            .yellow()
    );

    println!(
        "{}",
        style(
            "This tool migrates data from the legacy v0.3.7 Tantivy-based storage \
            architecture directly to the v2.x bichon-blob storage format."
        )
        .dim()
    );

    println!(
        "{}",
        style(
            "Legacy v0.3.7 architecture:\n\
            • envelope metadata stored in Tantivy\n\
            • message data stored in Tantivy\n\n\
                New v2.x architecture:\n\
            • mail indexes stored in Tantivy\n\
            • attachment indexes stored in Tantivy\n\
            • raw message data stored in bichon-blob engine\n\
            • attachment blobs stored in bichon-blob engine"
        )
        .dim()
    );

    println!(
        "\n{} {}",
        style("IMPORTANT:").yellow().bold(),
        style(
            "The paths below must exactly match what your old bichon server was configured with."
        )
        .yellow()
    );

    // --- bichon-root-dir ---
    let root_dir_str: String = Input::with_theme(theme)
        .with_prompt("Enter --bichon-root-dir (same value used by the old server)")
        .validate_with(|input: &String| -> Result<(), &str> {
            let path = PathBuf::from(input);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }
            if !path.exists() {
                return Err("Directory does not exist.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();

    let root_path = PathBuf::from(&root_dir_str);

    // --- bichon-index-dir ---
    let default_index = root_path.join("envelope");
    let default_new_index = root_path.join("bichon-indices");
    let index_dir_str: String = Input::with_theme(theme)
        .with_prompt(format!(
            "Enter --bichon-index-dir (leave blank to use default: {})",
            style(default_index.display()).cyan()
        ))
        .allow_empty(true)
        .validate_with(|input: &String| -> Result<(), &str> {
            if input.is_empty() {
                return Ok(());
            }
            let path = PathBuf::from(input);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }

            if !path.exists() {
                return Err("Directory does not exist.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();

    let index_path = if index_dir_str.is_empty() {
        default_index
    } else {
        PathBuf::from(&index_dir_str)
    };

    let new_index_path = if index_dir_str.is_empty() {
        default_new_index
    } else {
        PathBuf::from(&index_dir_str).join("bichon-indices")
    };

    // --- bichon-data-dir ---
    let default_data = root_path.join("eml");
    let default_new_data = root_path.join("bichon-storage");
    let data_dir_str: String = Input::with_theme(theme)
        .with_prompt(format!(
            "Enter --bichon-data-dir (leave blank to use default: {})",
            style(default_data.display()).cyan()
        ))
        .allow_empty(true)
        .validate_with(|input: &String| -> Result<(), &str> {
            if input.is_empty() {
                return Ok(());
            }
            let path = PathBuf::from(input);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }
            if !path.exists() {
                return Err("Directory does not exist.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();

    let data_path = if data_dir_str.is_empty() {
        default_data
    } else {
        PathBuf::from(&data_dir_str)
    };

    let new_data_path = if data_dir_str.is_empty() {
        default_new_data
    } else {
        PathBuf::from(&data_dir_str).join("bichon-storage")
    };

    println!("\n{}", style("Paths to be migrated:").bold());
    println!("----------------------------------------");
    println!(
        "{:<20} : {}",
        "bichon-root-dir",
        style(root_path.display()).cyan()
    );
    println!(
        "{:<20} : {}",
        "bichon-index-dir",
        style(index_path.display()).cyan()
    );
    println!(
        "{:<20} : {}",
        "bichon-data-dir",
        style(data_path.display()).cyan()
    );
    println!("----------------------------------------");

    println!(
        "\n{} Checking legacy v0.3.7 storage layout...",
        style("⌛").yellow()
    );

    match is_legacy_data_layout_with_paths(&index_path, &data_path) {
        Ok(true) => {
            println!(
                "{} {}",
                style("✔").green(),
                style("Legacy v0.3.7 Tantivy-based storage detected. Migration to v2.x is required.")
                    .yellow()
            );
        }
        Ok(false) => {
            println!(
                "{} {}",
                style("✔").green(),
                style("No legacy v0.3.7 storage layout was detected at the specified paths.").green()
            );

            println!(
                "{}",
                style(
                    "The selected directories may already be using a newer storage architecture."
                )
                .dim()
            );

            return;
        }
        Err(e) => {
            eprintln!(
                "{} Failed to verify legacy storage layout: {:?}",
                style("ERROR:").red().bold(),
                e
            );

            std::process::exit(1);
        }
    }

    println!(
        "\n{} {}",
        style("⚠").yellow(),
        style(
            "This migration is non-destructive. Existing v0.x storage files will remain unchanged."
        )
        .yellow()
    );

    if !Confirm::with_theme(theme)
        .with_prompt("Ready to migrate?")
        .default(true)
        .interact()
        .unwrap()
    {
        println!("{}", style("Migration cancelled.").dim());
        return;
    }

    // Step 1: Migrate metadata (meta.db + mailbox.db → memdb)
    match crate::meta::migrate_metadata(&root_path) {
        Ok(()) => {}
        Err(e) => {
            eprintln!(
                "\n{} Metadata migration failed:\n{}",
                style("✘").red().bold(),
                style(e).red()
            );
            eprintln!(
                "{}",
                style("Aborting migration. No changes have been made to Tantivy data.").yellow()
            );
            return;
        }
    }

    println!(
        "\n{} {}",
        style("⌛").yellow(),
        style("Step 2: Migrating email index and blob data...").cyan()
    );

    println!(
        "\n{} {}",
        style("ℹ").blue(),
        style("Batch size controls memory usage during migration:").dim()
    );
    println!(
        "  {} 1000  — ~500MB RAM  (slower, low memory)",
        style("•").dim()
    );
    println!("  {} 3000  — ~1GB RAM    (recommended)", style("•").dim());
    println!(
        "  {} 5000  — ~2GB RAM    (faster, high memory)",
        style("•").dim()
    );
    println!(
        "  {} Note: actual memory usage depends on your average email size.",
        style("•").yellow()
    );
    println!(
        "  {}       If your mailbox contains many large attachments, use a smaller batch size.\n",
        style(" ").dim()
    );

    let batch_size: u32 = {
        let input: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Enter batch size (affects memory usage, see notes above)")
            .default("3000".to_string())
            .validate_with(|s: &String| match s.trim().parse::<usize>() {
                Ok(n) if n > 0 => Ok(()),
                _ => Err("Please enter a valid positive number"),
            })
            .interact_text()
            .unwrap_or("3000".to_string());
        input.trim().parse::<u32>().unwrap_or(3000)
    };

    println!(
        "{} Using batch size: {}\n",
        style("✓").green(),
        style(batch_size).cyan().bold()
    );

    let legacy = LegacyDirs::new(index_path.clone(), data_path.clone());
    let total_segments = match count_eml_segments(&legacy) {
        Ok(n) => n,
        Err(e) => {
            eprintln!(
                "\n{} Failed to count EML segments:\n{:?}",
                style("✘").red().bold(),
                e
            );
            return;
        }
    };

    if total_segments == 0 {
        println!(
            "{} {}",
            style("✔").green(),
            style("No EML segments found. Nothing to migrate.").bold()
        );
        return;
    }

    println!(
        "{} EML segments to migrate: {}",
        style("⌛").yellow(),
        style(total_segments).cyan()
    );

    let pb = ProgressBar::new(total_segments as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
            )
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut writer = match NewIndexWriterV2::open(NewDirs::new(
        new_index_path.clone(),
        new_data_path.clone(),
    )) {
        Ok(w) => w,
        Err(e) => {
            pb.finish_with_message(format!("{}", style("Migration failed.").red()));
            eprintln!("\n{} {:?}", style("✘").red().bold(), e);
            return;
        }
    };

    let mut grand_total_migrated: usize = 0;
    let mut grand_total_skipped: usize = 0;

    for seg_idx in 0..total_segments {
        let seg_total: std::cell::Cell<usize> = std::cell::Cell::new(0);

        pb.set_message(format!("Segment {}/{}", seg_idx + 1, total_segments));
        let legacy = LegacyDirs::new(index_path.clone(), data_path.clone());
        match do_migrate_segment_v2(
            batch_size,
            legacy,
            &mut writer,
            seg_idx,
            |msg| {
                if let Some(data) = msg.strip_prefix("TOTAL:") {
                    seg_total.set(data.parse().unwrap_or(0));
                } else if let Some(data) = msg.strip_prefix("PHASE1:") {
                    let parts: Vec<&str> = data.split('/').collect();
                    let scanned: usize = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let total: usize = parts
                        .get(1)
                        .and_then(|s| s.split_once(" skipped:").map(|(n, _)| n))
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let skipped: usize = data
                        .split_once("skipped:")
                        .and_then(|(_, s)| s.parse().ok())
                        .unwrap_or(0);
                    let pct = if total > 0 {
                        (scanned * 100) / total
                    } else {
                        0
                    };
                    pb.set_message(format!(
                        "Segment {}/{} [scanning {}/{} skipped:{} {}%]",
                        seg_idx + 1,
                        total_segments,
                        scanned,
                        total,
                        skipped,
                        pct,
                    ));
                } else if let Some(data) = msg.strip_prefix("PROGRESS:") {
                    let parts: Vec<&str> = data.split(':').collect();
                    let migrated: usize = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let total = seg_total.get();
                    let pct = if total > 0 {
                        (migrated * 100) / total
                    } else {
                        0
                    };
                    pb.set_message(format!(
                        "Segment {}/{} [migrating {}/{} {}%]",
                        seg_idx + 1,
                        total_segments,
                        migrated,
                        total,
                        pct,
                    ));
                } else if let Some(warn) = msg.strip_prefix("WARN:") {
                    pb.println(format!("{} {}", style("⚠").yellow(), warn));
                } else if let Some(done_data) = msg.strip_prefix("DONE:") {
                    let parts: Vec<&str> = done_data.split(':').collect();
                    let migrated: usize = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let skipped: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    grand_total_migrated += migrated;
                    grand_total_skipped += skipped;
                }
            },
        ) {
            Ok(()) => {}
            Err(e) => {
                pb.finish_with_message(format!("{}", style("Migration failed.").red()));
                eprintln!("\n{} {:?}", style("✘").red().bold(), e);
                return;
            }
        }

        pb.set_position((seg_idx + 1) as u64);
    }

    pb.set_message(style("Finalizing indexes...").dim().to_string());
    if let Err(e) = writer.finish_writers() {
        pb.finish_with_message(format!("{}", style("Migration failed.").red()));
        eprintln!("\n{} {:?}", style("✘").red().bold(), e);
        return;
    }

    pb.set_message(style("Persisting stable IMAP UID counters...").dim().to_string());
    if let Err(e) = writer.finish_uids(&root_path) {
        pb.finish_with_message(format!("{}", style("Migration failed.").red()));
        eprintln!("\n{} {:?}", style("✘").red().bold(), e);
        return;
    }

    pb.set_message(style("Shutting down blob engine...").dim().to_string());
    if let Err(e) = writer.shutdown_engine() {
        pb.finish_with_message(format!("{}", style("Migration failed.").red()));
        eprintln!("\n{} {:?}", style("✘").red().bold(), e);
        return;
    }

    // Write STORAGE_VERSION = 2 to mark the data as v2.x compatible
    if let Err(e) = write_storage_version(&root_path, 2) {
        pb.finish_with_message(format!("{}", style("Migration failed.").red()));
        eprintln!(
            "\n{} Failed to write STORAGE_VERSION: {:?}",
            style("✘").red().bold(),
            e
        );
        return;
    }

    pb.finish_with_message(format!(
        "Migration finished. Total: {}, Skipped: {}",
        grand_total_migrated, grand_total_skipped
    ));

    println!(
        "{} {}",
        style("✔").green(),
        style("Migration to v2.x completed successfully!").bold()
    );
}

/// Migrate all documents from a single EML segment to the v2.x storage layout.
fn do_migrate_segment_v2<F>(
    batch_size: u32,
    legacy: LegacyDirs,
    writer: &mut NewIndexWriterV2,
    segment_index: usize,
    mut on_progress: F,
) -> BichonResult<()>
where
    F: FnMut(&str),
{
    // ── open legacy indices ────────────────────────────────────────────
    let envelope_index = Index::open_in_dir(&legacy.envelope_dir)
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    let eml_index = Index::open_in_dir(&legacy.eml_dir)
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

    let envelope_reader = envelope_index
        .reader()
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    let eml_reader = eml_index
        .reader()
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

    let envelope_searcher = envelope_reader.searcher();
    let eml_searcher = eml_reader.searcher();

    let ef = SchemaTools::envelope_fields();
    let mf = SchemaTools::eml_fields();

    let eml_segments = eml_searcher.segment_readers();
    let eml_segment = eml_segments.get(segment_index).ok_or_else(|| {
        raise_error!(
            format!(
                "segment index {} out of range ({} segments)",
                segment_index,
                eml_segments.len()
            ),
            ErrorCode::InternalError
        )
    })?;

    let num_docs = eml_segment.num_docs();
    if num_docs == 0 {
        on_progress("TOTAL:0");
        on_progress("DONE:0:0");
        return Ok(());
    }

    on_progress(&format!("TOTAL:{}", num_docs));

    let max_doc = eml_segment.max_doc();
    let ff = eml_segment.fast_fields();
    let f_id_col: Column<u64> = ff.u64("id").map_err(|e| {
        raise_error!(
            format!("failed to open f_id fast field: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;

    // ── Phase 1: build eid → (uid, internal_date) from envelope, then drop it ──
    let mut envelope_map: HashMap<u64, (u32, i64)> = HashMap::with_capacity(num_docs as usize);

    let mut env_scanned = 0u32;
    let mut env_skipped = 0u32;
    for doc_id in 0..max_doc {
        if eml_segment.is_deleted(doc_id) {
            continue;
        }
        let eid = f_id_col.values.get_val(doc_id);

        let term = Term::from_field_u64(ef.f_id, eid);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let hits: Vec<(_, DocAddress)> = envelope_searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        if let Some((_, addr)) = hits.first() {
            let env_doc: TantivyDocument = envelope_searcher
                .doc(*addr)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
            let uid = env_doc
                .get_first(ef.f_uid)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let internal_date = env_doc
                .get_first(ef.f_internal_date)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            envelope_map.insert(eid, (uid, internal_date));
            env_scanned += 1;
        } else {
            env_skipped += 1;
        }

        if env_scanned % 10 == 0 {
            on_progress(&format!(
                "PHASE1:{}/{} skipped:{}",
                env_scanned, max_doc, env_skipped
            ));
        }
    }

    // Free the envelope index before the heavy EML processing.
    drop(envelope_searcher);
    drop(envelope_reader);
    drop(envelope_index);

    // ── Phase 2: process EML docs, streaming one at a time ─────────────
    let mut total_migrated = 0usize;
    let mut total_skipped = 0usize;

    let mut chunk_start = 0u32;

    while chunk_start < max_doc {
        let chunk_end = (chunk_start + batch_size).min(max_doc);
        let store_reader = eml_segment
            .get_store_reader(2)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

        for doc_id in chunk_start..chunk_end {
            if eml_segment.is_deleted(doc_id) {
                continue;
            }

            let eid = f_id_col.values.get_val(doc_id);

            let (uid, internal_date) = match envelope_map.get(&eid) {
                Some(v) => *v,
                None => {
                    on_progress(&format!("WARN: eid {} envelope not found", eid));
                    total_skipped += 1;
                    continue;
                }
            };

            let eml_doc: TantivyDocument = store_reader
                .get(doc_id)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;

            let account_id = match eml_doc.get_first(mf.f_account_id).and_then(|v| v.as_u64()) {
                Some(v) => v,
                None => {
                    on_progress(&format!("WARN: eid {} account_id missing", eid));
                    total_skipped += 1;
                    continue;
                }
            };
            let mailbox_id = eml_doc
                .get_first(mf.f_mailbox_id)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let eml_bytes = match eml_doc.get_first(mf.f_eml).and_then(|v| v.as_bytes()) {
                Some(b) => b,
                None => {
                    on_progress(&format!("WARN: eid {} eml bytes missing", eid));
                    total_skipped += 1;
                    continue;
                }
            };

            if let Err(e) = writer.ingest(eml_bytes, account_id, mailbox_id, uid, internal_date) {
                on_progress(&format!(
                    "ERROR: Account {} eid {} ingest failed: {}",
                    account_id, eid, e
                ));
                total_skipped += 1;
                continue;
            }

            total_migrated += 1;

            if total_migrated % 10 == 0 || total_migrated as u32 == num_docs {
                on_progress(&format!("PROGRESS:{}:{}", total_migrated, num_docs));
            }
        }

        drop(store_reader);

        // Flush blob buffers to bichon-blob engine.
        writer.flush_blob_buffers()?;

        chunk_start = chunk_end;
    }

    on_progress(&format!("DONE:{}:{}", total_migrated, total_skipped));
    Ok(())
}

#[cfg(test)]
mod migration_e2e_tests {
    use super::*;
    use bichon_core::database::MemDbModel;
    use native_db::*;
    use tantivy::schema::{
        FacetOptions, Schema, TantivyDocument, FAST, INDEXED, STORED, STRING, TEXT,
    };

    /// Build the legacy envelope index with the *real* v0.3.7 field order.
    /// This deliberately does not reuse `crate::legacy::schema`, so a drift in
    /// that schema (the field-id mismatch that used to lose `internal_date`)
    /// makes this test fail instead of hiding behind a self-consistent fixture.
    fn legacy_envelope_index(dir: &std::path::Path) {
        let mut b = Schema::builder();
        let f_id = b.add_u64_field("id", INDEXED | STORED | FAST);
        let f_account_id = b.add_u64_field("account_id", INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field("mailbox_id", INDEXED | STORED | FAST);
        let f_uid = b.add_u64_field("uid", INDEXED | STORED | FAST);
        let f_subject = b.add_text_field("subject", TEXT | STORED);
        let f_text = b.add_text_field("text", TEXT | STORED);
        let f_from = b.add_text_field("from", STRING | STORED | FAST);
        let f_to = b.add_text_field("to", STRING | STORED);
        let f_cc = b.add_text_field("cc", STRING | STORED);
        let f_bcc = b.add_text_field("bcc", STRING | STORED);
        let f_date = b.add_i64_field("date", STORED | FAST);
        let f_internal_date = b.add_i64_field("internal_date", STORED | FAST);
        let f_size = b.add_u64_field("size", STORED | FAST);
        let f_thread_id = b.add_u64_field("thread_id", INDEXED | STORED | FAST);
        let f_message_id = b.add_text_field("message_id", STRING | STORED);
        let f_attachments = b.add_text_field("attachments", TEXT | STORED);
        let f_has_attachment = b.add_bool_field("has_attachment", INDEXED | STORED | FAST);
        let _f_tags = b.add_facet_field("tags", FacetOptions::default().set_stored());
        let schema = b.build();

        let index = Index::create_in_dir(dir, schema).unwrap();
        let mut writer = index.writer(50_000_000).unwrap();
        let mut doc = TantivyDocument::new();
        doc.add_u64(f_id, 1);
        doc.add_u64(f_account_id, 1);
        doc.add_u64(f_mailbox_id, 10);
        doc.add_u64(f_uid, 42);
        doc.add_text(f_subject, "legacy subject");
        doc.add_text(f_text, "legacy body");
        doc.add_text(f_from, "alice@example.com");
        doc.add_text(f_to, "bob@example.com");
        doc.add_text(f_cc, "");
        doc.add_text(f_bcc, "");
        doc.add_i64(f_date, 1_000_000_000_000);
        doc.add_i64(f_internal_date, 1_234_567_890_000);
        doc.add_u64(f_size, 123);
        doc.add_u64(f_thread_id, 7);
        doc.add_text(f_message_id, "<legacy@x>");
        doc.add_text(f_attachments, "");
        doc.add_bool(f_has_attachment, false);
        writer.add_document(doc).unwrap();
        writer.commit().unwrap();
    }

    fn legacy_eml_index(dir: &std::path::Path, eml: &[u8]) {
        let mut b = Schema::builder();
        let f_id = b.add_u64_field("id", INDEXED | FAST);
        let f_account_id = b.add_u64_field("account_id", INDEXED | STORED | FAST);
        let f_mailbox_id = b.add_u64_field("mailbox_id", INDEXED | STORED | FAST);
        let f_eml = b.add_bytes_field("eml", STORED);
        let schema = b.build();

        let index = Index::create_in_dir(dir, schema).unwrap();
        let mut writer = index.writer(50_000_000).unwrap();
        let mut doc = TantivyDocument::new();
        doc.add_u64(f_id, 1);
        doc.add_u64(f_account_id, 1);
        doc.add_u64(f_mailbox_id, 10);
        doc.add_bytes(f_eml, eml);
        writer.add_document(doc).unwrap();
        writer.commit().unwrap();
    }

    /// Full v0.3.7 -> v2.x migration over a synthetic v0.3.7 store.
    ///
    /// Deliberately a plain `#[test]`: `bichon-admin`'s interactive menu runs
    /// the migration without a Tokio runtime. This is the regression test for
    /// the `DB_MANAGER`/`tokio::spawn` panic introduced in 2.3.0, and it also
    /// pins the v0.3.7 field order so `internal_date` survives the migration.
    #[test]
    fn v037_migration_runs_without_tokio_and_preserves_internal_date() {
        let root = std::env::temp_dir()
            .join("bichon-v037-migration-e2e")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("envelope")).unwrap();
        std::fs::create_dir_all(root.join("eml")).unwrap();

        let eml = b"From: Alice <alice@example.com>\r\n\
To: bob@example.com\r\n\
Subject: legacy subject\r\n\
Date: Thu, 01 Jan 2026 00:00:00 +0000\r\n\
Message-ID: <legacy@x>\r\n\
\r\n\
legacy body\r\n";

        legacy_envelope_index(&root.join("envelope"));
        legacy_eml_index(&root.join("eml"), eml);

        {
            let db = Builder::new()
                .create(&crate::meta::META_MODELS, root.join("meta.db"))
                .unwrap();
            let rw = db.rw_transaction().unwrap();
            let account = crate::meta::AccountV3 {
                id: 1,
                email: "alice@example.com".into(),
                account_type: bichon_core::account::migration::AccountType::IMAP,
                ..Default::default()
            };
            rw.insert(account).unwrap();
            rw.commit().unwrap();
        }
        {
            let db = Builder::new()
                .create(&crate::meta::MAILBOX_MODELS, root.join("mailbox.db"))
                .unwrap();
            let rw = db.rw_transaction().unwrap();
            let mailbox = crate::meta::MailBox {
                id: 10,
                account_id: 1,
                name: "INBOX".into(),
                ..Default::default()
            };
            rw.insert(mailbox).unwrap();
            rw.commit().unwrap();
        }

        crate::meta::migrate_metadata(&root).unwrap();

        let legacy = LegacyDirs::new(root.join("envelope"), root.join("eml"));
        assert_eq!(count_eml_segments(&legacy).unwrap(), 1);

        let mut writer = NewIndexWriterV2::open(NewDirs::new(
            root.join("bichon-indices"),
            root.join("bichon-storage"),
        ))
        .unwrap();
        do_migrate_segment_v2(1000, legacy, &mut writer, 0, |message| {
            assert!(
                !message.starts_with("ERROR") && !message.starts_with("WARN"),
                "migration reported: {message}"
            );
        })
        .unwrap();
        writer.finish_writers().unwrap();
        writer.finish_uids(&root).unwrap();
        writer.shutdown_engine().unwrap();

        let index =
            Index::open_in_dir(root.join("bichon-indices").join("mail_metadata")).unwrap();
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();
        let hits = searcher
            .search(
                &tantivy::query::AllQuery,
                &tantivy::collector::TopDocs::with_limit(10).order_by_score(),
            )
            .unwrap();
        assert_eq!(hits.len(), 1, "one migrated envelope expected");

        let doc: TantivyDocument = searcher.doc(hits[0].1).unwrap();
        let fields = bichon_core::store::tantivy::schema::SchemaTools::email_fields();
        let get_str = |field| {
            doc.get_first(field)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let get_u64 = |field| doc.get_first(field).and_then(|v| v.as_u64());
        let get_i64 = |field| doc.get_first(field).and_then(|v| v.as_i64());

        assert_eq!(get_str(fields.f_subject).as_deref(), Some("legacy subject"));
        assert_eq!(get_str(fields.f_from).as_deref(), Some("alice@example.com"));
        assert_eq!(
            get_str(fields.f_from_name).as_deref(),
            Some("Alice"),
            "the display name must survive the migration"
        );
        assert_eq!(
            get_str(fields.f_wire_from),
            None,
            "v0.3.7 accounts have no sender-override rule, so wire_from stays empty"
        );
        assert_eq!(get_str(fields.f_to).as_deref(), Some("bob@example.com"));
        assert_eq!(get_u64(fields.f_account_id), Some(1));
        assert_eq!(get_u64(fields.f_mailbox_id), Some(10));
        assert_eq!(
            get_i64(fields.f_internal_date),
            Some(1_234_567_890_000),
            "v0.3.7 internal_date must survive the migration"
        );
        assert_eq!(
            get_i64(fields.f_date),
            Some(1_767_225_600_000),
            "the Date header in the EML must be re-parsed during the migration"
        );
        assert_eq!(
            get_u64(fields.f_imap_uid),
            Some(1),
            "migrated envelopes need a stable archive-side UID"
        );

        let db = bichon_core::admin::meta::open_database(root.join("memdb")).unwrap();
        let account: bichon_core::account::migration::Account = db
            .collection(bichon_core::account::migration::Account::collection())
            .get("1")
            .unwrap()
            .expect("migrated account row");
        assert_eq!(
            account.imap_uid_next, 2,
            "the stable-UID counter must sit above the assigned UID"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
