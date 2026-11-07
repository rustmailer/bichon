use crate::api::export::{
    create_export_job, download_export_to_file, get_export_job, list_saved_searches,
    preview_export, verify_export_job,
};
use bichon_core::export::{ExportFormat, ExportJobView};
use bichon_core::saved_search::SavedSearchKind;
use dialoguer::Select;
use crate::api::download::download_and_export_with_json_header;
use crate::api::search::search_messages;
use crate::api::stats::fetch_account_stats;
use crate::BichonCliConfig;
use bichon_core::account::payload::MinimalAccount;
use console::style;
use dialoguer::Confirm;
use dialoguer::{theme::ColorfulTheme, Input};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use sysinfo::Disks;

pub async fn handle_account_export(
    config: &BichonCliConfig,
    account: MinimalAccount,
    theme: &ColorfulTheme,
) {
    let client = Client::new();

    println!("Fetching account statistics...");
    let stats = match fetch_account_stats(&client, config, account.id).await {
        Some(s) => s,
        None => {
            eprintln!("{} Failed to fetch account statistics.", style("✘").red());
            return;
        }
    };

    println!("\n--- Account Statistics ---");
    println!("  Total Emails: {}", style(stats.total_count).cyan());
    println!(
        "  Total Size:   {}",
        style(format_bytes(stats.total_size)).cyan()
    );

    let path = loop {
        let input: String = Input::with_theme(theme)
            .with_prompt("Enter ABSOLUTE directory path for MBOX file")
            .interact_text()
            .unwrap();

        let p = PathBuf::from(&input);

        if !p.is_absolute() {
            eprintln!(
                " {} {}",
                style("✘").red(),
                style("Invalid path: Must be an absolute path.").red()
            );
            continue;
        }

        if !p.exists() {
            eprintln!(
                " {} {}",
                style("✘").red(),
                style("Invalid path: Directory does not exist.").red()
            );
            continue;
        }

        if !p.is_dir() {
            eprintln!(
                " {} {}",
                style("✘").red(),
                style("Invalid path: The path provided is not a directory.").red()
            );
            continue;
        }
        break p;
    };

    let disks = Disks::new_with_refreshed_list();
    let disk_result = disks
        .list()
        .iter()
        .find(|d| path.starts_with(d.mount_point()))
        .ok_or_else(|| "Could not identify the disk for the provided path.");

    match disk_result {
        Ok(disk) => {
            let free_space = disk.available_space();
            let required_space = (stats.total_size as f64 * 1.2) as u64;

            if free_space < required_space {
                eprintln!(
                " {} Insufficient disk space (including 10% safety buffer)!\n   Required:  {} (Base: {})\n   Available: {}",
                style("✘").red(),
                style(format_bytes(required_space)).yellow(),
                style(format_bytes(stats.total_size)).yellow(),
                style(format_bytes(free_space)).yellow()
            );
                return;
            }

            println!(
                " {} Disk space check passed. (Required: {}, Available: {})",
                style("✔").green(),
                style(format_bytes(required_space)).cyan(),
                style(format_bytes(free_space)).cyan()
            );
        }
        Err(e) => {
            eprintln!(" {} {}", style("✘").red(), style(e).red());
            return;
        }
    }
    let mbox_file = get_unique_mbox_path(&path, account.id, &account.email);
    if Confirm::with_theme(theme)
        .with_prompt(format!(
            "Export {} emails to '{}'?",
            stats.total_count,
            mbox_file.display()
        ))
        .default(true)
        .interact()
        .unwrap()
    {
        println!(
            " {} Starting export ({} items per page)...",
            style("✔").green(),
            100
        );

        let pb = ProgressBar::new(stats.total_count as u64);
        pb.set_style(ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}"
        ).unwrap());

        let mut file = match tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&mbox_file)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!(" ✘ Failed to open file '{}': {}", path.display(), e);
                return;
            }
        };

        let page_size = 100;
        let mut current_page = 1;
        let mut total_pages;

        loop {
            let account_ids = Some(std::collections::HashSet::from([account.id]));
            if let Some(batch) = search_messages(&client, config, account_ids, current_page, page_size).await {
                total_pages = batch.total_pages.unwrap();

                pb.set_message(format!("Page {}/{}", current_page, total_pages));

                for envelope in batch.items {
                    let success =
                        download_and_export_with_json_header(&client, config, envelope.clone(), &mut file)
                            .await;

                    if !success {
                        eprintln!(
                            " ✘ Failed to export email {}, skipping...",
                            envelope.id
                        );
                        continue;
                    }
                    pb.inc(1);
                }
                if current_page >= total_pages {
                    break;
                }
                current_page += 1;
            } else {
                pb.finish_with_message("Error");
                eprintln!(
                    " ✘ Failed to fetch page {}. Aborting process...",
                    current_page
                );
                return;
            }
        }
        pb.finish();
        println!(" {} Export complete!", style("✔").green());
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{:.2} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KB", bytes / 1024)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MB", bytes / 1024 / 1024)
    } else {
        format!("{:.2} GB", bytes / 1024 / 1024 / 1024)
    }
}

fn get_unique_mbox_path(base_dir: &Path, account_id: u64, email: &str) -> PathBuf {
    let email_part = email.replace(' ', "_");

    let mut base_name = format!("account_{}_{}", account_id, email_part);
    if base_name.starts_with('.') {
        base_name = format!("_{}", base_name);
    }

    let mut final_path = base_dir.join(format!("{}.mbox", base_name));

    let mut counter = 1;
    while final_path.exists() {
        final_path = base_dir.join(format!("{}_{}.mbox", base_name, counter));
        counter += 1;
    }
    final_path
}

/// Saved-search driven batch export: pick a format (mbox of whole messages,
/// or a zip of the attachments an attachment search matches), list matching
/// saved searches, preview the scope, confirm, start the server-side job,
/// poll until it finishes and download the artifact.
pub async fn handle_export(config: &BichonCliConfig, theme: &ColorfulTheme) {
    let client = Client::new();

    let format = Select::with_theme(theme)
        .with_prompt("Select export format")
        .items(&[
            "Mbox — whole messages (requires an email saved search)",
            "Attachments — matching attachments packed in a zip (requires an attachment saved search)",
        ])
        .default(0)
        .interact()
        .unwrap();
    let format = match format {
        0 => ExportFormat::Mbox,
        _ => ExportFormat::Attachments,
    };
    let kind = match format {
        ExportFormat::Mbox => SavedSearchKind::Email,
        ExportFormat::Attachments => SavedSearchKind::Attachment,
    };
    let item_label = match format {
        ExportFormat::Mbox => "emails",
        ExportFormat::Attachments => "attachments",
    };

    println!("Fetching saved searches...");
    let saved = list_saved_searches(&client, config, &kind).await;
    let searches: Vec<_> = saved
        .into_iter()
        .filter(|s| s.kind == kind)
        .collect();
    if searches.is_empty() {
        eprintln!(
            " {} No {} saved searches found. Save a search on the web UI first, then retry.",
            style("✘").red(),
            match kind {
                SavedSearchKind::Email => "email",
                SavedSearchKind::Attachment => "attachment",
            }
        );
        return;
    }

    let names: Vec<String> = searches.iter().map(|s| s.name.clone()).collect();
    let idx = Select::with_theme(theme)
        .with_prompt("Select a saved search to export")
        .items(&names)
        .default(0)
        .interact()
        .unwrap();
    let search = &searches[idx];

    println!("\n--- Export Preview ---");
    let Some(preview) = preview_export(&client, config, &search.id, format).await else {
        return;
    };
    println!("  Saved search: {}", style(&preview.saved_search_name).cyan());
    println!("  Accounts:     {}", style(preview.accounts.len()).cyan());
    for account in &preview.accounts {
        println!("    - {}", style(&account.email).yellow());
    }
    // Attachments are deduplicated by content: the count is unique files.
    println!("  Items:        {}", style(preview.total_emails).cyan());
    println!("  Estimated:    {}", style(format_bytes(preview.total_size)).cyan());

    if !Confirm::with_theme(theme)
        .with_prompt(format!(
            "Export {} {} from '{}'?",
            preview.total_emails, item_label, preview.saved_search_name
        ))
        .default(true)
        .interact()
        .unwrap()
    {
        println!("Export cancelled.");
        return;
    }

    println!(" {} Starting export job...", style("✔").green());
    let Some(mut job) = create_export_job(&client, config, &search.id, format).await else {
        return;
    };
    println!(" {} Job: {}", style("✔").green(), job.job_id);

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}").unwrap());
    pb.enable_steady_tick(std::time::Duration::from_millis(200));
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let Some(updated) = get_export_job(&client, config, &job.job_id).await else {
            continue;
        };
        job = updated;
        pb.set_message(format!(
            "Export status: {} ({}/{} processed, {} exported, {} failed)",
            job.status, job.processed, job.total_emails, job.exported, job.failed
        ));
        match job.status.as_str() {
            "finished" => break,
            "failed" | "cancelled" => {
                pb.finish_with_message(format!("Export {}", job.status));
                eprintln!(
                    "\n {} Export ended with status '{}'{}",
                    style("✘").red(),
                    job.status,
                    job.error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                );
                return;
            }
            _ => {}
        }
    }
    pb.finish_with_message("Export finished");

    let path = loop {
        let input: String = Input::with_theme(theme)
            .with_prompt("Enter directory to save the export file")
            .default(".".into())
            .interact_text()
            .unwrap();
        let p = PathBuf::from(&input);
        if !p.is_dir() {
            eprintln!(
                " {} Invalid directory: '{}'.",
                style("✘").red(),
                p.display()
            );
            continue;
        }
        break p;
    };
    // The server names the artifact ({job_id}.mbox / {job_id}.zip); keep its
    // extension rather than guessing from the format.
    let file_name = job
        .artifact_name
        .clone()
        .unwrap_or_else(|| format!("{}.mbox", job.job_id));
    let target = path.join(file_name);
    println!(
        " {} Downloading to '{}'...",
        style("✔").green(),
        target.display()
    );
    if download_export_to_file(&client, config, &job.job_id, &target).await {
        println!(
            " {} Export complete: {}",
            style("✔").green(),
            target.display()
        );
        handle_post_export_verify(&client, config, &job, theme).await;
    }
}

/// Optional compliance verification after a saved-search export: prints the
/// artifact SHA-256 and, when the server supports it, cross-checks every
/// exported message against the live archive.
async fn handle_post_export_verify(
    client: &Client,
    config: &BichonCliConfig,
    job: &ExportJobView,
    theme: &ColorfulTheme,
) {
    if let Some(hash) = job.artifact_hash.as_deref() {
        println!("  Artifact SHA-256: {}", style(hash).cyan());
    }
    if !Confirm::with_theme(theme)
        .with_prompt("Run compliance verification against the archive?")
        .default(true)
        .interact()
        .unwrap()
    {
        return;
    }
    let Some(view) = verify_export_job(client, config, &job.job_id).await else {
        return;
    };
    println!("\n--- Compliance Verification ---");
    println!(
        "  Artifact hash match: {}",
        style(if view.artifact_hash_match { "YES" } else { "NO" }).green()
    );
    println!(
        "  Messages: {} checked, {} matched, {} mismatched",
        style(view.checked).cyan(),
        style(view.matched).green(),
        style(view.mismatched).red()
    );
    for m in view.mismatches.iter().take(10) {
        eprintln!(
            "  ✘ Mismatch: {} (expected {}, got {})",
            m.envelope_id,
            m.expected_hash.as_deref().unwrap_or("-"),
            m.actual_hash.as_deref().unwrap_or("-")
        );
    }
    if view.artifact_hash_match && view.mismatched == 0 {
        println!(" {} Verification passed.", style("✔").green());
    } else {
        eprintln!(" {} Verification FAILED.", style("✘").red());
    }
}

/// Verifies an existing export job by job id (compliance).
pub async fn handle_export_verify(config: &BichonCliConfig, theme: &ColorfulTheme) {
    let client = Client::new();
    let job_id: String = Input::with_theme(theme)
        .with_prompt("Export job id (e.g. exp_...)")
        .interact_text()
        .unwrap();
    let job_id = job_id.trim().to_string();
    if job_id.is_empty() {
        eprintln!(" {} No job id provided.", style("✘").red());
        return;
    }
    println!(" {} Verifying export job '{}'...", style("✔").green(), job_id);
    let Some(view) = verify_export_job(&client, config, &job_id).await else {
        return;
    };
    println!("\n--- Compliance Verification ---");
    println!(
        "  Artifact: {}",
        style(view.artifact_name.as_deref().unwrap_or("-")).cyan()
    );
    println!(
        "  Artifact hash match: {}",
        style(if view.artifact_hash_match { "YES" } else { "NO" }).green()
    );
    println!(
        "  Messages: {} checked, {} matched, {} mismatched",
        style(view.checked).cyan(),
        style(view.matched).green(),
        style(view.mismatched).red()
    );
    for m in view.mismatches.iter().take(10) {
        eprintln!(
            "  ✘ Mismatch: {} (expected {}, got {})",
            m.envelope_id,
            m.expected_hash.as_deref().unwrap_or("-"),
            m.actual_hash.as_deref().unwrap_or("-")
        );
    }
    if view.artifact_hash_match && view.mismatched == 0 {
        println!(" {} Verification passed.", style("✔").green());
    } else {
        eprintln!(" {} Verification FAILED.", style("✘").red());
    }
}
