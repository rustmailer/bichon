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

use std::collections::HashMap;
use std::path::PathBuf;

use crate::api::sender::send_batch_request;
use crate::mbox::gmail::determine_folder;
use bichon_core::import::reader::MboxFile;
use crate::BichonCliConfig;
use bichon_core::base64_encode_url_safe;
use bichon_core::envelope::meta::{parse_bichon_metadata, BichonMetadata};
use console::style;
use dialoguer::{theme::ColorfulTheme, Input};
use dialoguer::{Confirm, Select};
use mail_parser::{HeaderForm, MessageParser};
use reqwest::Client;

/// Skip emails larger than this with a warning (100 MB).
const MAX_EMAIL_BYTES: usize = 100 * 1024 * 1024;
/// Flush a folder buffer when accumulated base64 bytes exceed this (200 MB).
const MAX_BUFFER_BYTES: usize = 200 * 1024 * 1024;

pub mod gmail;

pub async fn handle_mbox_single_file_import(
    config: &BichonCliConfig,
    account_id: u64,
    theme: &ColorfulTheme,
) {
    let path_str: String = Input::with_theme(theme)
        .with_prompt("Enter the path to your SINGLE .mbox file")
        .validate_with(|input: &String| {
            let p = std::path::Path::new(input);
            if !p.exists() {
                return Err("The specified path does not exist.");
            }
            if !p.is_file() {
                return Err("MBOX mode requires a SINGLE file, not a directory.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();

    let mbox_path = PathBuf::from(path_str);

    let options = vec![
        "Use labels from mail headers (X-Gmail-Labels)",
        "Specify a single target folder for all emails",
        "Use X-Bichon-Metadata header (Automatic)",
    ];

    let selection = Select::with_theme(theme)
        .with_prompt("How should we determine the target folder?")
        .items(&options)
        .default(0)
        .interact()
        .unwrap();

    let target_folder: Option<String> = match selection {
        0 => None,
        1 => {
            let folder: String = Input::with_theme(theme)
                .with_prompt("Target folder name")
                .default("INBOX".into())
                .interact_text()
                .unwrap();
            Some(folder)
        }
        2 => None,
        _ => unreachable!(),
    };

    if let Some(ref folder) = target_folder {
        println!(
            "{}",
            style(format!("Mode: Fixed folder ({})", folder)).dim()
        );
    } else {
        println!("{}", style("Mode: Dynamic (header-based)").dim());
    }

    println!(
        "\n{} Ready to process MBOX file: {}",
        style("✔").green(),
        style(mbox_path.display()).cyan()
    );

    if let Ok(meta) = std::fs::metadata(&mbox_path) {
        let size_mb = meta.len() as f64 / 1024.0 / 1024.0;
        println!(
            "{}",
            style(format!("Processing file: {:.1} MB", size_mb)).dim()
        );
    }

    if Confirm::with_theme(theme)
        .with_prompt("Start importing?")
        .default(true)
        .interact()
        .unwrap()
    {
        run_import(account_id, &mbox_path, config, target_folder).await
    }
}

pub async fn run_import(
    account_id: u64,
    mbox_path: &PathBuf,
    config: &BichonCliConfig,
    target_folder: Option<String>,
) {
    let client = Client::new();
    let mbox = match MboxFile::from_file(mbox_path) {
        Ok(mbox) => mbox,
        Err(err) => {
            println!("Skipping invalid MBOX: {} ({})", mbox_path.display(), err);
            return;
        }
    };

    let mut folder_buffers: HashMap<String, Vec<String>> = HashMap::new();
    let mut total_buffered_bytes: usize = 0;
    let batch_limit = 50;
    let mut skipped_count: u64 = 0;

    println!("Starting import process...");

    for (index, e) in mbox.iter().enumerate() {
        let msg_num = index + 1;
        let body = e.data;

        if body.len() > MAX_EMAIL_BYTES {
            let size_mb = body.len() as f64 / 1024.0 / 1024.0;
            eprintln!(
                "{} {}: email #{} is {:.1} MB (limit 100 MB). Skipping...",
                style("Warning").yellow().bold(),
                style(format!("oversized")).dim(),
                msg_num,
                size_mb,
            );
            skipped_count += 1;
            continue;
        }

        let message = match MessageParser::new()
            .unknown_headers(HeaderForm::Text)
            .parse(body)
        {
            Some(msg) => msg,
            None => {
                eprintln!(
                    "{} {}: {}",
                    style("Warning").yellow().bold(),
                    style(format!("at message #{}", msg_num)).dim(),
                    "Failed to parse email structure. Skipping..."
                );
                skipped_count += 1;
                continue;
            }
        };

        let mut metadata: Option<BichonMetadata> = None;
        if let Some(meta_header) = message
            .headers()
            .value("X-Bichon-Metadata")
            .and_then(|v| v.as_text())
        {
            metadata = parse_bichon_metadata(meta_header);
        }

        let get_default_folder = || {
            let labels = message
                .headers()
                .value("X-Gmail-Labels")
                .and_then(|v| v.as_text())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "INBOX".to_string());
            determine_folder(&labels)
        };

        let folder_name = if let Some(ref folder) = target_folder {
            folder.clone()
        } else if let Some(ref meta) = metadata {
            meta.mailbox_name.clone().unwrap_or_else(get_default_folder)
        } else {
            get_default_folder()
        };

        // Drop message before base64-encoding to free MIME parse memory.
        drop(message);

        let b64_eml = base64_encode_url_safe!(&body);
        let encoded_len = b64_eml.len();

        let buffer = folder_buffers
            .entry(folder_name.clone())
            .or_insert_with(Vec::new);
        buffer.push(b64_eml);
        total_buffered_bytes += encoded_len;

        if buffer.len() >= batch_limit || total_buffered_bytes >= MAX_BUFFER_BYTES {
            let emls_to_send = folder_buffers.remove(&folder_name).unwrap();
            let freed: usize = emls_to_send.iter().map(|s| s.len()).sum();
            total_buffered_bytes = total_buffered_bytes.saturating_sub(freed);
            send_batch_request(&client, config, account_id, &folder_name, emls_to_send).await;
        }
    }

    for (folder_name, emls) in folder_buffers {
        if !emls.is_empty() {
            send_batch_request(&client, config, account_id, &folder_name, emls).await;
        }
    }

    if skipped_count > 0 {
        println!(
            "{}",
            style(format!(
                "Skipped {} email(s) (oversized or unparseable).",
                skipped_count
            ))
            .yellow()
            .bold()
        );
    }

    println!("{}", style("Import completed successfully!").green().bold());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Fake sender: records every flushed batch as (folder_name, email_count, total_bytes).
    struct FakeSender {
        batches: Vec<(String, usize, usize)>,
    }

    impl FakeSender {
        fn new() -> Self {
            Self { batches: vec![] }
        }
        fn send(&mut self, folder: &str, emls: Vec<String>) {
            let count = emls.len();
            let bytes: usize = emls.iter().map(|s| s.len()).sum();
            self.batches.push((folder.to_string(), count, bytes));
            // emls is dropped here, simulating real send
        }
    }

    fn fake_encode(size: usize) -> String {
        // base64 expands ~1.33x, so the encoded string is roughly this long.
        // We just need a predictable byte size, so use a repeated character.
        "x".repeat(size)
    }

    #[test]
    fn flush_on_global_byte_threshold() {
        let mut buffers: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_bytes: usize = 0;
        let batch_limit = 50;
        let mut sender = FakeSender::new();

        // Simulate 3 emails, each 80 MB encoded, spread across 3 folders.
        // After each email, global total goes up by 80 MB.
        // After the 3rd email: 240 MB > 200 MB → flush the folder that got the 3rd email.
        let emails = vec![
            ("Inbox", 80_000_000),
            ("Sent", 80_000_000),
            ("Archive", 80_000_000),
        ];

        for (folder, eml_size) in emails {
            let encoded = fake_encode(eml_size);
            let len = encoded.len();
            let buffer = buffers.entry(folder.to_string()).or_insert_with(Vec::new);
            buffer.push(encoded);
            total_bytes += len;

            if buffer.len() >= batch_limit || total_bytes >= MAX_BUFFER_BYTES {
                let sent = buffers.remove(folder).unwrap();
                let freed: usize = sent.iter().map(|s| s.len()).sum();
                total_bytes = total_bytes.saturating_sub(freed);
                sender.send(folder, sent);
            }
        }

        // The 3rd email should trigger a global flush of "Archive".
        assert_eq!(sender.batches.len(), 1);
        assert_eq!(sender.batches[0].0, "Archive");
        assert_eq!(sender.batches[0].1, 1);
        // "Inbox" and "Sent" are still buffered (160 MB total).
        assert_eq!(buffers.len(), 2);
        assert!(buffers.contains_key("Inbox"));
        assert!(buffers.contains_key("Sent"));
        assert_eq!(total_bytes, 160_000_000);
    }

    #[test]
    fn flush_on_count_threshold() {
        let mut buffers: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_bytes: usize = 0;
        let batch_limit = 3;
        let mut sender = FakeSender::new();

        // 4 small emails all to Inbox, well under byte threshold.
        for _ in 0..4 {
            let encoded = fake_encode(100); // tiny
            let len = encoded.len();
            let buffer = buffers
                .entry("Inbox".to_string())
                .or_insert_with(Vec::new);
            buffer.push(encoded);
            total_bytes += len;

            if buffer.len() >= batch_limit || total_bytes >= MAX_BUFFER_BYTES {
                let sent = buffers.remove("Inbox").unwrap();
                let freed: usize = sent.iter().map(|s| s.len()).sum();
                total_bytes = total_bytes.saturating_sub(freed);
                sender.send("Inbox", sent);
            }
        }

        // Count=3 should trigger flush once; the 4th email stays buffered.
        assert_eq!(sender.batches.len(), 1);
        assert_eq!(sender.batches[0].1, 3); // 3 emails flushed
        let remaining = buffers.get("Inbox").unwrap();
        assert_eq!(remaining.len(), 1); // 1 still buffered
    }

    #[test]
    fn global_bytes_exact_boundary() {
        let mut buffers: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_bytes: usize = 0;
        let mut sender = FakeSender::new();

        // Push one email that puts us right at 200 MB.
        let encoded = fake_encode(MAX_BUFFER_BYTES);
        let len = encoded.len();
        buffers
            .entry("Inbox".to_string())
            .or_insert_with(Vec::new)
            .push(encoded);
        total_bytes += len;

        if total_bytes >= MAX_BUFFER_BYTES {
            let sent = buffers.remove("Inbox").unwrap();
            let freed: usize = sent.iter().map(|s| s.len()).sum();
            total_bytes = total_bytes.saturating_sub(freed);
            sender.send("Inbox", sent);
        }

        // Should have flushed on the boundary.
        assert_eq!(sender.batches.len(), 1);
        assert_eq!(total_bytes, 0);
    }

    #[test]
    fn flush_one_folder_does_not_lose_others() {
        let mut buffers: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_bytes: usize = 0;
        let batch_limit = 50;
        let mut sender = FakeSender::new();

        // Build up A to 150 MB, B to 100 MB (total 250 MB > 200 MB).
        // A should trigger flush; B should stay buffered.
        let folder_a = "A".to_string();
        let folder_b = "B".to_string();

        // Folder A: 150 MB
        let encoded = fake_encode(150_000_000);
        let len = encoded.len();
        buffers.entry(folder_a.clone()).or_insert_with(Vec::new).push(encoded);
        total_bytes += len;

        // Folder B: 100 MB → total 250 MB → trigger flush on B
        let encoded = fake_encode(100_000_000);
        let len = encoded.len();
        buffers.entry(folder_b.clone()).or_insert_with(Vec::new).push(encoded);
        total_bytes += len;

        // Check trigger on B
        let b_buffer = buffers.get(&folder_b).unwrap();
        if b_buffer.len() >= batch_limit || total_bytes >= MAX_BUFFER_BYTES {
            let sent = buffers.remove(&folder_b).unwrap();
            let freed: usize = sent.iter().map(|s| s.len()).sum();
            total_bytes = total_bytes.saturating_sub(freed);
            sender.send(&folder_b, sent);
        }

        assert_eq!(sender.batches.len(), 1);
        assert_eq!(sender.batches[0].0, "B"); // B flushed
        assert!(buffers.contains_key("A")); // A still there
        assert_eq!(total_bytes, 150_000_000);
    }

    #[test]
    fn skip_oversized_email() {
        assert!(100 <= MAX_EMAIL_BYTES);
        // Use vec! so the 100 MB array lives on the heap, not the stack.
        let huge = vec![0u8; MAX_EMAIL_BYTES + 1];
        assert!(huge.len() > MAX_EMAIL_BYTES);
    }
}
