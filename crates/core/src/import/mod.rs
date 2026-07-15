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


//use poem_openapi::Object;
pub mod history;
pub mod reader;
pub mod pst;
pub use history::ImportHistory;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    sync::RwLock,
};

use crate::{
    base64_decode_url_safe,
    {
        account::migration::{AccountModel, AccountType},
        cache::imap::mailbox::{Attribute, AttributeEnum, MailBox},
        envelope::extractor::extract_envelope_from_eml,
        error::{BichonResult, code::ErrorCode},
        settings::dir::DATA_DIR_MANAGER,
        utils::create_hash,
    },
    raise_error,
};

/// Maximum byte size of an individual email message after splitting (100 MB).
const MAX_SINGLE_EML_BYTES: usize = 100 * 1024 * 1024;

/// Max file size accepted via the web upload endpoint.
pub const MAX_WEB_EML_BYTES: usize = 100 * 1024 * 1024;       // 100 MB
pub const MAX_WEB_MBOX_BYTES: usize = 1024 * 1024 * 1024;     // 1 GB

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchEmlRequest {
    pub account_id: u64,
    pub mail_folder: String,
    /// A list of emails in base64-encoded format. Each element represents one .eml file.
    pub emls: Vec<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct FailedItemDetail {
    /// The index (0-based) of the failed item.
    pub index: usize,
    /// The error message that caused the import to fail.
    pub error_message: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchEmlResult {
    /// Total number of emails processed.
    pub total: usize,
    /// Number of emails successfully imported.
    pub success: usize,
    /// Number of duplicate emails skipped (content hash already existed).
    pub duplicates: usize,
    /// Number of emails failed to import.
    pub failed: usize,
    /// A list of details for failed imports.
    pub failed_details: Vec<FailedItemDetail>,
}

pub struct ImportEmls;

impl ImportEmls {
    pub async fn do_import(mut request: BatchEmlRequest) -> BichonResult<BatchEmlResult> {
        let account = AccountModel::check_account_exists(request.account_id)?;
        
        if !account.enabled {
            return Err(raise_error!("The account is disabled and cannot be used for this operation.".into(), ErrorCode::InvalidParameter));
        }

        let mailbox_id = match account.account_type {
            AccountType::IMAP => {
                let all_mailboxes = MailBox::list_all(account.id)?;
                let mailbox = all_mailboxes.into_iter().find(|m| m.name == request.mail_folder);
                
                match mailbox {
                    Some(mailbox) => mailbox.id,
                    None => return Err(raise_error!(
                        format!("Mail folder '{}' not found for account ID {}. The target folder must exist before importing.", 
                                request.mail_folder, 
                                request.account_id).into(),
                        ErrorCode::ResourceNotFound
                    )),
                }
            },
            AccountType::NoSync => {
                let mailbox = MailBox {
                    id: create_hash(request.account_id, &request.mail_folder),
                    account_id: request.account_id,
                    name: request.mail_folder.clone(),
                    delimiter: Some("/".to_string()),
                    attributes: vec![Attribute {
                        attr: AttributeEnum::Extension,
                        extension: Some("CreatedByBichon".into()),
                    }],
                    exists: 0,
                    unseen: None,
                    uid_next: None,
                    uid_validity: None,
                    highest_uid: None,
                };
                let mailbox_id = mailbox.id;
                // Upsert the mailbox, creating it if it doesn't exist
                MailBox::batch_upsert(&[mailbox])?;
                mailbox_id
            },
        };

        let account_id = account.id;
        let mut success_count = 0;
        let mut failed_details: Vec<FailedItemDetail> = Vec::new(); // Store failure details

        let total = request.emls.len();
        let mut index: usize = 0;
        while let Some(eml_base64) = request.emls.pop() {
            let decoded = match base64_decode_url_safe!(eml_base64.as_bytes()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let error_msg =
                        format!("Failed to decode base64 EML at index {}: {:?}", index, e);
                    tracing::error!("{}", error_msg);
                    failed_details.push(FailedItemDetail {
                        index,
                        error_message: error_msg,
                    });
                    index += 1;
                    continue;
                }
            };
            // eml_base64 string dropped here — frees base64 memory before parsing

            if decoded.len() > MAX_SINGLE_EML_BYTES {
                let size_mb = decoded.len() as f64 / 1024.0 / 1024.0;
                let error_msg = format!(
                    "Email at index {} is {:.1} MB (limit 50 MB). Skipping.",
                    index, size_mb,
                );
                tracing::warn!("{}", error_msg);
                failed_details.push(FailedItemDetail {
                    index,
                    error_message: error_msg,
                });
                index += 1;
                continue;
            }

            match extract_envelope_from_eml(&decoded, account_id, mailbox_id).await {
                Ok(_) => {
                    success_count += 1;
                },
                Err(e) => {
                    let error_msg = format!(
                        "Failed to extract envelope from EML at index {}: {:?}",
                        index, e
                    );
                    tracing::error!("{}", error_msg);
                    failed_details.push(FailedItemDetail {
                        index,
                        error_message: error_msg,
                    });
                    index += 1;
                    continue;
                }
            };
            index += 1;
        }

        let failed_count = failed_details.len();

        Ok(BatchEmlResult {
            total,
            success: success_count,
            duplicates: 0,
            failed: failed_count,
            failed_details, // Return the list of failure details
        })
    }
}

// ── File upload import ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum ImportStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ImportProgress {
    pub import_id: String,
    pub status: ImportStatus,
    pub format: String,
    pub total: usize,
    pub success: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub failed_details: Vec<FailedItemDetail>,
}

static PROGRESS_STORE: std::sync::LazyLock<RwLock<HashMap<String, ImportProgress>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn get_import_progress(import_id: &str) -> Option<ImportProgress> {
    PROGRESS_STORE.read().ok()?.get(import_id).cloned()
}

pub fn update_progress(import_id: &str, progress: ImportProgress) {
    if let Ok(mut store) = PROGRESS_STORE.write() {
        store.insert(import_id.to_string(), progress);
    }
}

/// Check free disk space (in bytes) on the temp directory's filesystem.
pub fn check_temp_disk_space() -> BichonResult<u64> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let temp_path = &DATA_DIR_MANAGER.temp_dir;
    // Use the canonical path so we can match mount points
    let canonical = std::fs::canonicalize(temp_path).unwrap_or_else(|_| temp_path.clone());
    for disk in disks.list() {
        if canonical.starts_with(disk.mount_point()) {
            return Ok(disk.available_space());
        }
    }
    // Fallback: if we can't find the mount point, report plenty of space
    Ok(u64::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Eml,
    Mbox,
    Pst,
}

pub fn detect_format(bytes: &[u8], file_name: &str) -> Option<FileFormat> {
    // PST files start with OLE2 compound document magic bytes
    if bytes.len() >= 8 && &bytes[..8] == b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1" {
        return Some(FileFormat::Pst);
    }

    // MBOX files start with "From " (note the trailing space after From)
    if bytes.starts_with(b"From ") {
        // Double-check: look for a valid date after the first "From " line
        // MBOX format: "From sender@host DayOfWeek Mon DD HH:MM:SS YYYY"
        if let Some(first_newline) = bytes.iter().position(|&b| b == b'\n') {
            let from_line = std::str::from_utf8(&bytes[..first_newline]).unwrap_or("");
            let parts: Vec<&str> = from_line.split_whitespace().collect();
            if parts.len() >= 7 {
                return Some(FileFormat::Mbox);
            }
        }
    }
    // EML: starts with a header line or "Return-Path:", "Received:", "From:", "Date:", etc.
    // Or check extension
    if bytes.starts_with(b"Return-Path:")
        || bytes.starts_with(b"Received:")
        || bytes.starts_with(b"Date:")
        || bytes.starts_with(b"From:")
        || bytes.starts_with(b"Subject:")
        || bytes.starts_with(b"To:")
        || bytes.starts_with(b"Message-ID:")
    {
        return Some(FileFormat::Eml);
    }
    // Fallback: check file extension
    let lower = file_name.to_lowercase();
    if lower.ends_with(".eml") {
        Some(FileFormat::Eml)
    } else if lower.ends_with(".mbox") {
        Some(FileFormat::Mbox)
    } else if lower.ends_with(".pst") {
        Some(FileFormat::Pst)
    } else {
        None
    }
}

/// Check whether `bytes` looks like a text file by inspecting the first chunk.
/// Returns `true` if it passes, `false` if it appears to be binary (video, executable, etc.).
///
/// Email files (EML/MBOX) are text-based with printable ASCII, whitespace, and
/// optional UTF-8. Binary files like video contain null bytes and high ratios of
/// non-printable control characters.
pub fn detect_text_file(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(8192);
    if check_len == 0 {
        return false;
    }
    let sample = &bytes[..check_len];

    // Null bytes are a strong binary indicator
    if sample.contains(&0x00) {
        return false;
    }

    let mut printable = 0usize;
    let mut total = 0usize;

    let mut i = 0;
    while i < sample.len() {
        total += 1;
        let b = sample[i];

        if b.is_ascii_graphic() || b.is_ascii_whitespace() {
            // Printable ASCII + whitespace (space, tab, CR, LF)
            printable += 1;
        } else if b == 0x1b {
            // ESC — common in terminal sequences, rare in email
            // Count as printable to avoid false positives
            printable += 1;
        } else if b >= 0x80 {
            // UTF-8 continuation or multi-byte lead byte — allow.
            // Check that we have a valid UTF-8 sequence ahead.
            let seq_len = match b {
                b if b & 0xE0 == 0xC0 => 2,
                b if b & 0xF0 == 0xE0 => 3,
                b if b & 0xF8 == 0xF0 => 4,
                _ => 0,
            };
            if seq_len > 0 && i + seq_len <= sample.len() {
                let valid = std::str::from_utf8(&sample[i..i + seq_len]).is_ok();
                if valid {
                    printable += 1;
                    i += 1; // lead byte counted, continuations counted in loop
                }
                // if invalid, don't count as printable
            }
            // standalone continuation byte — not printable
        }
        // Other control characters (0x01-0x1F except whitespace/Esc) are not counted as printable

        i += 1;
    }

    // Require at least 90% printable characters
    printable as f64 / total as f64 >= 0.90
}

/// Validate that the target account exists, is enabled, and is NoSync type.
fn validate_import_account(account_id: u64) -> BichonResult<AccountModel> {
    let account = AccountModel::check_account_exists(account_id)?;
    if !account.enabled {
        return Err(raise_error!(
            "The account is disabled.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    Ok(account)
}

/// Resolve or create a mailbox/folder for the given account.
pub(super) fn resolve_mailbox(account: &AccountModel, folder: &str) -> BichonResult<u64> {
    match account.account_type {
        AccountType::IMAP => {
            // Shouldn't reach here (validated above), but handle gracefully
            let all_mailboxes = MailBox::list_all(account.id)?;
            let mailbox = all_mailboxes.into_iter().find(|m| m.name == folder);
            match mailbox {
                Some(m) => Ok(m.id),
                None => Err(raise_error!(
                    format!("Mail folder '{}' not found.", folder).into(),
                    ErrorCode::ResourceNotFound
                )),
            }
        }
        AccountType::NoSync => {
            let mailbox = MailBox {
                id: create_hash(account.id, folder),
                account_id: account.id,
                name: folder.to_string(),
                delimiter: Some("/".to_string()),
                attributes: vec![Attribute {
                    attr: AttributeEnum::Extension,
                    extension: Some("CreatedByBichon".into()),
                }],
                exists: 0,
                unseen: None,
                uid_next: None,
                uid_validity: None,
                highest_uid: None,
            };
            let mailbox_id = mailbox.id;
            MailBox::batch_upsert(&[mailbox])?;
            Ok(mailbox_id)
        }
    }
}

/// Resolve or create a mailbox for a given account_id and folder name.
/// Used by PST import to create per-folder mailboxes.
pub fn resolve_mailbox_by_account_id(account_id: u64, folder: &str) -> BichonResult<u64> {
    let account = AccountModel::check_account_exists(account_id)?;
    resolve_mailbox(&account, folder)
}

/// Process an uploaded file (EML or MBOX) and import into the given account/folder.
/// This runs synchronously and should be spawned on a background thread.
///
/// For MBOX files, the file is memory-mapped via `memmap2` and messages are yielded
/// one at a time — the full file is never loaded into RAM. Individual messages
/// exceeding `MAX_SINGLE_EML_BYTES` (100 MB) are skipped.
pub fn process_uploaded_file(
    import_id: &str,
    file_path: &Path,
    file_name: &str,
    account_id: u64,
    folder: &str,
    user_id: u64,
) {
    let account = match validate_import_account(account_id) {
        Ok(a) => a,
        Err(e) => {
            let progress = ImportProgress {
                import_id: import_id.to_string(),
                status: ImportStatus::Failed,
                format: "unknown".to_string(),
                total: 0,
                success: 0,
                duplicates: 0,
                failed: 0,
                failed_details: vec![FailedItemDetail {
                    index: 0,
                    error_message: format!("Account validation failed: {:?}", e),
                }],
            };
            update_progress(import_id, progress.clone());
            history::save_import_history(user_id, account_id, folder, &progress);
            return;
        }
    };

    let mailbox_id = match resolve_mailbox(&account, folder) {
        Ok(id) => id,
        Err(e) => {
            let progress = ImportProgress {
                import_id: import_id.to_string(),
                status: ImportStatus::Failed,
                format: "unknown".to_string(),
                total: 0,
                success: 0,
                duplicates: 0,
                failed: 0,
                failed_details: vec![FailedItemDetail {
                    index: 0,
                    error_message: format!("Mailbox resolution failed: {:?}", e),
                }],
            };
            update_progress(import_id, progress.clone());
            history::save_import_history(user_id, account_id, folder, &progress);
            return;
        }
    };

    // Read a small prefix for format detection
    let format = match detect_format_from_file(file_path, file_name) {
        Ok(f) => f,
        Err(e) => {
            let progress = ImportProgress {
                import_id: import_id.to_string(),
                status: ImportStatus::Failed,
                format: "unknown".to_string(),
                total: 0,
                success: 0,
                duplicates: 0,
                failed: 0,
                failed_details: vec![FailedItemDetail {
                    index: 0,
                    error_message: format!("{:?}", e),
                }],
            };
            update_progress(import_id, progress.clone());
            history::save_import_history(user_id, account_id, folder, &progress);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    match format {
        FileFormat::Eml => process_eml_file(import_id, file_path, account_id, mailbox_id, user_id, folder),
        FileFormat::Mbox => process_mbox_file(import_id, file_path, account_id, mailbox_id, user_id, folder),
        FileFormat::Pst => process_pst_upload(import_id, file_path, account_id, mailbox_id, user_id, folder),
    }
}

/// Detect format from a file by reading only the first few KB.
fn detect_format_from_file(file_path: &Path, file_name: &str) -> BichonResult<FileFormat> {
    use std::io::Read;
    let mut file = std::fs::File::open(file_path).map_err(|e| {
        raise_error!(format!("Failed to open file: {}", e), ErrorCode::InternalError)
    })?;
    let mut buf = vec![0u8; 8192];
    let n = file.read(&mut buf).unwrap_or(0);
    buf.truncate(n);

    detect_format(&buf, file_name).ok_or_else(|| {
        raise_error!(
            "Unknown file format. Supported: .eml, .mbox, .pst".into(),
            ErrorCode::InvalidParameter
        )
    })
}

/// Process a single EML file. The file is at most `MAX_WEB_EML_BYTES` (100 MB),
/// so reading it entirely is safe.
fn process_eml_file(
    import_id: &str,
    file_path: &Path,
    account_id: u64,
    mailbox_id: u64,
    user_id: u64,
    folder: &str,
) {
    let file_bytes = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(e) => {
            fail_progress(import_id, "eml", &format!("Failed to read file: {}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    let total = 1;
    update_progress(import_id, ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Processing,
        format: "eml".to_string(),
        total,
        success: 0,
        duplicates: 0,
        failed: 0,
        failed_details: vec![],
    });

    let (success_count, failed_details) = process_single_eml(&file_bytes, 0, account_id, mailbox_id);

    // Clean up
    let _ = std::fs::remove_file(file_path);

    let final_progress = ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Completed,
        format: "eml".to_string(),
        total,
        success: success_count,
        duplicates: 0,
        failed: failed_details.len(),
        failed_details,
    };
    history::save_import_history(user_id, account_id, folder, &final_progress);
    update_progress(import_id, final_progress);
}

/// Process an MBOX file using memory-mapped I/O. Messages are yielded one at a
/// time by `MboxReader` — the full file is never loaded into RAM.
fn process_mbox_file(
    import_id: &str,
    file_path: &Path,
    account_id: u64,
    mailbox_id: u64,
    user_id: u64,
    folder: &str,
) {
    let mbox = match reader::MboxFile::from_file(file_path) {
        Ok(m) => m,
        Err(e) => {
            fail_progress(import_id, "mbox", &format!("Failed to open MBOX file: {}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    // First pass: count total messages (MboxReader is lazy, so this is O(n) but cheap)
    let total = mbox.iter().count();

    update_progress(import_id, ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Processing,
        format: "mbox".to_string(),
        total,
        success: 0,
        duplicates: 0,
        failed: 0,
        failed_details: vec![],
    });

    let mut success_count = 0usize;
    let mut failed_details: Vec<FailedItemDetail> = Vec::new();

    for (index, entry) in mbox.iter().enumerate() {
        let eml_bytes = entry.data;

        if eml_bytes.len() > MAX_SINGLE_EML_BYTES {
            let size_mb = eml_bytes.len() as f64 / 1024.0 / 1024.0;
            failed_details.push(FailedItemDetail {
                index,
                error_message: format!(
                    "Email at index {} is {:.1} MB (limit {} MB). Skipping.",
                    index,
                    size_mb,
                    MAX_SINGLE_EML_BYTES / 1024 / 1024
                ),
            });
            continue;
        }

        match futures::executor::block_on(extract_envelope_from_eml(eml_bytes, account_id, mailbox_id)) {
            Ok(_) => {
                success_count += 1;
            }
            Err(e) => {
                failed_details.push(FailedItemDetail {
                    index,
                    error_message: format!("{:?}", e),
                });
            }
        };

        // Update progress every 100 items
        if index % 100 == 0 || index == total - 1 {
            update_progress(import_id, ImportProgress {
                import_id: import_id.to_string(),
                status: ImportStatus::Processing,
                format: "mbox".to_string(),
                total,
                success: success_count,
                duplicates: 0,
                failed: failed_details.len(),
                failed_details: failed_details.clone(),
            });
        }
    }

    // Clean up temp file (drop the mmap first — MboxFile owns it)
    drop(mbox);
    let _ = std::fs::remove_file(file_path);

    let final_progress = ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Completed,
        format: "mbox".to_string(),
        total,
        success: success_count,
        duplicates: 0,
        failed: failed_details.len(),
        failed_details,
    };
    history::save_import_history(user_id, account_id, folder, &final_progress);
    update_progress(import_id, final_progress);
}

/// Process a single EML byte slice and return (success_count, failed_details).
fn process_single_eml(
    eml_bytes: &[u8],
    index: usize,
    account_id: u64,
    mailbox_id: u64,
) -> (usize, Vec<FailedItemDetail>) {
    if eml_bytes.len() > MAX_SINGLE_EML_BYTES {
        let size_mb = eml_bytes.len() as f64 / 1024.0 / 1024.0;
        return (0, vec![FailedItemDetail {
            index,
            error_message: format!(
                "Email is {:.1} MB (limit {} MB). Skipping.",
                size_mb,
                MAX_SINGLE_EML_BYTES / 1024 / 1024
            ),
        }]);
    }

    match futures::executor::block_on(extract_envelope_from_eml(eml_bytes, account_id, mailbox_id)) {
        Ok(_) => (1, vec![]),
        Err(e) => (0, vec![FailedItemDetail {
            index,
            error_message: format!("{:?}", e),
        }]),
    }
}

/// Process a PST file uploaded via the web UI.
/// Two-pass approach: count messages first, then process with periodic progress updates.
fn process_pst_upload(
    import_id: &str,
    file_path: &Path,
    account_id: u64,
    _mailbox_id: u64,  // ignored; PST creates its own mailboxes per folder
    user_id: u64,
    folder: &str,
) {
    // Pass 1: count total messages
    let total = match pst::count_pst_messages(file_path) {
        Ok(n) => n,
        Err(e) => {
            fail_progress(import_id, "pst", &format!("{:?}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    update_progress(import_id, ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Processing,
        format: "pst".to_string(),
        total,
        success: 0,
        duplicates: 0,
        failed: 0,
        failed_details: vec![],
    });

    // Pass 2: process messages with progress updates
    let mut success_count: usize = 0;
    let mut failed_details: Vec<FailedItemDetail> = Vec::new();
    let mut index: usize = 0;

    let pst_store = match outlook_pst::open_store(file_path) {
        Ok(s) => s,
        Err(e) => {
            fail_progress(import_id, "pst", &format!("{:?}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    let ipm_sub_tree = match pst_store.properties().ipm_sub_tree_entry_id() {
        Ok(id) => id,
        Err(e) => {
            fail_progress(import_id, "pst", &format!("{:?}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    let ipm_subtree_folder = match pst_store.open_folder(&ipm_sub_tree) {
        Ok(f) => f,
        Err(e) => {
            fail_progress(import_id, "pst", &format!("{:?}", e), user_id, account_id, folder);
            let _ = std::fs::remove_file(file_path);
            return;
        }
    };

    // Progress callback: update progress every 50 messages
    let import_id = import_id.to_string();
    let format_str = "pst".to_string();
    pst::process_folder_with_progress(
        &ipm_subtree_folder,
        "",  // parent_path starts empty
        account_id,
        total,  // pass pre-counted total for accurate progress
        &mut success_count,
        &mut failed_details,
        &mut index,
        &|processed, actual_failed| {
            update_progress(&import_id, ImportProgress {
                import_id: import_id.clone(),
                status: ImportStatus::Processing,
                format: format_str.clone(),
                total,
                success: processed - actual_failed,
                duplicates: 0,
                failed: actual_failed,
                failed_details: vec![],
            });
        },
    );

    // Clean up temp file
    let _ = std::fs::remove_file(file_path);

    let final_progress = ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Completed,
        format: "pst".to_string(),
        total,
        success: success_count,
        duplicates: 0,
        failed: failed_details.len(),
        failed_details,
    };
    history::save_import_history(user_id, account_id, folder, &final_progress);
    update_progress(&import_id, final_progress);
}

/// Record a fatal failure and save history.
fn fail_progress(
    import_id: &str,
    format: &str,
    message: &str,
    user_id: u64,
    account_id: u64,
    folder: &str,
) {
    let progress = ImportProgress {
        import_id: import_id.to_string(),
        status: ImportStatus::Failed,
        format: format.to_string(),
        total: 0,
        success: 0,
        duplicates: 0,
        failed: 0,
        failed_details: vec![FailedItemDetail {
            index: 0,
            error_message: message.to_string(),
        }],
    };
    update_progress(import_id, progress.clone());
    history::save_import_history(user_id, account_id, folder, &progress);
}
