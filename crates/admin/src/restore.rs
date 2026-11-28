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

//! Disaster recovery: restore a backup point from the configured backup
//! target into an empty directory. Restore lives in `bichon-admin` (not the
//! server binary) so a fresh disaster-recovery box — which has no data root
//! yet — can run it.
//!
//! The backend connection details come from a JSON config file
//! (`--config` / interactive prompt) — either an S3-compatible endpoint or a
//! local directory. Restore deliberately does not read the local install's
//! metadata store, so it needs no encryption password and runs unchanged on a
//! fresh box.
//!
//! The interactive flow validates everything up front and tells the operator
//! exactly which item is wrong: the config file, the backend connection
//! (an S3 endpoint's credentials/bucket/prefix, or a local directory's mount;
//! each diagnosed individually), the target directories (R7 clean +
//! disjoint), and the free space on each target's mount point — all before a
//! single byte is written.
//!
//! The engine is the shared core restore ([`bichon_core::backup::restore`]):
//! the community edition ships no audit replayer, so Pro/Enterprise audit
//! deltas are restored verbatim under `audit/deltas/` (nothing is silently
//! dropped); the Pro admin build registers its replayer before running.

use std::path::{Path, PathBuf};

use bichon_core::{
    backup::{
        config::RestoreFileConfig,
        restore::{
            check_restore_space, check_restore_targets, format_restore_report,
            restore_options_from_cli, restore_preflight, run_restore, RestoreOptions,
        },
    },
    settings::cli::RestoreArgs,
};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};

/// Run a restore from parsed CLI arguments (`bichon-admin restore …`).
/// `verify_only` turns the run into the Pro verify-only drill (the community
/// binary always passes `false`). Returns the process exit code.
pub async fn run_cli(args: RestoreArgs, verify_only: bool) -> i32 {
    init_tracing();

    let opts = match restore_options_from_cli(&args, verify_only) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{} Restore failed: {e:?}", style("ERROR:").red().bold());
            return 1;
        }
    };

    match run_restore(&opts).await {
        Ok(report) => {
            print!("{}", format_restore_report(&report));
            println!("\n{}", style("Restore complete.").green().bold());
            0
        }
        Err(e) => {
            eprintln!("{} Restore failed: {e:?}", style("ERROR:").red().bold());
            1
        }
    }
}

/// Interactive restore (admin main-menu entry). Everything is validated
/// before the download starts:
///
/// 1. restore config file (JSON) — re-prompted until it parses;
/// 2. S3 connection test + manifest read — the error is mapped to the
///    config item that caused it;
/// 3. target root / index dir / data dir (index and data optional, default
///    layout under the target root) — re-prompted until R7-clean;
/// 4. free-space check per mount point;
/// 5. confirmation, then the restore runs.
///
/// `verify_only` turns the run into the Pro verify-only drill (the community
/// binary always passes `false`).
pub async fn run_interactive(theme: &ColorfulTheme, verify_only: bool) -> i32 {
    println!(
        "\n{}",
        style("Restore a backup point from the configured backup target into an empty directory.")
            .bold()
    );
    println!(
        "{}\n",
        style("The target must be empty and disjoint from any live data root (R7).").dim()
    );

    // ── 1+2: config file, then the target connection test ─────────────────
    let (pre, mut opts) = loop {
        let path: String = Input::with_theme(theme)
            .with_prompt("Path to the restore config file (JSON, see README)")
            .interact_text()
            .unwrap_or_default();
        let cfg = match RestoreFileConfig::load(Path::new(path.trim())) {
            Ok(cfg) => cfg,
            Err(e) => {
                println!("{} {e:?}", style("Config file problem:").red());
                continue;
            }
        };
        let point: String = Input::with_theme(theme)
            .with_prompt("Restore point to use (blank = LATEST)")
            .allow_empty(true)
            .interact_text()
            .unwrap_or_default();
        let point = point.trim().to_string();
        let opts = RestoreOptions {
            target_type: cfg.target_type,
            s3_uri: cfg.s3_uri(),
            local_dir: cfg.local_dir.as_deref().map(PathBuf::from),
            prefix: cfg.prefix.clone(),
            point: if point.is_empty() { None } else { Some(point) },
            into: Default::default(),
            index_dir: None,
            data_dir: None,
            verify_only,
            access_key: cfg.s3_access_key.clone(),
            secret_key: cfg.s3_secret_key.clone(),
            region: cfg.s3_region.clone(),
            live_root: None,
        };
        println!(
            "\n{}",
            style(format!("Testing connection to {} ...", cfg.target_display())).dim()
        );
        match restore_preflight(&opts).await {
            Ok(pre) => break (pre, opts),
            Err(e) => {
                println!(
                    "{}\n{}",
                    style("Connection test failed:").red().bold(),
                    diagnose_connection_error(&e)
                );
            }
        }
    };

    println!(
        "\n{} {} (created {})\n  {} objects, ~{} to download\n  {}",
        style("Backup target OK. Restore point:").green().bold(),
        style(&pre.manifest.id).cyan(),
        pre.manifest.created,
        pre.objects_total,
        human_bytes(pre.bytes_total),
        style("Leave the point blank on re-runs to pick an older one (m-<id>).").dim(),
    );

    // ── 3: target directories (re-prompted until R7-clean) ────────────────
    let targets = loop {
        let into: String = Input::with_theme(theme)
            .with_prompt("Target data root directory (must be empty or not exist)")
            .interact_text()
            .unwrap_or_default();
        let into = into.trim().to_string();
        if into.is_empty() {
            println!("{} The target data root is required.", style("→").yellow());
            continue;
        }
        let index_dir: String = Input::with_theme(theme)
            .with_prompt(format!(
                "Index parent directory (blank = {}/bichon-indices)",
                into
            ))
            .allow_empty(true)
            .interact_text()
            .unwrap_or_default();
        let data_dir: String = Input::with_theme(theme)
            .with_prompt(format!(
                "Blob data parent directory (blank = {}/bichon-storage)",
                into
            ))
            .allow_empty(true)
            .interact_text()
            .unwrap_or_default();

        opts.into = PathBuf::from(&into);
        opts.index_dir = non_empty(index_dir).map(PathBuf::from);
        opts.data_dir = non_empty(data_dir).map(PathBuf::from);

        match check_restore_targets(&opts) {
            Ok(targets) => break targets,
            Err(e) => {
                println!("{} {e:?}", style("Target directory problem:").red());
            }
        }
    };

    // ── 4: free space per mount point ─────────────────────────────────────
    if let Err(e) = check_restore_space(&targets, &pre.manifest) {
        eprintln!("{} {e:?}", style("ERROR:").red().bold());
        return 1;
    }
    println!(
        "{} (need ~{}, per-mount-point check passed)",
        style("Disk space OK.").green(),
        human_bytes((pre.bytes_total as f64 * DISK_SAFETY_FACTOR) as u64),
    );

    // ── 5: confirm and run ────────────────────────────────────────────────
    if !Confirm::with_theme(theme)
        .with_prompt(format!(
            "Restore point {} into {} now?",
            style(&pre.manifest.id).cyan(),
            style(opts.into.display()).cyan()
        ))
        .default(true)
        .interact()
        .unwrap()
    {
        println!("{}", style("Cancelled.").dim());
        return 0;
    }

    match run_restore(&opts).await {
        Ok(report) => {
            print!("{}", format_restore_report(&report));
            println!("\n{}", style("Restore complete.").green().bold());
            0
        }
        Err(e) => {
            eprintln!("{} Restore failed: {e:?}", style("ERROR:").red().bold());
            1
        }
    }
}

const DISK_SAFETY_FACTOR: f64 = 1.1;

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn human_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// Map a preflight failure to the config item the operator should look at.
fn diagnose_connection_error(e: &impl std::fmt::Debug) -> String {
    let msg = format!("{e:?}").to_lowercase();
    if msg.contains("no restore point found") {
        return format!(
            "The backup target is reachable, but no restore point was found at the given \
             prefix. Check `prefix` in the config file (default: bichon-backup) and \
             make sure a backup has actually run.\nDetails: {e:?}"
        );
    }
    // The target is fine; the restore point itself is from a newer release.
    // Re-prompting for S3 settings would never fix that, so say it plainly.
    if msg.contains("newer version of bichon") {
        return format!(
            "The backup point was written by a newer version of Bichon than this tool. \
             Run the restore with the same or a newer Bichon release; downgrading does \
             not restore points that contain data this build cannot read.\nDetails: {e:?}"
        );
    }
    // Local target: the failures are filesystem ones, and the actionable
    // question behind each is "is the mount attached, writable, and not
    // full?" — none of which the S3 branches below would say.
    if msg.contains("canonicalize")
        || msg.contains("no such file")
        || msg.contains("does not exist")
        || msg.contains("not a directory")
    {
        return format!(
            "The local backup directory could not be opened. If it is a mount point, check \
             that the volume is attached and mounted before running this.\nDetails: {e:?}"
        );
    }
    if msg.contains("permission denied") || msg.contains("os error 13") {
        return format!(
            "The local backup directory is not readable by this user. Check the mount's \
             permissions.\nDetails: {e:?}"
        );
    }
    if msg.contains("read-only") || msg.contains("os error 30") {
        return format!(
            "The local backup directory is on a read-only mount. Remount it read-write.\n\
             Details: {e:?}"
        );
    }
    if msg.contains("no space left") || msg.contains("os error 28") {
        return format!(
            "The filesystem holding the local backup directory is full.\nDetails: {e:?}"
        );
    }
    if msg.contains("stale file handle") || msg.contains("os error 116") {
        return format!(
            "The network filesystem handle went stale — the share was probably remounted. \
             Remount the target and retry.\nDetails: {e:?}"
        );
    }
    if msg.contains("403")
        || msg.contains("accessdenied")
        || msg.contains("invalidaccesskeyid")
        || msg.contains("signaturedoesnotmatch")
        || msg.contains("credential")
    {
        return format!(
            "The endpoint was reached, but the credentials were rejected. Check \
             `s3_access_key` / `s3_secret_key` in the config file.\nDetails: {e:?}"
        );
    }
    if msg.contains("nosuchbucket")
        || (msg.contains("404") && msg.contains("bucket"))
        || msg.contains("the specified bucket")
    {
        return format!(
            "The endpoint was reached, but the bucket does not exist. Check `s3_bucket` \
             in the config file.\nDetails: {e:?}"
        );
    }
    if msg.contains("connect")
        || msg.contains("transport")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("dns")
        || msg.contains("refused")
        || msg.contains("unreachable")
        || msg.contains("certificate")
    {
        return format!(
            "The S3 endpoint could not be reached. Check `s3_endpoint` in the config \
             file (host/port reachable from this machine? scheme http vs https?).\nDetails: {e:?}"
        );
    }
    format!("Unexpected problem:\nDetails: {e:?}")
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
