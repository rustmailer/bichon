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

//! `bichon-admin reindex`: rebuild the search indexes from the existing indexes.
//!
//! The community edition has **no separate metadata database** — account /
//! mailbox / UID / tag associations live only in the tantivy documents' stored
//! fields, and blobs are keyed purely by content hash. The only viable rebuild
//! is therefore a **bootstrap from the old indexes**: read the stored fields from
//! the old index and re-extract bodies from the blobs, writing into a new index.
//! Never "delete the directory and rebuild from blobs" — the associations would
//! be lost.
//!
//! A single unreadable document never aborts the run: it is skipped and written
//! to an NDJSON report (see [`SkipReport`]) with as much identifying data as
//! could be read, so the operator can re-fetch it from the source mailbox or
//! another archive. Failures that are *not* per-document (a segment whose store
//! is unreadable, an I/O error) still abort, and the partial `.rebuild` dirs are
//! cleaned up; the live dirs are untouched until the final swap.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bichon_blob::{Codec, Config, Engine, Error as BlobError};
use bichon_core::{
    account::migration::AccountModel,
    archive::imap::stable_uid::AccountUidAssigner,
    database::MemDbModel,
    envelope::extractor::{resolve_sender, ResolvedSender},
    error::{code::ErrorCode, BichonResult},
    ext::text_extractor,
    message::content::AttachmentInfo,
    migrate::{is_tantivy_index_dir, read_storage_version},
    raise_error,
    settings::dir::{available_space, resolve_dirs, ResolvedDirs},
    store::{
        envelope::Envelope,
        tantivy::{
            format::{
                check_index_format, write_index_format_version, IndexFormatStatus,
                CURRENT_INDEX_FORMAT_VERSION,
            },
            model::{AttachmentModel, EnvelopeWithAttachments},
            schema::SchemaTools,
            tokenizers::{tri_analyzer, EuroTokenizer},
        },
    },
    utils::html::extract_text,
};

use clap::Parser;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use indicatif::{ProgressBar, ProgressStyle};
use mail_parser::MessageParser;
use tantivy::{
    collector::{Count, TopDocs},
    indexer::NoMergePolicy,
    query::{AllQuery, TermQuery},
    schema::{Facet, Field, IndexRecordOption, Schema, Value},
    Index, Searcher, TantivyDocument, Term,
};
use tracing::warn;

/// Disk pre-check factor: the rebuilt index coexists with the live one on the
/// same volume, so room for the rebuild must be available.
const DISK_SAFETY_FACTOR: f64 = 1.15;

/// Commit the rebuild writer every this many *source* documents. Committing in
/// batches bounds the writer's memory, but the progress bar is updated for
/// every document (see `progress_bar`), so a slow batch still looks alive.
const COMMIT_BATCH: u64 = 10_000;

/// The whole index root is rebuilt into a sibling `<name>.new` directory (same
/// volume, so the final swap is an atomic rename), and the previous root is
/// renamed to `<name>.old-<timestamp>`. The tool never deletes either of them —
/// the operator removes the backup once search has been verified.
const NEW_SUFFIX: &str = ".new";
const DEFAULT_REPORT_FILE: &str = "reindex-skipped.ndjson";

#[derive(Parser, Debug, Default)]
pub struct ReindexArgs {
    /// bichon data root (prompted interactively when omitted).
    #[clap(
        long,
        visible_aliases = ["root-dir", "bichon-root-dir"],
        env = "BICHON_ROOT_DIR",
        help = "Path of the bichon data root (same value as the server's --bichon-root-dir)"
    )]
    pub root: Option<String>,

    /// Index parent directory (default: `<root>/bichon-indices`).
    #[clap(
        long = "index-dir",
        visible_aliases = ["index-parent", "bichon-index-dir"],
        env = "BICHON_INDEX_DIR",
        help = "Parent directory of bichon-indices (default: <root>); same value as the server's --bichon-index-dir"
    )]
    pub index_parent: Option<String>,

    /// Blob data parent directory (default: `<root>/bichon-storage`).
    #[clap(
        long = "data-dir",
        visible_aliases = ["data-parent", "bichon-data-dir"],
        env = "BICHON_DATA_DIR",
        help = "Parent directory of bichon-storage (default: <root>); same value as the server's --bichon-data-dir"
    )]
    pub data_parent: Option<String>,

    /// Skip attachment text re-extraction (already a noop in the community edition).
    #[clap(long, help = "Skip attachment text re-extraction")]
    pub skip_attachment_text: bool,

    /// Reindex even if the index is already at the current format.
    #[clap(long, help = "Reindex even if the index is already at the current format")]
    pub force: bool,

    /// NDJSON file recording every document that could not be carried over
    /// (default: `<root>/reindex-skipped.ndjson`). It contains plain-text
    /// message metadata, so treat it like the archive itself.
    #[clap(long, help = "NDJSON file for skipped/degraded documents; contains plain-text message metadata (default: <root>/reindex-skipped.ndjson)")]
    pub report: Option<String>,
}

#[derive(Default)]
struct PhaseStats {
    docs: u64,
    skipped: u64,
    empty_bodies: u64,
    extracted_texts: u64,
    /// Id of the first document successfully written — used by the pre-swap
    /// validation as a real term to query back.
    sample_id: Option<String>,
}

#[derive(Default)]
struct ReindexReport {
    emails: u64,
    attachments: u64,
    skipped: u64,
    empty_bodies: u64,
    extracted_texts: u64,
    email_sample: Option<String>,
    attachment_sample: Option<String>,
    elapsed: Duration,
}

pub fn run_cli(args: ReindexArgs) -> i32 {
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("\n{} {e:?}", style("ERROR:").red().bold());
            1
        }
    }
}

fn run(args: ReindexArgs) -> BichonResult<i32> {
    let theme = ColorfulTheme::default();
    let root = resolve_root(&args, &theme)?;
    // A bare `bichon-admin reindex` must be enough: ask for the two optional
    // directories right here, exactly like the v1.x migration does. Anything
    // supplied by flag or env var is taken as-is and not asked again.
    let args = prompt_missing_dirs(args, &theme, &root);
    let dirs = resolve_dirs(
        &root,
        args.index_parent.as_deref().map(Path::new),
        args.data_parent.as_deref().map(Path::new),
    );
    execute(&root, &dirs, &args, true)
}

/// Run the reindex **without** its own confirmation prompt — for callers that
/// have already asked the user, such as the v1.x migration. The plan is still
/// printed; only the "Proceed?" question is skipped. Returns the process code.
pub fn run_without_confirm(args: ReindexArgs) -> i32 {
    let theme = ColorfulTheme::default();
    let result = resolve_root(&args, &theme).and_then(|root| {
        let dirs = resolve_dirs(
            &root,
            args.index_parent.as_deref().map(Path::new),
            args.data_parent.as_deref().map(Path::new),
        );
        execute(&root, &dirs, &args, false)
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("\n{} {e:?}", style("ERROR:").red().bold());
            1
        }
    }
}

/// Arguments for the interactive menu's "Rebuild Search Indexes" entry.
///
/// The menu never parses a command line, so clap's env fallbacks never run —
/// the root is resolved from `BICHON_ROOT_DIR` or asked here. The optional
/// index/data directories are then asked for by `run` (via
/// [`prompt_missing_dirs`]), just like the CLI subcommand.
pub fn interactive_args(theme: &ColorfulTheme) -> ReindexArgs {
    let root = resolve_root(
        &ReindexArgs {
            root: env_value("BICHON_ROOT_DIR"),
            ..Default::default()
        },
        theme,
    )
    .ok()
    .map(|path| path.display().to_string());

    ReindexArgs {
        root,
        ..Default::default()
    }
}

/// A non-empty environment variable, trimmed; `None` when unset or blank.
fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Ask for whichever optional directory was not supplied on the command line
/// or via its environment variable. The prompt shows the concrete default
/// under the resolved root, so pressing Enter twice is the normal path for a
/// default layout.
fn prompt_missing_dirs(
    mut args: ReindexArgs,
    theme: &ColorfulTheme,
    root: &Path,
) -> ReindexArgs {
    // Scripts and `docker exec` without `-it` have no terminal to answer on;
    // keep their current defaults instead of blocking or emitting a prompt.
    if !std::io::stdin().is_terminal() {
        return args;
    }
    if args.index_parent.is_none() {
        let prompt = format!(
            "Index parent directory (blank = {})",
            root.join("bichon-indices").display()
        );
        args.index_parent = prompt_optional_dir(theme, "BICHON_INDEX_DIR", &prompt);
    }
    if args.data_parent.is_none() {
        let prompt = format!(
            "Data parent directory (blank = {})",
            root.join("bichon-storage").display()
        );
        args.data_parent = prompt_optional_dir(theme, "BICHON_DATA_DIR", &prompt);
    }
    args
}

/// Ask for an optional directory, honouring the same env var the subcommand
/// uses. An empty answer means "use the default under the data root".
fn prompt_optional_dir(theme: &ColorfulTheme, env: &str, prompt: &str) -> Option<String> {
    if let Some(value) = env_value(env) {
        return Some(value);
    }
    let input: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .allow_empty(true)
        .validate_with(|input: &String| -> Result<(), &str> {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                return Ok(());
            }
            let path = PathBuf::from(trimmed);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }
            if !path.exists() {
                return Err("Directory does not exist.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap_or_default();
    let trimmed = input.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Non-interactive body (pre-checks → both phases → swap → report).
/// `confirm=false` lets tests drive it directly.
fn execute(
    root: &Path,
    dirs: &ResolvedDirs,
    args: &ReindexArgs,
    confirm: bool,
) -> BichonResult<i32> {
    let start = std::time::Instant::now();

    // ── Pre-check 1: are both index directories tantivy indexes? ──
    let email_ok = is_tantivy_index_dir(&dirs.envelope_dir).unwrap_or(false);
    let attach_ok = is_tantivy_index_dir(&dirs.attachment_dir).unwrap_or(false);
    if !email_ok && !attach_ok {
        println!(
            "{} No index found under {} — nothing to reindex.",
            style("·").dim(),
            dirs.index_root.display()
        );
        if args.index_parent.is_none() {
            println!(
                "{}",
                style("If this deployment uses a custom index directory (`bichon_index_dir` / BICHON_INDEX_DIR), pass the same value as --index-dir (or --bichon-index-dir).").dim()
            );
        }
        return Ok(0);
    }

    // ── Pre-check 2: storage version ──
    match read_storage_version(root) {
        Some(v) if v >= 2 => {}
        Some(_) => {
            eprintln!(
                "{} Storage is not on the v2 layout yet; run `bichon-admin` to migrate first.",
                style("ERROR:").red().bold()
            );
            return Ok(1);
        }
        None => {
            eprintln!(
                "{} {} does not look like a bichon data root (no STORAGE_VERSION). Check --root.",
                style("ERROR:").red().bold(),
                root.display()
            );
            return Ok(1);
        }
    }

    // ── Pre-check 3: already current? ──
    let need_email =
        email_ok && check_index_format(&dirs.envelope_dir) == IndexFormatStatus::NeedsUpgrade;
    let need_attach =
        attach_ok && check_index_format(&dirs.attachment_dir) == IndexFormatStatus::NeedsUpgrade;
    if !args.force && !need_email && !need_attach {
        println!(
            "{} Indexes are already at the current format, already current (use --force to rebuild anyway).",
            style("·").dim()
        );
        return Ok(0);
    }

    // ── Pre-check 4: the blob store must be the one that belongs to this
    // index. Creating an empty one on a mistyped --data-dir would "succeed"
    // with every message body missing; fail instead, before touching disk. ──
    if email_ok && !dirs.blobs_dir.exists() {
        let docs = Index::open_in_dir(&dirs.envelope_dir)
            .and_then(|index| index.reader())
            .map(|reader| reader.searcher().num_docs())
            .unwrap_or(0);
        if docs > 0 {
            eprintln!(
                "{} The email index in {} has {} document(s), but its blob store {} does not exist.\n  \
                 Check --data-dir (pass the same value as the server's --bichon-data-dir); rebuilding against an empty store would lose every message body.",
                style("ERROR:").red().bold(),
                dirs.envelope_dir.display(),
                docs,
                dirs.blobs_dir.display()
            );
            return Ok(1);
        }
    }

    // ── Pre-check 5: service-running detection = take the blob lock ──
    std::fs::create_dir_all(&dirs.blobs_dir).map_err(|e| {
        raise_error!(
            format!("cannot create blob directory {:?}: {e:#?}", dirs.blobs_dir),
            ErrorCode::InternalError
        )
    })?;
    let mut blob_config = Config::default();
    blob_config.default_codec = Codec::Zstd;
    blob_config.compress_threshold = 1024;
    blob_config.flush_interval_secs = 0;
    blob_config.gc_interval_secs = 0;
    let engine = match Engine::open(&dirs.blobs_dir, blob_config) {
        Ok(e) => e,
        Err(BlobError::AlreadyOpen { .. }) => {
            eprintln!(
                "{} The service appears to be running (the blob store is locked). Stop it and retry.",
                style("ERROR:").red().bold()
            );
            return Ok(1);
        }
        Err(e) => {
            return Err(raise_error!(
                format!("cannot open blob store {:?}: {e:#?}", dirs.blobs_dir),
                ErrorCode::InternalError
            ))
        }
    };
    // The engine is held for the whole run: it doubles as the mutex against the
    // service starting mid-run, and the Drop guard releases the lock on both the
    // success and failure paths.
    let mut engine_guard = EngineGuard(Some(engine));
    let blobs = engine_guard.0.as_ref().expect("engine present");

    // ── The whole index root is rebuilt into a sibling `<root>.new` directory on
    // the same volume, so the final swap is an atomic rename. ──
    let index_root = &dirs.index_root;
    let new_root = sibling(index_root, NEW_SUFFIX);
    let new_envelope = new_root.join(leaf(&dirs.envelope_dir)?);
    let new_attachment = new_root.join(leaf(&dirs.attachment_dir)?);

    // ── Pre-check 6: disk space (the rebuild coexists with the live index) ──
    let index_parent = index_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf());
    let old_bytes = dir_size(&dirs.envelope_dir) + dir_size(&dirs.attachment_dir);
    let need_bytes = (old_bytes as f64 * DISK_SAFETY_FACTOR) as u64;
    let free_bytes = available_space(&index_parent);
    if free_bytes < need_bytes {
        eprintln!(
            "{} Not enough disk space: need about {}, {} available.",
            style("ERROR:").red().bold(),
            human_bytes(need_bytes),
            human_bytes(free_bytes)
        );
        return Ok(1);
    }

    // ── Pre-check 7: a leftover `.new` from an interrupted run is a temp
    // artifact and is safe to drop. Existing `.old-*` backups are NEVER touched:
    // only the operator removes those. ──
    if new_root.exists() {
        println!(
            "{} Cleaning up leftover temp directory {}",
            style("·").dim(),
            new_root.display()
        );
        let _ = std::fs::remove_dir_all(&new_root);
    }

    // ── Pre-check 8: summary + confirmation ──
    let backup = backup_path(index_root);
    println!(
        "\n{}\n  {:<18} {}\n  {:<18} {}\n  {:<18} {}\n  {:<18} {}",
        style("REINDEX PLAN").bold().bright().cyan(),
        "index root:",
        index_root.display(),
        "rebuild into:",
        new_root.display(),
        "previous kept at:",
        backup.display(),
        "blob store:",
        dirs.blobs_dir.display(),
    );
    println!(
        "{}",
        style("Do not restart the service until the rebuild finishes — it holds the data lock.").yellow()
    );
    if confirm
        && !Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Proceed with reindex?")
            .default(true)
            .interact()
            .unwrap_or(false)
    {
        println!("{}", style("Cancelled.").dim());
        return Ok(0);
    }

    // ── Open the old indexes (read-only; kept until just before the swap) ──
    let mut old_email: Option<OldIndex> = None;
    if email_ok {
        old_email = Some(OldIndex::open(&dirs.envelope_dir)?);
    }
    let mut old_attach: Option<OldIndex> = None;
    if attach_ok {
        old_attach = Some(OldIndex::open(&dirs.attachment_dir)?);
    }

    let report_path = args
        .report
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(DEFAULT_REPORT_FILE));
    let mut skip = SkipReport::new(report_path);

    // A failure before the swap only discards the temp `.new` dir; the live index
    // root is untouched.
    let cleanup_new = || {
        let _ = std::fs::remove_dir_all(&new_root);
    };

    // ── Phases A + B, building into the temp `.new` root. ──
    // The rebuild re-evaluates each message's stored sender against the
    // account's current header rules, so those rules are read up front. The
    // database handle is dropped here: the wrap-up reopens it to persist UID
    // counters, and only one handle may exist at a time.
    let account_headers = load_account_headers(root)?;
    let mut uids = AccountUidAssigner::new();
    let mut report = match run_phases(
        old_email.as_ref(),
        old_attach.as_ref(),
        &new_envelope,
        &new_attachment,
        blobs,
        &account_headers,
        args.skip_attachment_text,
        &mut skip,
        &mut uids,
    ) {
        Ok(r) => r,
        Err(e) => {
            cleanup_new();
            return Err(e);
        }
    };

    // Accounts that mixed carried and freshly assigned UIDs: normal rebuilds are
    // all-or-nothing per account, so this means the old index was already
    // inconsistent. The rebuild still succeeds (fresh numbers are assigned above
    // every carried one), but it is worth surfacing.
    for account_id in uids.mixed_accounts() {
        skip.record(skip_record(
            "mixed_uid_state",
            "email",
            "-",
            0,
            Some(format!(
                "account {account_id} had both carried and freshly assigned IMAP UIDs"
            )),
            None,
        ));
    }

    // ── Wrap-up 1: stamp the format marker, sanity-check the freshly built
    // indexes, and persist the UID counters BEFORE anything is swapped in. A
    // failure here aborts with the live indexes untouched. Persisting the
    // counters before the swap is the safe order: the counter may end up ahead
    // of a live index (wasted numbers), never behind it (reissued numbers).
    let pre_swap = (|| -> BichonResult<()> {
        if email_ok {
            write_index_format_version(&new_envelope).map_err(|e| {
                raise_error!(format!("failed to write email INDEX_FORMAT: {e:#?}"), ErrorCode::InternalError)
            })?;
            validate_rebuild(
                &new_envelope,
                report.emails,
                report.email_sample.as_deref(),
                SchemaTools::email_fields().f_id,
                "email",
            )?;
            validate_stable_uids(&new_envelope)?;
        }
        if attach_ok {
            write_index_format_version(&new_attachment).map_err(|e| {
                raise_error!(
                    format!("failed to write attachment INDEX_FORMAT: {e:#?}"),
                    ErrorCode::InternalError
                )
            })?;
            validate_rebuild(
                &new_attachment,
                report.attachments,
                report.attachment_sample.as_deref(),
                SchemaTools::attachment_fields().f_id,
                "attachment",
            )?;
        }
        if email_ok {
            // Persist the counters the rebuild produced. Opening memdb from the
            // resolved root (not `DB_MANAGER`) matches the rest of this tool:
            // it operates on the root it was pointed at.
            let db = bichon_core::admin::meta::open_database(root.join("memdb"))?;
            uids.finish(&db)?;
        }
        Ok(())
    })();
    if let Err(e) = pre_swap {
        cleanup_new();
        return Err(e);
    }

    // ── Wrap-up 2: explicitly drop every old index handle (on Windows a rename
    // fails while handles are open) ──
    drop(old_attach);
    drop(old_email);

    // ── Wrap-up 3: swap the whole index root with two same-volume atomic
    // renames. The previous root is renamed aside — never deleted — so it can be
    // restored manually at any time. ──
    rename_with_retry(index_root, &backup).map_err(|e| {
        cleanup_new();
        raise_error!(
            format!("cannot rename {:?} to {:?}: {e:#?}", index_root, backup),
            ErrorCode::InternalError
        )
    })?;
    if let Err(e) = rename_with_retry(&new_root, index_root) {
        if rename_with_retry(&backup, index_root).is_err() {
            eprintln!(
                "{} Rollback failed. Your previous indexes are safe at {} — rename it back to {} manually.",
                style("ERROR:").red().bold(),
                backup.display(),
                index_root.display()
            );
        } else {
            cleanup_new();
        }
        return Err(raise_error!(
            format!("cannot rename {:?} to {:?}: {e:#?}", new_root, index_root),
            ErrorCode::InternalError
        ));
    }

    // ── Wrap-up 4: blob flush + shutdown (the Drop guard releases the lock) ──
    if let Some(e) = engine_guard.0.take() {
        let _ = e.flush();
        let _ = e.shutdown();
    }

    report.elapsed = start.elapsed();
    println!("\n{}", style("REINDEX COMPLETE").green().bold());
    println!("  {:<18} {}", "emails:", report.emails);
    println!("  {:<18} {}", "attachments:", report.attachments);
    println!("  {:<18} {}", "skipped docs:", report.skipped);
    println!("  {:<18} {}", "empty bodies:", report.empty_bodies);
    println!("  {:<18} {}", "extracted texts:", report.extracted_texts);
    println!("  {:<18} {:.1}s", "elapsed:", report.elapsed.as_secs_f64());
    if skip.count > 0 {
        println!(
            "{} {} document(s) were skipped or degraded — see {} for full details.",
            style("NOTE:").yellow().bold(),
            skip.count,
            skip.path.display()
        );
    } else {
        println!("  {:<18} {}", "report:", "none (nothing skipped)");
    }
    println!(
        "{} Previous indexes were kept (not deleted) at {}",
        style("BACKUP:").yellow().bold(),
        backup.display()
    );
    println!(
        "{}",
        style("Verify search in the UI, then remove that directory yourself to free the space.").dim()
    );
    println!("{} You can start the service now.", style("→").green());

    Ok(0)
}

/// Read every account's `override_from_headers` before the rebuild starts.
///
/// The handle is dropped when this returns; the wrap-up opens the same database
/// again to persist UID counters, and `MemDb` owns the directory while open.
fn load_account_headers(root: &Path) -> BichonResult<HashMap<u64, Option<Vec<String>>>> {
    let db = bichon_core::admin::meta::open_database(root.join("memdb"))?;
    let accounts: Vec<AccountModel> = db
        .collection(AccountModel::collection())
        .list_all()
        .map_err(|e| {
            raise_error!(
                format!("cannot read accounts from memdb: {e:#?}"),
                ErrorCode::InternalError
            )
        })?;
    Ok(accounts
        .into_iter()
        .map(|account| (account.id, account.override_from_headers))
        .collect())
}

/// Run both rebuild phases and accumulate the report.
fn run_phases(
    old_email: Option<&OldIndex>,
    old_attach: Option<&OldIndex>,
    email_target: &Path,
    attach_target: &Path,
    blobs: &Engine,
    account_headers: &HashMap<u64, Option<Vec<String>>>,
    skip_text: bool,
    skip: &mut SkipReport,
    uids: &mut AccountUidAssigner,
) -> BichonResult<ReindexReport> {
    let mut report = ReindexReport::default();

    if let Some(old) = old_email {
        let schema = old.index.schema();
        let of = OldEmailFields::resolve(&schema)?;
        let total = old.searcher.num_docs();
        println!(
            "{} Rebuilding email index: {} document(s) to examine...",
            style("·").dim(),
            total
        );
        let pb = progress_bar(total, "Rebuilding email index");
        let stats = rebuild_email(
            &old.searcher,
            &schema,
            &of,
            email_target,
            blobs,
            account_headers,
            &pb,
            skip,
            uids,
        )?;
        pb.finish_and_clear();
        report.emails = stats.docs;
        report.skipped = stats.skipped;
        report.empty_bodies = stats.empty_bodies;
        report.email_sample = stats.sample_id;
    }

    if let Some(old) = old_attach {
        let schema = old.index.schema();
        let af = OldAttachmentFields::resolve(&schema)?;
        // Parent lookup: prefer the still-open old email index.
        let email_ctx = match old_email {
            Some(e) => Some((&e.searcher, OldEmailFields::resolve(&e.index.schema())?)),
            None => None,
        };
        let total = old.searcher.num_docs();
        println!(
            "{} Rebuilding attachment index: {} document(s) to examine...",
            style("·").dim(),
            total
        );
        let pb = progress_bar(total, "Rebuilding attachment index");
        let stats = rebuild_attachments(
            &old.searcher,
            &schema,
            &af,
            email_ctx.as_ref().map(|(s, f)| (*s, f)),
            attach_target,
            blobs,
            account_headers,
            skip_text,
            &pb,
            skip,
        )?;
        pb.finish_and_clear();
        report.attachments = stats.docs;
        report.skipped += stats.skipped;
        report.extracted_texts = stats.extracted_texts;
        report.attachment_sample = stats.sample_id;
    }

    Ok(report)
}

// ─── root resolution ──────────────────────────────────────────────────────────

fn resolve_root(args: &ReindexArgs, theme: &ColorfulTheme) -> BichonResult<PathBuf> {
    if let Some(root) = args.root.as_deref().filter(|s| !s.trim().is_empty()) {
        let path = PathBuf::from(root.trim());
        validate_root(&path)?;
        return Ok(path);
    }

    loop {
        let input: String = Input::with_theme(theme)
            .with_prompt("Enter the absolute path for 'bichon_root_dir'")
            .interact_text()
            .unwrap_or_default();
        let path = PathBuf::from(input.trim());
        match validate_root(&path) {
            Ok(()) => return Ok(path),
            Err(e) => println!("{} {e:?}", style("→").yellow()),
        }
    }
}

fn validate_root(path: &Path) -> BichonResult<()> {
    if !path.is_absolute() {
        return Err(raise_error!("Path must be absolute.".into(), ErrorCode::InvalidParameter));
    }
    if !path.exists() {
        return Err(raise_error!("Directory does not exist.".into(), ErrorCode::InvalidParameter));
    }
    // Deliberately no `bichon-indices` check here: the index directory may live
    // elsewhere (`bichon_index_dir` / `--index-dir`), in which case nothing
    // index-related exists under the root. The resolved index root is checked in
    // the pre-checks instead.
    Ok(())
}

// ─── old index handles ────────────────────────────────────────────────────────

struct OldIndex {
    index: Index,
    #[allow(dead_code)]
    reader: tantivy::IndexReader,
    searcher: Searcher,
}

impl OldIndex {
    fn open(dir: &Path) -> BichonResult<Self> {
        let index = Index::open_in_dir(dir).map_err(|e| {
            raise_error!(format!("cannot open index {:?}: {e:#?}", dir), ErrorCode::InternalError)
        })?;
        let reader = index.reader().map_err(|e| {
            raise_error!(format!("cannot create reader for {:?}: {e:#?}", dir), ErrorCode::InternalError)
        })?;
        reader.reload().map_err(|e| {
            raise_error!(format!("reader reload failed for {:?}: {e:#?}", dir), ErrorCode::InternalError)
        })?;
        let searcher = reader.searcher();
        Ok(Self { index, reader, searcher })
    }
}

/// The old index's field ids differ from the current schema (the trigram sibling
/// fields were added), so reading an old document must resolve `Field` by **name**
/// from the old schema — `SchemaTools::email_fields()` (the new schema's ids)
/// must not be used here.
struct OldEmailFields {
    id: Field,
    message_id: Field,
    account_id: Field,
    mailbox_id: Field,
    uid: Field,
    /// `None` on a pre-v3 index: those documents never had a stable UID, and
    /// the rebuild assigns one instead of carrying it.
    imap_uid: Option<Field>,
    subject: Field,
    preview: Field,
    content_hash: Field,
    from: Field,
    to: Field,
    cc: Field,
    bcc: Field,
    date: Field,
    internal_date: Field,
    ingest_at: Field,
    size: Field,
    thread_id: Field,
    attachment_count: Field,
    regular_attachment_count: Field,
    attachments: Field,
    tags: Field,
    shard_id: Field,
    /// Absent before index format 4; the rebuild recomputes both from the raw
    /// message, so a missing field is not an error.
    from_name: Option<Field>,
    wire_from: Option<Field>,
}

impl OldEmailFields {
    fn resolve(schema: &Schema) -> BichonResult<Self> {
        let g = |n: &str| {
            schema.get_field(n).map_err(|e| {
                raise_error!(
                    format!("old email index is missing field '{n}': {e:#?}"),
                    ErrorCode::InternalError
                )
            })
        };
        Ok(Self {
            id: g("id")?,
            message_id: g("message_id")?,
            account_id: g("account_id")?,
            mailbox_id: g("mailbox_id")?,
            uid: g("uid")?,
            // Absent on a pre-v3 index, which is a normal state (not an error):
            // those documents get a freshly assigned stable UID.
            imap_uid: schema.get_field("imap_uid").ok(),
            subject: g("subject")?,
            preview: g("preview")?,
            content_hash: g("content_hash")?,
            from: g("from")?,
            to: g("to")?,
            cc: g("cc")?,
            bcc: g("bcc")?,
            date: g("date")?,
            internal_date: g("internal_date")?,
            ingest_at: g("ingest_at")?,
            size: g("size")?,
            thread_id: g("thread_id")?,
            attachment_count: g("attachment_count")?,
            regular_attachment_count: g("regular_attachment_count")?,
            attachments: g("attachments")?,
            tags: g("tags")?,
            shard_id: g("shard_id")?,
            from_name: schema.get_field("from_name").ok(),
            wire_from: schema.get_field("wire_from").ok(),
        })
    }
}

struct OldAttachmentFields {
    id: Field,
    envelope_id: Field,
    account_id: Field,
    mailbox_id: Field,
    subject: Field,
    content_hash: Field,
    from: Field,
    date: Field,
    ingest_at: Field,
    size: Field,
    ext: Field,
    category: Field,
    content_type: Field,
    shard_id: Field,
    has_text: Field,
    is_ocr: Field,
    page_count: Field,
    is_indexed: Field,
    is_message: Field,
    name_exact: Field,
    tags: Field,
    auto_tags: Field,
    /// Absent before index format 4; recomputed from the parent email when
    /// possible, otherwise carried from the old document.
    from_name: Option<Field>,
    wire_from: Option<Field>,
}

impl OldAttachmentFields {
    fn resolve(schema: &Schema) -> BichonResult<Self> {
        let g = |n: &str| {
            schema.get_field(n).map_err(|e| {
                raise_error!(
                    format!("old attachment index is missing field '{n}': {e:#?}"),
                    ErrorCode::InternalError
                )
            })
        };
        Ok(Self {
            id: g("id")?,
            envelope_id: g("eid")?,
            account_id: g("account_id")?,
            mailbox_id: g("mailbox_id")?,
            subject: g("subject")?,
            content_hash: g("content_hash")?,
            from: g("from")?,
            date: g("date")?,
            ingest_at: g("ingest_at")?,
            size: g("size")?,
            ext: g("attachment_ext")?,
            category: g("attachment_category")?,
            content_type: g("attachment_content_type")?,
            shard_id: g("shard_id")?,
            has_text: g("has_text")?,
            is_ocr: g("is_ocr")?,
            page_count: g("page_count")?,
            is_indexed: g("is_indexed")?,
            is_message: g("is_message")?,
            name_exact: g("name_exact")?,
            tags: g("tags")?,
            auto_tags: g("auto_tags")?,
            from_name: schema.get_field("from_name").ok(),
            wire_from: schema.get_field("wire_from").ok(),
        })
    }
}

// ─── Phase A: email ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn rebuild_email(
    old_searcher: &Searcher,
    old_schema: &Schema,
    of: &OldEmailFields,
    rebuild_dir: &Path,
    blobs: &Engine,
    account_headers: &HashMap<u64, Option<Vec<String>>>,
    pb: &ProgressBar,
    skip: &mut SkipReport,
    uids: &mut AccountUidAssigner,
) -> BichonResult<PhaseStats> {
    std::fs::create_dir_all(rebuild_dir).map_err(|e| {
        raise_error!(
            format!("cannot create rebuild directory {:?}: {e:#?}", rebuild_dir),
            ErrorCode::InternalError
        )
    })?;
    let new_index = Index::create_in_dir(rebuild_dir, SchemaTools::email_schema()).map_err(|e| {
        raise_error!(
            format!("cannot create new email index {:?}: {e:#?}", rebuild_dir),
            ErrorCode::InternalError
        )
    })?;
    new_index.tokenizers().register("euro", EuroTokenizer::new());
    new_index.tokenizers().register("tri", tri_analyzer());

    let mut writer = new_index
        .writer_with_num_threads(3, 256 * 1024 * 1024)
        .map_err(|e| raise_error!(format!("cannot create writer: {e:#?}"), ErrorCode::InternalError))?;
    writer.set_merge_policy(Box::new(NoMergePolicy));

    let mut stats = PhaseStats::default();
    let mut processed = 0u64;
    // Memoize the last content_hash → body (`None` = blob missing). Deduplicated
    // messages share a hash, so this avoids re-fetching; a memo hit still reports
    // a missing body per document, so the report stays complete.
    let mut last_body: Option<((u64, String), Option<(String, ResolvedSender)>)> = None;

    for seg in old_searcher.segment_readers() {
        let seg_id = seg.segment_id().uuid_string();
        // A segment whose store cannot be opened is not a per-document problem:
        // abort rather than silently lose the whole segment.
        let store = seg
            .get_store_reader(1)
            .map_err(|e| raise_error!(format!("cannot read doc store: {e:#?}"), ErrorCode::InternalError))?;
        let alive = seg.alive_bitset();

        for doc_id in 0..seg.max_doc() {
            if let Some(a) = alive {
                if !a.is_alive(doc_id) {
                    continue;
                }
            }

            // Advance once per *source* document, before any work and before
            // the skip paths below, so the bar keeps moving while individual
            // messages are being parsed and still reaches the total when some
            // documents cannot be carried over.
            processed += 1;
            pb.set_position(processed);
            if processed % COMMIT_BATCH == 0 {
                writer
                    .commit()
                    .map_err(|e| raise_error!(format!("commit failed: {e:#?}"), ErrorCode::InternalError))?;
            }

            let doc: TantivyDocument = match store.get(doc_id) {
                Ok(d) => d,
                Err(e) => {
                    skip.record(skip_record(
                        "unreadable_document",
                        "email",
                        &seg_id,
                        doc_id,
                        Some(format!("{e:#?}")),
                        None,
                    ));
                    stats.skipped += 1;
                    continue;
                }
            };

            let (mut envelope, attachments, shard_id) = match envelope_from_old_doc(&doc, of) {
                Ok(v) => v,
                Err(e) => {
                    skip.record(skip_record(
                        "skipped_document",
                        "email",
                        &seg_id,
                        doc_id,
                        Some(format!("{e:?}")),
                        Some(doc_fields_json(&doc, old_schema)),
                    ));
                    stats.skipped += 1;
                    continue;
                }
            };

            // Write-once: a document that already carries a stable UID keeps it
            // (the UID is what clients have seen); one that does not gets a
            // fresh number from the per-account counter, which is always above
            // every carried UID of that account.
            if envelope.imap_uid > 0 {
                uids.carry(envelope.account_id, envelope.imap_uid);
            } else {
                envelope.imap_uid = uids.assign(envelope.account_id)?;
            }

            // Recompute the stored sender from the raw message with the
            // account's *current* rule list. The old document only carries the
            // value produced at ingest time, which is exactly what a reindex
            // must not trust when the configuration has changed.
            let cache_key = (envelope.account_id, envelope.content_hash.clone());
            let content = match &last_body {
                Some((k, v)) if k == &cache_key => v.clone(),
                _ => {
                    let headers = account_headers
                        .get(&envelope.account_id)
                        .and_then(|h| h.as_deref());
                    let v = extract_content(blobs, &envelope.content_hash, headers);
                    last_body = Some((cache_key, v.clone()));
                    v
                }
            };
            let body = match content {
                Some((body, sender)) => {
                    envelope.from = sender
                        .address
                        .unwrap_or_else(|| "unknown".to_string());
                    envelope.from_name = sender.name;
                    envelope.wire_from = sender.wire_from;
                    body
                }
                None => {
                    // Keep the old sender fields: without the blob there is
                    // nothing to re-derive them from.
                    stats.empty_bodies += 1;
                    warn!(hash = %envelope.content_hash, "blob missing, leaving body empty");
                    skip.record(skip_record(
                        "missing_body",
                        "email",
                        &seg_id,
                        doc_id,
                        None,
                        Some(doc_fields_json(&doc, old_schema)),
                    ));
                    String::new()
                }
            };

            let sample_id = envelope.id.clone();
            let built = EnvelopeWithAttachments { envelope, attachments }
                .to_document(&body, shard_id);
            let new_doc = match built {
                Ok(d) => d,
                Err(e) => {
                    skip.record(skip_record(
                        "skipped_document",
                        "email",
                        &seg_id,
                        doc_id,
                        Some(format!("to_document failed: {e:?}")),
                        Some(doc_fields_json(&doc, old_schema)),
                    ));
                    stats.skipped += 1;
                    continue;
                }
            };
            if let Err(e) = writer.add_document(new_doc) {
                skip.record(skip_record(
                    "skipped_document",
                    "email",
                    &seg_id,
                    doc_id,
                    Some(format!("add_document failed: {e:?}")),
                    Some(doc_fields_json(&doc, old_schema)),
                ));
                stats.skipped += 1;
                continue;
            }

            if stats.sample_id.is_none() {
                stats.sample_id = Some(sample_id);
            }
            stats.docs += 1;
        }
    }

    pb.set_position(processed);
    pb.set_message("Finalizing email index (committing and merging segments)");
    writer
        .commit()
        .map_err(|e| raise_error!(format!("final commit failed: {e:#?}"), ErrorCode::InternalError))?;
    finish_writer(writer)?;
    drop(new_index);
    Ok(stats)
}

// ─── Phase B: attachment ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn rebuild_attachments(
    old_searcher: &Searcher,
    old_schema: &Schema,
    af: &OldAttachmentFields,
    email_ctx: Option<(&Searcher, &OldEmailFields)>,
    rebuild_dir: &Path,
    blobs: &Engine,
    account_headers: &HashMap<u64, Option<Vec<String>>>,
    skip_text: bool,
    pb: &ProgressBar,
    skip: &mut SkipReport,
) -> BichonResult<PhaseStats> {
    std::fs::create_dir_all(rebuild_dir).map_err(|e| {
        raise_error!(
            format!("cannot create rebuild directory {:?}: {e:#?}", rebuild_dir),
            ErrorCode::InternalError
        )
    })?;
    let new_index = Index::create_in_dir(rebuild_dir, SchemaTools::attachment_schema()).map_err(|e| {
        raise_error!(
            format!("cannot create new attachment index {:?}: {e:#?}", rebuild_dir),
            ErrorCode::InternalError
        )
    })?;
    new_index.tokenizers().register("euro", EuroTokenizer::new());
    new_index.tokenizers().register("tri", tri_analyzer());

    let mut writer = new_index
        .writer_with_num_threads(3, 256 * 1024 * 1024)
        .map_err(|e| raise_error!(format!("cannot create writer: {e:#?}"), ErrorCode::InternalError))?;
    writer.set_merge_policy(Box::new(NoMergePolicy));

    let extract = !skip_text && !text_extractor::is_noop();
    let mut stats = PhaseStats::default();
    let mut processed = 0u64;
    // Attachments of one message are usually adjacent; a one-entry cache keeps
    // the parent lookup + blob parse out of the hot path.
    let mut last_sender: Option<((u64, String), Option<ResolvedSender>)> = None;

    for seg in old_searcher.segment_readers() {
        let seg_id = seg.segment_id().uuid_string();
        let store = seg
            .get_store_reader(1)
            .map_err(|e| raise_error!(format!("cannot read doc store: {e:#?}"), ErrorCode::InternalError))?;
        let alive = seg.alive_bitset();

        for doc_id in 0..seg.max_doc() {
            if let Some(a) = alive {
                if !a.is_alive(doc_id) {
                    continue;
                }
            }

            // See `rebuild_email`: count source documents, not just the ones
            // written, so the bar advances on every iteration and ends at 100%.
            processed += 1;
            pb.set_position(processed);
            if processed % COMMIT_BATCH == 0 {
                writer
                    .commit()
                    .map_err(|e| raise_error!(format!("commit failed: {e:#?}"), ErrorCode::InternalError))?;
            }

            let doc: TantivyDocument = match store.get(doc_id) {
                Ok(d) => d,
                Err(e) => {
                    skip.record(skip_record(
                        "unreadable_document",
                        "attachment",
                        &seg_id,
                        doc_id,
                        Some(format!("{e:#?}")),
                        None,
                    ));
                    stats.skipped += 1;
                    continue;
                }
            };

            let mut model = match attachment_from_old_doc(&doc, af) {
                Ok(m) => m,
                Err(e) => {
                    skip.record(skip_record(
                        "skipped_document",
                        "attachment",
                        &seg_id,
                        doc_id,
                        Some(format!("{e:?}")),
                        Some(doc_fields_json(&doc, old_schema)),
                    ));
                    stats.skipped += 1;
                    continue;
                }
            };

            // Keep the attachment's sender in step with its parent envelope:
            // re-derive it from the parent's raw message and current rules.
            if let Some((account_id, content_hash)) = parent_email_meta(email_ctx, &model.envelope_id)
            {
                let key = (account_id, content_hash);
                let sender = match &last_sender {
                    Some((k, v)) if k == &key => v.clone(),
                    _ => {
                        let headers = account_headers.get(&account_id).and_then(|h| h.as_deref());
                        let v = extract_content(blobs, &key.1, headers).map(|(_, s)| s);
                        last_sender = Some((key, v.clone()));
                        v
                    }
                };
                if let Some(sender) = sender {
                    model.from = sender
                        .address
                        .unwrap_or_else(|| "unknown".to_string());
                    model.from_name = sender.name;
                    model.wire_from = sender.wire_from;
                }
            }

            if extract {
                match reextract_attachment_text(email_ctx, &model, blobs) {
                    Some(t) => {
                        model.text = Some(t.text);
                        model.has_text = true;
                        model.is_ocr = t.is_ocr;
                        model.page_count = t.page_count.map(|n| n as u64);
                        model.is_indexed = true;
                        stats.extracted_texts += 1;
                    }
                    None => {
                        reset_attachment_flags(&mut model);
                        stats.skipped += 1;
                        skip.record(skip_record(
                            "attachment_text_unavailable",
                            "attachment",
                            &seg_id,
                            doc_id,
                            None,
                            Some(doc_fields_json(&doc, old_schema)),
                        ));
                    }
                }
            } else {
                reset_attachment_flags(&mut model);
            }

            let sample_id = model.id.clone();
            if let Err(e) = writer.add_document(model.into_document()) {
                skip.record(skip_record(
                    "skipped_document",
                    "attachment",
                    &seg_id,
                    doc_id,
                    Some(format!("add_document failed: {e:?}")),
                    Some(doc_fields_json(&doc, old_schema)),
                ));
                stats.skipped += 1;
                continue;
            }

            if stats.sample_id.is_none() {
                stats.sample_id = Some(sample_id);
            }
            stats.docs += 1;
        }
    }

    pb.set_position(processed);
    pb.set_message("Finalizing attachment index (committing and merging segments)");
    writer
        .commit()
        .map_err(|e| raise_error!(format!("final commit failed: {e:#?}"), ErrorCode::InternalError))?;
    finish_writer(writer)?;
    drop(new_index);
    Ok(stats)
}

/// Locate the attachment blob via its parent email and re-extract text.
/// Any miss returns `None` (the caller resets the flags and records the skip).
fn reextract_attachment_text(
    email_ctx: Option<(&Searcher, &OldEmailFields)>,
    model: &AttachmentModel,
    blobs: &Engine,
) -> Option<text_extractor::ExtractedText> {
    let (email_searcher, of) = email_ctx?;

    let ext = model.ext.clone().unwrap_or_default();
    if !text_extractor::should_try_extract(&model.content_type, &ext) {
        return None;
    }
    if model.size > text_extractor::max_extract_bytes() as u64 {
        return None;
    }

    let bytes = parent_attachment_bytes(
        email_searcher,
        of,
        &model.envelope_id,
        &model.content_hash,
        blobs,
    )?;
    text_extractor::extract_text(&model.content_type, &ext, &bytes)
}

/// Locate the parent email document and return its `(account_id, content_hash)`.
/// Used to re-derive an attachment's sender from the message it belongs to.
fn parent_email_meta(
    email_ctx: Option<(&Searcher, &OldEmailFields)>,
    envelope_id: &str,
) -> Option<(u64, String)> {
    let (email_searcher, of) = email_ctx?;
    let term = Term::from_field_text(of.id, envelope_id);
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    let hits = email_searcher
        .search(&query, &TopDocs::with_limit(1).order_by_score())
        .ok()?;
    let (_, addr) = hits.first()?;
    let doc: TantivyDocument = email_searcher.doc(*addr).ok()?;
    Some((opt_u64(&doc, of.account_id)?, opt_str(&doc, of.content_hash)?))
}

/// Point-lookup the parent email → parse its f_attachments JSON → match by
/// content_hash → fetch the blob.
fn parent_attachment_bytes(
    email_searcher: &Searcher,
    of: &OldEmailFields,
    envelope_id: &str,
    attachment_content_hash: &str,
    blobs: &Engine,
) -> Option<Vec<u8>> {
    let term = Term::from_field_text(of.id, envelope_id);
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    let hits = email_searcher
        .search(&query, &TopDocs::with_limit(1).order_by_score())
        .ok()?;
    let (_, addr) = hits.first()?;
    let doc: TantivyDocument = email_searcher.doc(*addr).ok()?;
    let json = doc.get_first(of.attachments).and_then(|v| v.as_str())?;
    let infos: Vec<AttachmentInfo> = serde_json::from_str(json).ok()?;
    let info = infos
        .iter()
        .find(|a| a.content_hash == attachment_content_hash)?;
    let key = hex_to_key(info.storage_hash()).ok()?;
    blobs.get(&key).ok()?
}

fn reset_attachment_flags(model: &mut AttachmentModel) {
    model.text = None;
    model.has_text = false;
    model.is_ocr = false;
    model.page_count = None;
    model.is_indexed = false;
}

// ─── document reconstruction ──────────────────────────────────────────────────

fn envelope_from_old_doc(
    doc: &TantivyDocument,
    of: &OldEmailFields,
) -> BichonResult<(Envelope, Option<Vec<AttachmentInfo>>, u64)> {
    let attachments: Option<Vec<AttachmentInfo>> = doc
        .get_first(of.attachments)
        .and_then(|v| v.as_str())
        .and_then(|json| serde_json::from_str(json).ok());

    let tags = facets(doc, of.tags);

    let envelope = Envelope {
        id: req_str(doc, of.id, "id")?,
        message_id: req_str(doc, of.message_id, "message_id")?,
        account_id: req_u64(doc, of.account_id, "account_id")?,
        account_email: None,
        account_name: None,
        mailbox_id: req_u64(doc, of.mailbox_id, "mailbox_id")?,
        mailbox_name: None,
        uid: req_u64(doc, of.uid, "uid")? as u32,
        // Carried verbatim when the old document had one (write-once), 0 when
        // the old index predates the field. `rebuild_email` resolves the 0 case
        // by assigning from the per-account counter.
        imap_uid: of
            .imap_uid
            .and_then(|f| opt_u64(doc, f))
            .unwrap_or(0) as u32,
        subject: req_str(doc, of.subject, "subject")?,
        preview: opt_str(doc, of.preview).unwrap_or_default(),
        from: req_str(doc, of.from, "from")?,
        from_name: of.from_name.and_then(|f| opt_str(doc, f)),
        wire_from: of.wire_from.and_then(|f| opt_str(doc, f)),
        to: vec_str(doc, of.to),
        cc: vec_str(doc, of.cc),
        bcc: vec_str(doc, of.bcc),
        date: req_i64(doc, of.date, "date")?,
        internal_date: req_i64(doc, of.internal_date, "internal_date")?,
        ingest_at: req_i64(doc, of.ingest_at, "ingest_at")?,
        size: req_u64(doc, of.size, "size")? as u32,
        thread_id: req_str(doc, of.thread_id, "thread_id")?,
        attachment_count: req_u64(doc, of.attachment_count, "attachment_count")? as usize,
        regular_attachment_count: req_u64(
            doc,
            of.regular_attachment_count,
            "regular_attachment_count",
        )? as usize,
        tags: (!tags.is_empty()).then_some(tags),
        content_hash: req_str(doc, of.content_hash, "content_hash")?,
        // A reindex deliberately resets every cleanup-move marker.
        //
        // The flag answers "has cleanup already moved this message out of its
        // source folder?", and a rebuild from the old index cannot answer it:
        // the source UID may be stale and the old documents may predate the
        // field entirely. Rather than carry a value of unknown correctness,
        // every message becomes a candidate again and cleanup re-decides from
        // the source mailbox — messages that are genuinely gone are skipped as
        // `source_uid_missing`, with no data loss. Operators should expect one
        // cleanup pass to revisit previously moved mail after a reindex.
        cleanup_moved: false,
    };
    let shard_id = opt_u64(doc, of.shard_id).unwrap_or(0);
    Ok((envelope, attachments, shard_id))
}

fn attachment_from_old_doc(
    doc: &TantivyDocument,
    af: &OldAttachmentFields,
) -> BichonResult<AttachmentModel> {
    let tags = facets(doc, af.tags);
    let auto_tags = facets(doc, af.auto_tags);
    Ok(AttachmentModel {
        id: req_str(doc, af.id, "id")?,
        envelope_id: req_str(doc, af.envelope_id, "eid")?,
        account_id: req_u64(doc, af.account_id, "account_id")?,
        account_email: None,
        mailbox_id: req_u64(doc, af.mailbox_id, "mailbox_id")?,
        mailbox_name: None,
        subject: req_str(doc, af.subject, "subject")?,
        content_hash: req_str(doc, af.content_hash, "content_hash")?,
        from: req_str(doc, af.from, "from")?,
        from_name: af.from_name.and_then(|f| opt_str(doc, f)),
        wire_from: af.wire_from.and_then(|f| opt_str(doc, f)),
        date: req_i64(doc, af.date, "date")?,
        ingest_at: req_i64(doc, af.ingest_at, "ingest_at")?,
        size: req_u64(doc, af.size, "size")?,
        ext: opt_str(doc, af.ext),
        category: req_str(doc, af.category, "attachment_category")?,
        content_type: req_str(doc, af.content_type, "attachment_content_type")?,
        shard_id: opt_u64(doc, af.shard_id).unwrap_or(0),
        text: None,
        has_text: opt_bool(doc, af.has_text).unwrap_or(false),
        is_ocr: opt_bool(doc, af.is_ocr).unwrap_or(false),
        page_count: opt_u64(doc, af.page_count),
        is_indexed: opt_bool(doc, af.is_indexed).unwrap_or(false),
        is_message: opt_bool(doc, af.is_message).unwrap_or(false),
        name: opt_str(doc, af.name_exact),
        tags: (!tags.is_empty()).then_some(tags),
        auto_tags: (!auto_tags.is_empty()).then_some(auto_tags),
    })
}

// ─── field reading helpers ────────────────────────────────────────────────────

fn opt_str(doc: &TantivyDocument, f: Field) -> Option<String> {
    doc.get_first(f).and_then(|v| v.as_str()).map(str::to_string)
}

fn req_str(doc: &TantivyDocument, f: Field, name: &str) -> BichonResult<String> {
    opt_str(doc, f).ok_or_else(|| {
        raise_error!(format!("old document is missing string field '{name}'"), ErrorCode::InternalError)
    })
}

fn opt_u64(doc: &TantivyDocument, f: Field) -> Option<u64> {
    doc.get_first(f).and_then(|v| v.as_u64())
}

fn req_u64(doc: &TantivyDocument, f: Field, name: &str) -> BichonResult<u64> {
    opt_u64(doc, f).ok_or_else(|| {
        raise_error!(format!("old document is missing u64 field '{name}'"), ErrorCode::InternalError)
    })
}

fn req_i64(doc: &TantivyDocument, f: Field, name: &str) -> BichonResult<i64> {
    doc.get_first(f).and_then(|v| v.as_i64()).ok_or_else(|| {
        raise_error!(format!("old document is missing i64 field '{name}'"), ErrorCode::InternalError)
    })
}

fn opt_bool(doc: &TantivyDocument, f: Field) -> Option<bool> {
    doc.get_first(f).and_then(|v| v.as_bool())
}

fn vec_str(doc: &TantivyDocument, f: Field) -> Vec<String> {
    doc.get_all(f)
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn facets(doc: &TantivyDocument, f: Field) -> Vec<String> {
    doc.get_all(f)
        .filter_map(|v| v.as_facet())
        .filter_map(|enc| Facet::from_encoded(enc.as_bytes().to_vec()).ok())
        .map(|facet| facet.to_string())
        .collect()
}

// ─── skip report (NDJSON) ─────────────────────────────────────────────────────

/// NDJSON writer for documents the reindex could not carry over cleanly.
///
/// Record kinds:
/// - `unreadable_document`: the stored document could not be deserialized at all
///   (no field data is available).
/// - `skipped_document`: the document could not be reconstructed or written, so
///   it is absent from the new index.
/// - `missing_body`: the document WAS indexed, but its body blob was absent, so
///   its body text is empty (it is still findable by metadata).
/// - `attachment_text_unavailable`: the attachment was indexed with text
///   extraction reset (orphan parent / missing blob / unsupported type).
///
/// Every record carries `segment_id` + `doc_id` and, when the document could be
/// read, a `fields` object with every stored field, so the operator can re-fetch
/// the message from the source mailbox (by `uid` / `message_id`) or elsewhere.
struct SkipReport {
    path: PathBuf,
    file: Option<std::fs::File>,
    count: u64,
}

impl SkipReport {
    fn new(path: PathBuf) -> Self {
        // Drop a stale report so it cannot be mistaken for this run's output.
        let _ = std::fs::remove_file(&path);
        Self { path, file: None, count: 0 }
    }

    fn record(&mut self, value: serde_json::Value) {
        if self.file.is_none() {
            match create_private_report_file(&self.path) {
                Ok(f) => self.file = Some(f),
                Err(e) => {
                    warn!(error = %e, path = %self.path.display(), "cannot create reindex skip report");
                    return;
                }
            }
        }
        if let Some(f) = self.file.as_mut() {
            let line = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
            if writeln!(f, "{line}").is_ok() {
                self.count += 1;
            }
        }
    }
}

/// Create the report file with owner-only permissions where the platform
/// supports it: the report carries plain-text subjects, addresses and message
/// ids, so it should not be world-readable.
fn create_private_report_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn skip_record(
    kind: &str,
    phase: &str,
    segment_id: &str,
    doc_id: u32,
    error: Option<String>,
    fields: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("kind".into(), serde_json::Value::String(kind.into()));
    obj.insert("phase".into(), serde_json::Value::String(phase.into()));
    obj.insert("segment_id".into(), serde_json::Value::String(segment_id.into()));
    obj.insert("doc_id".into(), serde_json::Value::from(doc_id));
    if let Some(e) = error {
        obj.insert("error".into(), serde_json::Value::String(e));
    }
    if let Some(f) = fields {
        obj.insert("fields".into(), f);
    }
    serde_json::Value::Object(obj)
}

/// Dump every stored field of `doc` as `{field_name: value}`; repeated field
/// names (to/cc/bcc/tags) become arrays. Best-effort: unknown value types become
/// `null`.
fn doc_fields_json(doc: &TantivyDocument, schema: &Schema) -> serde_json::Value {
    use serde_json::map::Entry;
    use serde_json::Value as J;

    let mut map = serde_json::Map::new();
    for (field, value) in doc.field_values() {
        let name = schema.get_field_name(field).to_string();
        let v = if let Some(s) = value.as_str() {
            J::String(s.to_string())
        } else if let Some(n) = value.as_u64() {
            J::from(n)
        } else if let Some(n) = value.as_i64() {
            J::from(n)
        } else if let Some(b) = value.as_bool() {
            J::from(b)
        } else if let Some(f) = value.as_facet() {
            J::String(f.to_string())
        } else {
            J::Null
        };
        match map.entry(name) {
            Entry::Vacant(e) => {
                e.insert(v);
            }
            Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if let J::Array(arr) = existing {
                    arr.push(v);
                } else {
                    let prev = existing.take();
                    *existing = J::Array(vec![prev, v]);
                }
            }
        }
    }
    J::Object(map)
}

// ─── blob / body ──────────────────────────────────────────────────────────────

fn hex_to_key(hex: &str) -> BichonResult<[u8; 32]> {
    let mut key = [0u8; 32];
    hex::decode_to_slice(hex, &mut key).map_err(|e| {
        raise_error!(
            format!("invalid content hash '{hex}': {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    Ok(key)
}

/// Rebuild the body text and the stored sender from the raw message blob.
///
/// The sender is resolved with the account's current header rules, which is
/// what lets a reindex apply a newly configured (or changed) rule to mail that
/// was archived before the rule existed.
fn extract_content(
    blobs: &Engine,
    content_hash: &str,
    override_headers: Option<&[String]>,
) -> Option<(String, ResolvedSender)> {
    let key = hex_to_key(content_hash).ok()?;
    let bytes = blobs.get(&key).ok()??;
    let message = MessageParser::new().parse(&bytes)?;
    let text = message
        .body_text(0)
        .map(|cow| cow.into_owned())
        .or_else(|| message.body_html(0).map(|cow| extract_text(cow.into_owned())))
        .unwrap_or_default();
    let sender = resolve_sender(&message, override_headers);
    Some((
        text.split_whitespace().collect::<Vec<_>>().join(" "),
        sender,
    ))
}

// ─── wrap-up helpers ──────────────────────────────────────────────────────────

fn finish_writer(mut writer: tantivy::IndexWriter) -> BichonResult<()> {
    let seg_ids = writer.index().searchable_segment_ids().map_err(|e| {
        raise_error!(format!("searchable_segment_ids failed: {e:#?}"), ErrorCode::InternalError)
    })?;
    if seg_ids.len() > 1 {
        let _ = writer.merge(&seg_ids).wait();
    }
    writer.wait_merging_threads().map_err(|e| {
        raise_error!(format!("wait_merging_threads failed: {e:#?}"), ErrorCode::InternalError)
    })?;
    Ok(())
}

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let mut s = dir.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// The final path component of `p` (e.g. `mail_metadata`).
fn leaf(p: &Path) -> BichonResult<std::ffi::OsString> {
    p.file_name().map(|s| s.to_os_string()).ok_or_else(|| {
        raise_error!(format!("path {:?} has no final component", p), ErrorCode::InternalError)
    })
}

/// `<index root>.old-<timestamp>` — a unique name, so repeated reindexes never
/// collide and older backups are never overwritten. If the timestamped name is
/// already taken (two runs in the same second), a numeric suffix is appended.
fn backup_path(index_root: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut candidate = sibling(index_root, &format!(".old-{stamp}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = sibling(index_root, &format!(".old-{stamp}-{n}"));
        n += 1;
    }
    candidate
}

/// Sanity-check a freshly built index BEFORE it replaces the live one: it must
/// carry the current format marker, report exactly `expected` documents, answer
/// a `Count(AllQuery)` query over the whole index, and return the sample document
/// by its id. Any failure aborts the reindex with the live index untouched.
fn validate_rebuild(
    dir: &Path,
    expected: u64,
    sample_id: Option<&str>,
    id_field: Field,
    label: &str,
) -> BichonResult<()> {
    if check_index_format(dir) != IndexFormatStatus::Current {
        return Err(raise_error!(
            format!("validation: rebuilt {label} index {:?} is not stamped at the current format", dir),
            ErrorCode::InternalError
        ));
    }

    let index = Index::open_in_dir(dir).map_err(|e| {
        raise_error!(
            format!("validation: cannot open rebuilt {label} index {:?}: {e:#?}", dir),
            ErrorCode::InternalError
        )
    })?;
    index.tokenizers().register("euro", EuroTokenizer::new());
    index.tokenizers().register("tri", tri_analyzer());
    let reader = index.reader().map_err(|e| {
        raise_error!(
            format!("validation: cannot create reader for rebuilt {label} index: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    reader.reload().map_err(|e| {
        raise_error!(
            format!("validation: reader reload failed for rebuilt {label} index: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    let searcher = reader.searcher();

    let docs = searcher.num_docs();
    if docs != expected {
        return Err(raise_error!(
            format!("validation: rebuilt {label} index has {docs} documents, expected {expected}"),
            ErrorCode::InternalError
        ));
    }

    // A query over the whole index proves the term dictionary and collectors are
    // readable, not just the doc store.
    let matched = searcher.search(&AllQuery, &Count).map_err(|e| {
        raise_error!(
            format!("validation: query on rebuilt {label} index failed: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    if matched as u64 != expected {
        return Err(raise_error!(
            format!("validation: rebuilt {label} index matched {matched} documents for AllQuery, expected {expected}"),
            ErrorCode::InternalError
        ));
    }

    // A real term round-trip on one known document id.
    if let Some(id) = sample_id {
        let term = Term::from_field_text(id_field, id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let hits = searcher.search(&query, &Count).map_err(|e| {
            raise_error!(
                format!("validation: sample query on rebuilt {label} index failed: {e:#?}"),
                ErrorCode::InternalError
            )
        })?;
        if hits == 0 {
            return Err(raise_error!(
                format!("validation: rebuilt {label} index cannot find its own sample document '{id}'"),
                ErrorCode::InternalError
            ));
        }
    }

    println!(
        "{} {} index validated: {} documents, marker at v{}",
        style("✓").green(),
        label,
        docs,
        CURRENT_INDEX_FORMAT_VERSION
    );
    Ok(())
}

/// Gate the rebuilt email index on the stable-UID invariants that make IMAP
/// incremental clients safe: every document carries a non-zero `imap_uid`, and
/// no two documents of the same account share one.
///
/// Without this, a rebuild bug (a missed carry, a counter that started too low)
/// would silently hand the same UID to two messages — exactly the failure this
/// whole mechanism exists to prevent — and only show up later as mail that
/// clients never fetch. Failing here keeps the live index in place.
///
/// Pairs are collected and sorted rather than hashed: it is an exact check at
/// 12 bytes per document instead of a hash-table-sized one.
fn validate_stable_uids(dir: &Path) -> BichonResult<()> {
    let index = Index::open_in_dir(dir).map_err(|e| {
        raise_error!(
            format!("validation: cannot open rebuilt email index {:?}: {e:#?}", dir),
            ErrorCode::InternalError
        )
    })?;
    let fields = SchemaTools::email_fields();
    let reader = index.reader().map_err(|e| {
        raise_error!(
            format!("validation: cannot create reader for rebuilt email index: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    reader.reload().map_err(|e| {
        raise_error!(
            format!("validation: reader reload failed for rebuilt email index: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    let searcher = reader.searcher();

    // This pass reads every rebuilt document, which on a large archive is long
    // enough that a silent wait would look like a hang.
    let pb = progress_bar(searcher.num_docs(), "Validating rebuilt email index");
    let mut processed = 0u64;
    let mut pairs: Vec<(u64, u32)> = Vec::with_capacity(searcher.num_docs() as usize);
    for seg in searcher.segment_readers() {
        let seg_id = seg.segment_id().uuid_string();
        let store = seg.get_store_reader(1).map_err(|e| {
            raise_error!(format!("validation: cannot read doc store: {e:#?}"), ErrorCode::InternalError)
        })?;
        let alive = seg.alive_bitset();
        for doc_id in 0..seg.max_doc() {
            if let Some(a) = alive {
                if !a.is_alive(doc_id) {
                    continue;
                }
            }
            processed += 1;
            pb.set_position(processed);
            let doc: TantivyDocument = store.get(doc_id).map_err(|e| {
                raise_error!(
                    format!("validation: cannot read document {doc_id} of {seg_id}: {e:#?}"),
                    ErrorCode::InternalError
                )
            })?;
            let account_id = doc
                .get_first(fields.f_account_id)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let imap_uid = doc
                .get_first(fields.f_imap_uid)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if imap_uid == 0 {
                return Err(raise_error!(
                    format!(
                        "validation: document {doc_id} of {seg_id} (account {account_id}) has no stable IMAP UID"
                    ),
                    ErrorCode::InternalError
                ));
            }
            pairs.push((account_id, imap_uid as u32));
        }
    }

    pb.set_message("Checking for duplicate stable IMAP UIDs");
    pairs.sort_unstable();
    for window in pairs.windows(2) {
        if window[0] == window[1] {
            return Err(raise_error!(
                format!(
                    "validation: stable IMAP UID {} is assigned to two documents of account {}",
                    window[0].1, window[0].0
                ),
                ErrorCode::InternalError
            ));
        }
    }
    pb.finish_and_clear();

    println!(
        "{} email index stable UIDs validated: {} unique (account, uid) pairs",
        style("✓").green(),
        pairs.len()
    );
    Ok(())
}

fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..3 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    Err(last.unwrap())
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(entry.path()),
                Ok(_) => {
                    if let Ok(md) = entry.metadata() {
                        total += md.len();
                    }
                }
                Err(_) => {}
            }
        }
    }
    total
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

const PROGRESS_TEMPLATE: &str =
    "{msg} [{bar:32.cyan/blue}] {pos}/{len} ({percent}%) {per_sec} elapsed {elapsed_precise} ETA {eta}";

/// A bar that never looks frozen: the position is advanced for every source
/// document (see the rebuild loops), and the steady tick keeps the elapsed
/// clock moving during long per-document work such as parsing a message or
/// committing a batch.
fn progress_bar(total: u64, msg: &'static str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(PROGRESS_TEMPLATE)
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(500));
    pb
}

/// Holds the blob engine and flushes + shuts it down on Drop, releasing the LOCK
/// on both the success and failure paths.
struct EngineGuard(Option<Engine>);

impl Drop for EngineGuard {
    fn drop(&mut self) {
        if let Some(engine) = self.0.take() {
            let _ = engine.flush();
            let _ = engine.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bichon_core::migrate::write_storage_version;
    use tantivy::collector::Count;
    use tantivy::query::QueryParser;
    use tantivy::schema::{
        FacetOptions, IndexRecordOption as IRO, TextFieldIndexing, TextOptions, FAST, INDEXED,
        STORED, STRING,
    };

    fn temp_root(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("bichon-reindex-test")
            .join(prefix)
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hex32(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    fn text_store_opts() -> TextOptions {
        TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("euro")
                    .set_index_option(IRO::WithFreqsAndPositions),
            )
            .set_stored()
    }

    /// A legacy-style email schema: same field names as the current one but in a
    /// **different order**, so this also verifies that reindex resolves FieldIds
    /// by name rather than relying on ids.
    fn old_email_schema() -> Schema {
        let mut b = Schema::builder();
        b.add_text_field("id", STRING | STORED | FAST);
        b.add_text_field("content_hash", STRING | STORED | FAST);
        b.add_u64_field("shard_id", INDEXED | STORED | FAST);
        b.add_text_field("message_id", STRING | STORED);
        b.add_u64_field("account_id", INDEXED | STORED | FAST);
        b.add_u64_field("mailbox_id", INDEXED | STORED | FAST);
        b.add_u64_field("uid", INDEXED | STORED | FAST);
        b.add_text_field("subject", text_store_opts());
        b.add_text_field("preview", STORED);
        b.add_text_field("from", STRING | STORED | FAST);
        b.add_text_field("to", STRING | STORED);
        b.add_text_field("cc", STRING | STORED);
        b.add_text_field("bcc", STRING | STORED);
        b.add_i64_field("date", INDEXED | STORED | FAST);
        b.add_i64_field("internal_date", INDEXED | STORED | FAST);
        b.add_i64_field("ingest_at", INDEXED | STORED | FAST);
        b.add_u64_field("size", INDEXED | STORED | FAST);
        b.add_text_field("thread_id", STRING | STORED | FAST);
        b.add_u64_field("attachment_count", INDEXED | STORED | FAST);
        b.add_u64_field("regular_attachment_count", INDEXED | STORED | FAST);
        b.add_text_field("attachments", STORED);
        b.add_facet_field("tags", FacetOptions::default().set_stored());
        b.build()
    }

    fn old_attachment_schema() -> Schema {
        let mut b = Schema::builder();
        b.add_text_field("id", STRING | STORED | FAST);
        b.add_text_field("content_hash", STRING | STORED | FAST);
        b.add_text_field("eid", STRING | STORED | FAST);
        b.add_text_field("subject", text_store_opts());
        b.add_text_field("from", STRING | STORED | FAST);
        b.add_u64_field("account_id", INDEXED | STORED | FAST);
        b.add_u64_field("mailbox_id", INDEXED | STORED | FAST);
        b.add_i64_field("date", INDEXED | STORED | FAST);
        b.add_i64_field("ingest_at", INDEXED | STORED | FAST);
        b.add_u64_field("size", INDEXED | STORED | FAST);
        b.add_text_field("attachment_ext", STRING | STORED | FAST);
        b.add_text_field("attachment_category", STRING | STORED | FAST);
        b.add_text_field("attachment_content_type", STRING | STORED | FAST);
        b.add_u64_field("shard_id", INDEXED | STORED | FAST);
        b.add_bool_field("has_text", INDEXED | STORED | FAST);
        b.add_bool_field("is_ocr", INDEXED | STORED | FAST);
        b.add_u64_field("page_count", INDEXED | STORED | FAST);
        b.add_bool_field("is_indexed", INDEXED | STORED | FAST);
        b.add_bool_field("is_message", INDEXED | STORED | FAST);
        b.add_text_field("name_exact", STRING | STORED);
        b.add_facet_field("tags", FacetOptions::default().set_stored());
        b.add_facet_field("auto_tags", FacetOptions::default().set_stored());
        b.build()
    }

    fn put_blob(blobs_dir: &Path, hex_hash: &str, data: &[u8]) {
        let mut config = Config::default();
        config.default_codec = Codec::Zstd;
        config.compress_threshold = 1024;
        config.flush_interval_secs = 0;
        config.gc_interval_secs = 0;
        let engine = Engine::open(blobs_dir, config).unwrap();
        let mut key = [0u8; 32];
        hex::decode_to_slice(hex_hash, &mut key).unwrap();
        engine.put(key, data, Codec::Lz4).unwrap();
        engine.flush().unwrap();
        engine.shutdown().unwrap();
    }

    /// Fake data root: a blob engine (1 body) + 2 emails (with tags / attachment
    /// JSON) + 1 attachment, all with **no** INDEX_FORMAT marker. The second
    /// email's blob is deliberately absent so the run exercises the
    /// `missing_body` report path.
    /// Write `(eid, account_id, imap_uid)` documents into a fresh v3 index.
    fn build_uid_fixture(prefix: &str, docs: &[(&str, u64, u64)]) -> PathBuf {
        let dir = temp_root(prefix);
        let index = Index::create_in_dir(&dir, SchemaTools::email_schema()).unwrap();
        index.tokenizers().register("euro", EuroTokenizer::new());
        index.tokenizers().register("tri", tri_analyzer());
        let mut writer = index.writer_with_num_threads(1, 15_000_000).unwrap();
        let f = SchemaTools::email_fields();
        for (eid, account_id, imap_uid) in docs {
            let mut doc = TantivyDocument::new();
            doc.add_text(f.f_id, *eid);
            doc.add_u64(f.f_account_id, *account_id);
            doc.add_u64(f.f_imap_uid, *imap_uid);
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();
        drop(writer);
        drop(index);
        dir
    }

    /// The gate that turns a silent rebuild bug into a failed reindex: a
    /// duplicate or missing stable UID would make incremental IMAP clients skip
    /// messages, and by then the live index is already gone.
    #[test]
    fn t9_stable_uid_validation_rejects_duplicates_and_gaps() {
        let ok = build_uid_fixture("uid-gate-ok", &[("env-1", 1, 5), ("env-2", 1, 6), ("env-3", 2, 5)]);
        validate_stable_uids(&ok).expect("distinct (account, uid) pairs must pass");
        let _ = std::fs::remove_dir_all(&ok);

        let duplicate =
            build_uid_fixture("uid-gate-dup", &[("env-1", 1, 5), ("env-2", 1, 5)]);
        let err = validate_stable_uids(&duplicate).expect_err("a duplicate must fail");
        assert!(
            format!("{err:?}").contains("assigned to two documents"),
            "unexpected error: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&duplicate);

        let unassigned = build_uid_fixture("uid-gate-zero", &[("env-1", 1, 0)]);
        let err = validate_stable_uids(&unassigned).expect_err("an unassigned UID must fail");
        assert!(
            format!("{err:?}").contains("no stable IMAP UID"),
            "unexpected error: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&unassigned);
    }

    fn build_fixture(prefix: &str) -> (PathBuf, ResolvedDirs) {
        let root = temp_root(prefix);
        let dirs = resolve_dirs(&root, None, None);
        write_storage_version(&root, 2).unwrap();
        std::fs::create_dir_all(&dirs.envelope_dir).unwrap();
        std::fs::create_dir_all(&dirs.attachment_dir).unwrap();
        std::fs::create_dir_all(&dirs.blobs_dir).unwrap();

        let body_eml = b"From: alice@example.com\r\n\
                         To: bob@example.com\r\n\
                         Subject: Rechnung\r\n\
                         Date: Thu, 01 Jan 2023 00:00:00 +0000\r\n\
                         Message-ID: <m1@x>\r\n\
                         \r\n\
                         laserdrucker brother toner\r\n";
        put_blob(&dirs.blobs_dir, &hex32(0xaa), body_eml);

        // ── legacy email index ──
        {
            let index = Index::create_in_dir(&dirs.envelope_dir, old_email_schema()).unwrap();
            index.tokenizers().register("euro", EuroTokenizer::new());
            let mut w = index.writer_with_num_threads(1, 15_000_000).unwrap();
            let s = index.schema();
            let f = |n: &str| s.get_field(n).unwrap();

            let atts = serde_json::json!([{
                "file_type": "application/pdf",
                "inline": false,
                "filename": "invoice.pdf",
                "size": 5000,
                "content_id": null,
                "content_hash": "decoded-hash-1",
                "raw_content_hash": "raw-hash-1",
                "is_message": false
            }]);

            let mut d1 = TantivyDocument::new();
            d1.add_text(f("id"), "env-1");
            d1.add_text(f("message_id"), "<m1@x>");
            d1.add_u64(f("account_id"), 1);
            d1.add_u64(f("mailbox_id"), 10);
            d1.add_u64(f("uid"), 1);
            d1.add_text(f("subject"), "Rechnung");
            d1.add_text(f("preview"), "laserdrucker...");
            d1.add_text(f("content_hash"), hex32(0xaa));
            d1.add_text(f("from"), "alice@example.com");
            d1.add_text(f("to"), "bob@example.com");
            d1.add_i64(f("date"), 1_700_000_000_000);
            d1.add_i64(f("internal_date"), 1_700_000_000_000);
            d1.add_i64(f("ingest_at"), 1_700_000_000_000);
            d1.add_u64(f("size"), 123);
            d1.add_text(f("thread_id"), "t-1");
            d1.add_u64(f("attachment_count"), 1);
            d1.add_u64(f("regular_attachment_count"), 1);
            d1.add_text(f("attachments"), atts.to_string());
            d1.add_facet(f("tags"), "/inbox");
            d1.add_u64(f("shard_id"), 0);
            w.add_document(d1).unwrap();

            let mut d2 = TantivyDocument::new();
            d2.add_text(f("id"), "env-2");
            d2.add_text(f("message_id"), "<m2@x>");
            d2.add_u64(f("account_id"), 1);
            d2.add_u64(f("mailbox_id"), 10);
            d2.add_u64(f("uid"), 2);
            d2.add_text(f("subject"), "Zweite Mail");
            d2.add_text(f("preview"), "nichts");
            d2.add_text(f("content_hash"), hex32(0xbb)); // no blob for this one
            d2.add_text(f("from"), "carol@example.com");
            d2.add_text(f("to"), "dave@example.com");
            d2.add_i64(f("date"), 1_700_000_001_000);
            d2.add_i64(f("internal_date"), 1_700_000_001_000);
            d2.add_i64(f("ingest_at"), 1_700_000_001_000);
            d2.add_u64(f("size"), 45);
            d2.add_text(f("thread_id"), "t-2");
            d2.add_u64(f("attachment_count"), 0);
            d2.add_u64(f("regular_attachment_count"), 0);
            d2.add_u64(f("shard_id"), 0);
            w.add_document(d2).unwrap();
            w.commit().unwrap();
        }

        // ── legacy attachment index ──
        {
            let index = Index::create_in_dir(&dirs.attachment_dir, old_attachment_schema()).unwrap();
            index.tokenizers().register("euro", EuroTokenizer::new());
            let mut w = index.writer_with_num_threads(1, 15_000_000).unwrap();
            let s = index.schema();
            let f = |n: &str| s.get_field(n).unwrap();
            let mut d = TantivyDocument::new();
            d.add_text(f("id"), "att-1");
            d.add_text(f("content_hash"), "decoded-hash-1");
            d.add_text(f("eid"), "env-1");
            d.add_text(f("subject"), "Rechnung");
            d.add_text(f("from"), "alice@example.com");
            d.add_u64(f("account_id"), 1);
            d.add_u64(f("mailbox_id"), 10);
            d.add_i64(f("date"), 1_700_000_000_000);
            d.add_i64(f("ingest_at"), 1_700_000_000_000);
            d.add_u64(f("size"), 5000);
            d.add_text(f("attachment_ext"), "pdf");
            d.add_text(f("attachment_category"), "document");
            d.add_text(f("attachment_content_type"), "application/pdf");
            d.add_u64(f("shard_id"), 0);
            d.add_bool(f("has_text"), false);
            d.add_bool(f("is_ocr"), false);
            d.add_bool(f("is_indexed"), false);
            d.add_bool(f("is_message"), false);
            d.add_text(f("name_exact"), "invoice.pdf");
            w.add_document(d).unwrap();
            w.commit().unwrap();
        }

        (root, dirs)
    }

    fn open_registered(dir: &Path) -> Index {
        let index = Index::open_in_dir(dir).unwrap();
        index.tokenizers().register("euro", EuroTokenizer::new());
        index.tokenizers().register("tri", tri_analyzer());
        index
    }

    /// Every `<index root>.old-*` backup directory sitting next to `index_root`.
    fn backups_under(index_root: &Path) -> Vec<PathBuf> {
        let parent = index_root.parent().unwrap();
        let prefix = format!("{}.old-", index_root.file_name().unwrap().to_string_lossy());
        let mut found: Vec<PathBuf> = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn reindex_rebuilds_preserves_and_swaps() {
        let (root, dirs) = build_fixture("main");
        let args = ReindexArgs::default();

        let code = execute(&root, &dirs, &args, false).expect("reindex ok");
        assert_eq!(code, 0);

        // Live dirs are stamped with the current INDEX_FORMAT
        assert_eq!(
            check_index_format(&dirs.envelope_dir),
            IndexFormatStatus::Current
        );
        assert_eq!(
            check_index_format(&dirs.attachment_dir),
            IndexFormatStatus::Current
        );
        // .old exists (kept aside, never deleted)
        let backups = backups_under(&dirs.index_root);
        assert_eq!(backups.len(), 1, "exactly one backup root, got {backups:?}");
        assert!(
            dirs.envelope_dir.exists(),
            "the live index root must be the freshly rebuilt one"
        );
        // The temp build dir is gone (renamed into place).
        assert!(!sibling(&dirs.index_root, ".new").exists());

        // ── email index: doc count + body/subject/tags/attachment-name/hash preserved ──
        let index = open_registered(&dirs.envelope_dir);
        let reader = index.reader().unwrap();
        reader.reload().unwrap();
        let searcher = reader.searcher();
        assert_eq!(
            searcher.num_docs(),
            2,
            "both messages are indexed (the second just has an empty body)"
        );
        let f = SchemaTools::email_fields();

        // Body re-derived from the blob
        let body_q = QueryParser::for_index(&index, vec![f.f_body])
            .parse_query("laserdrucker")
            .unwrap();
        assert_eq!(searcher.search(&body_q, &Count).unwrap(), 1, "body from blob");
        // Subject (new deterministic tokenizer)
        let subj_q = QueryParser::for_index(&index, vec![f.f_subject])
            .parse_query("Rechnung")
            .unwrap();
        let subj_hits = searcher
            .search(&subj_q, &TopDocs::with_limit(1).order_by_score())
            .unwrap();
        assert_eq!(subj_hits.len(), 1);

        // Tags preserved
        let tag_q = TermQuery::new(
            Term::from_facet(f.f_tags, &Facet::from("/inbox")),
            IRO::Basic,
        );
        assert_eq!(searcher.search(&tag_q, &Count).unwrap(), 1, "tags preserved");

        // Attachment name + storage-domain content hash preserved
        let doc: TantivyDocument = searcher.doc(subj_hits[0].1).unwrap();
        assert_eq!(
            doc.get_first(f.f_attachment_content_hash)
                .and_then(|v| v.as_str()),
            Some("raw-hash-1"),
            "attachment storage-domain hash preserved"
        );
        let name_q = TermQuery::new(
            Term::from_field_text(f.f_attachment_name_exact, "invoice.pdf"),
            IRO::Basic,
        );
        assert_eq!(
            searcher.search(&name_q, &Count).unwrap(),
            1,
            "attachment name preserved"
        );

        // ── attachment index: doc count + name preserved ──
        let aindex = open_registered(&dirs.attachment_dir);
        let areader = aindex.reader().unwrap();
        areader.reload().unwrap();
        let asearcher = areader.searcher();
        assert_eq!(asearcher.num_docs(), 1, "attachment doc count preserved");
        let af = SchemaTools::attachment_fields();
        let aname_q = TermQuery::new(
            Term::from_field_text(af.f_name_exact, "invoice.pdf"),
            IRO::Basic,
        );
        assert_eq!(asearcher.search(&aname_q, &Count).unwrap(), 1);

        // ── the NDJSON report recorded the missing body ──
        let report = std::fs::read_to_string(root.join(DEFAULT_REPORT_FILE)).unwrap();
        let lines: Vec<serde_json::Value> = report
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1, "exactly one degraded document");
        assert_eq!(lines[0]["kind"], "missing_body");
        assert_eq!(lines[0]["phase"], "email");
        // Identifying fields must be present so the message can be re-fetched.
        assert_eq!(lines[0]["fields"]["id"], "env-2");
        assert_eq!(lines[0]["fields"]["uid"], 2);
        assert_eq!(lines[0]["fields"]["message_id"], "<m2@x>");
        assert_eq!(lines[0]["fields"]["content_hash"], hex32(0xbb));
    }

    /// The tool never deletes the previous indexes — it only renames the whole
    /// root aside. A second reindex must therefore produce a SECOND backup under
    /// a fresh timestamp, never overwriting the first.
    #[test]
    fn reindex_never_deletes_and_never_overwrites_backups() {
        let (root, dirs) = build_fixture("backup");
        let args = ReindexArgs::default();

        assert_eq!(execute(&root, &dirs, &args, false).unwrap(), 0);
        let first = backups_under(&dirs.index_root);
        assert_eq!(first.len(), 1, "one backup after the first run");

        // Force a second rebuild; it must not remove or overwrite the first backup.
        let forced = ReindexArgs {
            force: true,
            ..ReindexArgs::default()
        };
        assert_eq!(execute(&root, &dirs, &forced, false).unwrap(), 0);
        let second = backups_under(&dirs.index_root);
        assert_eq!(second.len(), 2, "a second backup is added, none removed: {second:?}");
        assert!(
            second.iter().all(|p| first.contains(p) || p != &first[0]),
            "the first backup must survive"
        );
        assert_eq!(
            check_index_format(&dirs.envelope_dir),
            IndexFormatStatus::Current
        );
    }

    #[test]
    fn reindex_is_noop_when_already_current() {
        let (root, dirs) = build_fixture("current");
        let args = ReindexArgs::default();
        // Run once to become current
        assert_eq!(execute(&root, &dirs, &args, false).unwrap(), 0);
        // Run again: already current, exits 0, and leaves no temp build dir
        assert_eq!(execute(&root, &dirs, &args, false).unwrap(), 0);
        assert!(!sibling(&dirs.index_root, NEW_SUFFIX).exists());
    }

    /// A deployment can put the index tree outside the data root
    /// (`bichon_index_dir` / `--index-dir`). The root must still validate
    /// even though it holds no `bichon-indices`, and the resolved directories
    /// must follow the custom parent.
    #[test]
    fn custom_index_parent_is_honoured() {
        let root = temp_root("custom-index-root");
        write_storage_version(&root, 2).unwrap();
        assert!(
            !root.join("bichon-indices").exists(),
            "fixture must not have an index under the root"
        );

        // This used to be rejected outright, before `--index-dir` was even
        // taken into account.
        validate_root(&root).expect("a root without bichon-indices must still be valid");

        let index_parent = temp_root("custom-index-parent");
        let dirs = resolve_dirs(&root, Some(&index_parent), None);
        assert_eq!(
            dirs.envelope_dir,
            index_parent.join("bichon-indices").join("mail_metadata")
        );
        assert_eq!(
            dirs.attachment_dir,
            index_parent.join("bichon-indices").join("attachment_metadata")
        );
        // The blob store still comes from the root when no data parent is given.
        assert_eq!(dirs.blobs_dir, root.join("bichon-storage").join("blobs"));
    }

    /// `reindex` must let the operator point the index and the blob data at
    /// different disks, using the same spellings as the server. Both the
    /// primary names and the server-style `--bichon-*` aliases parse.
    /// The rebuild bar must show rate, elapsed time and ETA — a typo in the
    /// template would silently fall back to the bare default bar.
    #[test]
    fn progress_template_is_valid() {
        assert!(ProgressStyle::with_template(PROGRESS_TEMPLATE).is_ok());
    }

    #[test]
    fn index_and_data_dirs_can_be_set_separately() {
        let args = ReindexArgs::try_parse_from([
            "reindex",
            "--root",
            "/data",
            "--index-dir",
            "/ssd",
            "--data-dir",
            "/hdd",
        ])
        .expect("primary dir flags parse");
        assert_eq!(args.root.as_deref(), Some("/data"));
        assert_eq!(args.index_parent.as_deref(), Some("/ssd"));
        assert_eq!(args.data_parent.as_deref(), Some("/hdd"));

        let aliased = ReindexArgs::try_parse_from([
            "reindex",
            "--bichon-root-dir",
            "/data",
            "--bichon-index-dir",
            "/ssd",
            "--bichon-data-dir",
            "/hdd",
        ])
        .expect("server-style aliases parse");
        assert_eq!(aliased.index_parent.as_deref(), Some("/ssd"));
        assert_eq!(aliased.data_parent.as_deref(), Some("/hdd"));

        let dirs = resolve_dirs(
            Path::new("/data"),
            Some(Path::new("/ssd")),
            Some(Path::new("/hdd")),
        );
        assert_eq!(
            dirs.envelope_dir,
            Path::new("/ssd").join("bichon-indices").join("mail_metadata")
        );
        assert_eq!(
            dirs.attachment_dir,
            Path::new("/ssd")
                .join("bichon-indices")
                .join("attachment_metadata")
        );
        assert_eq!(
            dirs.blobs_dir,
            Path::new("/hdd").join("bichon-storage").join("blobs")
        );
    }
}
