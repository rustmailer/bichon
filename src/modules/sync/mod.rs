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

use crate::modules::{
    account::migration::{AccountModel, AccountType},
    cache::imap::{
        find_intersecting_mailboxes, find_missing_mailboxes,
        mailbox::MailBox,
        sync::{execute_imap_sync, flow::{generate_uid_sequence_hashset, DEFAULT_BATCH_SIZE}},
    },
    error::{code::ErrorCode, BichonResult},
    imap::executor::ImapExecutor,
    indexer::manager::{ENVELOPE_INDEX_MANAGER, EML_INDEX_MANAGER},
    mailbox::list::request_imap_all_mailbox_list,
};
use crate::raise_error;
use base64::Engine;
use futures::TryStreamExt;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Decode RFC 2047 encoded-word subjects (e.g. `=?utf-8?B?...?=`) to readable text.
fn decode_rfc2047(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    // Try to decode =?charset?B?base64?= or =?charset?Q?quoted?= patterns
    let mut result = String::new();
    let mut remaining = s.as_ref();
    while let Some(start) = remaining.find("=?") {
        result.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        // Find charset?encoding?data?=
        let parts: Vec<&str> = after_start.splitn(4, '?').collect();
        if parts.len() >= 3 && parts[2].ends_with("?=") || (parts.len() == 4 && parts[3].starts_with('=')) {
            let encoding = parts[1];
            let data = if parts.len() == 4 {
                // parts[2] is the data, parts[3] starts with '='
                parts[2]
            } else {
                parts[2].trim_end_matches("?=")
            };
            let decoded = match encoding.to_uppercase().as_str() {
                "B" => base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok()),
                "Q" => {
                    // Quoted-printable: _ = space, =XX = hex byte
                    let qp: Vec<u8> = data.as_bytes().iter().copied().fold(
                        (Vec::new(), false, 0u8),
                        |(mut acc, in_hex, hex_byte), b| {
                            if in_hex {
                                if hex_byte == 0 {
                                    return (acc, true, b);
                                }
                                let hex_str = [hex_byte, b];
                                if let Ok(val) = u8::from_str_radix(
                                    &String::from_utf8_lossy(&hex_str), 16,
                                ) {
                                    acc.push(val);
                                }
                                (acc, false, 0)
                            } else if b == b'=' {
                                (acc, true, 0)
                            } else if b == b'_' {
                                acc.push(b' ');
                                (acc, false, 0)
                            } else {
                                acc.push(b);
                                (acc, false, 0)
                            }
                        },
                    ).0;
                    String::from_utf8(qp).ok()
                }
                _ => None,
            };
            if let Some(text) = decoded {
                result.push_str(&text);
            } else {
                // Fallback: keep original
                result.push_str(&remaining[start..start + 2]);
                remaining = after_start;
                continue;
            }
            // Skip past the closing ?=
            let end_marker = if parts.len() == 4 {
                // start + 2 + charset? + encoding? + data?=
                start + 2 + parts[0].len() + 1 + parts[1].len() + 1 + parts[2].len() + 2
            } else {
                start + 2 + parts[0].len() + 1 + parts[1].len() + 1 + parts[2].len()
            };
            remaining = if end_marker <= remaining.len() {
                &remaining[end_marker..]
            } else {
                ""
            };
        } else {
            result.push_str("=?");
            remaining = after_start;
        }
    }
    result.push_str(remaining);
    if result.is_empty() { s.to_string() } else { result }
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct MissingMessageInfo {
    pub uid: u32,
    pub date: String,
    pub message_id: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct SyncFolderResult {
    pub server_count: usize,
    pub local_count_before: usize,
    pub local_count_after: u64,
    pub missing_count: usize,
    pub fetched: usize,
    pub new_messages: i64,
    pub dedup_count: i64,
    pub missing_messages: Vec<MissingMessageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct FetchEmlRequest {
    pub uids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct RawEmailExport {
    pub uid: u32,
    pub eml_base64: String,
}

/// Fetch raw EML content from IMAP for the given UIDs (without storing).
pub async fn fetch_raw_emails(
    account_id: u64,
    mailbox_id: u64,
    uids: Vec<u32>,
) -> BichonResult<Vec<RawEmailExport>> {
    let account = AccountModel::check_account_exists(account_id).await?;
    if !matches!(account.account_type, AccountType::IMAP) {
        return Err(raise_error!(
            "Only IMAP accounts supported.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let local_mailbox = MailBox::get(mailbox_id).await?;
    if local_mailbox.account_id != account_id {
        return Err(raise_error!(
            "Mailbox does not belong to this account.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    let encoded_name = local_mailbox.encoded_name();
    let mut session = ImapExecutor::create_connection(account_id).await?;
    session
        .examine(&encoded_name)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;

    let uid_set: String = uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
    let mut results = Vec::new();
    {
        let mut stream = session
            .uid_fetch(&uid_set, "BODY.PEEK[]")
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;

        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?
        {
            let uid = fetch.uid.unwrap_or(0);
            if let Some(body) = fetch.body() {
                results.push(RawEmailExport {
                    uid,
                    eml_base64: base64::engine::general_purpose::STANDARD.encode(body),
                });
            }
        }
    }

    session.logout().await.ok();
    Ok(results)
}

/// Trigger a full sync for an IMAP account (on demand).
pub async fn sync_account_on_demand(account_id: u64) -> BichonResult<()> {
    let account = AccountModel::check_account_exists(account_id).await?;
    if !matches!(account.account_type, AccountType::IMAP) {
        return Err(raise_error!(
            "Sync is only supported for IMAP accounts.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if !account.enabled {
        return Err(raise_error!(
            "Account is disabled. Enable it before syncing.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    execute_imap_sync(&account).await
}

/// Sync a single folder/mailbox for an IMAP account.
pub async fn sync_single_folder(account_id: u64, mailbox_id: u64) -> BichonResult<SyncFolderResult> {
    let account = AccountModel::check_account_exists(account_id).await?;
    if !matches!(account.account_type, AccountType::IMAP) {
        return Err(raise_error!(
            "Sync is only supported for IMAP accounts.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let local_mailbox = MailBox::get(mailbox_id).await?;
    if local_mailbox.account_id != account_id {
        return Err(raise_error!(
            "Mailbox does not belong to this account.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    perform_single_folder_sync(&account, &local_mailbox).await
}

/// Connect to IMAP, compare server UIDs with local UIDs, fetch missing ones, flush indexes.
async fn perform_single_folder_sync(
    account: &AccountModel,
    local_mailbox: &MailBox,
) -> BichonResult<SyncFolderResult> {
    let encoded_name = local_mailbox.encoded_name();

    // Get all UIDs on the server
    let mut session = ImapExecutor::create_connection(account.id).await?;
    let server_uids = ImapExecutor::uid_search(&mut session, &encoded_name, "UID 1:*").await?;

    // Get all UIDs in the local index
    let local_uids = ENVELOPE_INDEX_MANAGER.get_all_uids(account.id, local_mailbox.id)?;

    // Compute missing UIDs (on server but not in local)
    let mut missing: Vec<u32> = server_uids.difference(&local_uids).copied().collect();
    missing.sort();

    let before_count = local_uids.len();
    info!(
        "[account {}][mailbox {}] server={} UIDs, local={} UIDs, missing={}",
        account.id, local_mailbox.name, server_uids.len(), before_count, missing.len()
    );

    let mut missing_messages = Vec::new();
    let mut after_count = before_count as u64;
    let mut new_messages: i64 = 0;
    let mut dedup_count: i64 = 0;

    if !missing.is_empty() {
        // Fetch ENVELOPE info for missing UIDs
        let uid_set: String = missing.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");

        {
            let mut envelope_stream = session
                .uid_fetch(&uid_set, "(UID ENVELOPE INTERNALDATE)")
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
            while let Some(fetch) = envelope_stream
                .try_next()
                .await
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?
            {
                let uid = fetch.uid.unwrap_or(0);
                if let Some(env) = fetch.envelope() {
                    let subject = env.subject.as_ref()
                        .map(|s| decode_rfc2047(s))
                        .unwrap_or_default();
                    let message_id = env.message_id.as_ref()
                        .map(|s| {
                            let raw = String::from_utf8_lossy(s).to_string();
                            raw.trim_matches(|c| c == '<' || c == '>').to_string()
                        })
                        .unwrap_or_default();
                    let date = env.date.as_ref()
                        .map(|s| String::from_utf8_lossy(s).to_string())
                        .unwrap_or_default();
                    // Fall back to INTERNALDATE when envelope Date is missing
                    let date = if date.is_empty() {
                        fetch.internal_date()
                            .map(|dt| dt.to_rfc2822())
                            .unwrap_or_default()
                    } else {
                        date
                    };
                    missing_messages.push(MissingMessageInfo {
                        uid,
                        date,
                        message_id,
                        subject,
                    });
                }
            }
        }

        let batch_size = account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize;
        let uid_batches = generate_uid_sequence_hashset(missing.clone(), batch_size, false);
        for batch in &uid_batches {
            ImapExecutor::uid_batch_retrieve_emails(
                &mut session,
                account.id,
                local_mailbox.id,
                batch,
                &encoded_name,
            )
            .await?;
        }

        // Flush both indexes so counts are accurate immediately
        ENVELOPE_INDEX_MANAGER.flush().await;
        EML_INDEX_MANAGER.flush().await;

        after_count = ENVELOPE_INDEX_MANAGER
            .count_messages_in_mailbox(account.id, local_mailbox.id)
            .await?;
        new_messages = after_count as i64 - before_count as i64;
        dedup_count = missing.len() as i64 - new_messages;
    }

    session.logout().await.ok();

    // Update mailbox metadata with latest server state
    let mut session2 = ImapExecutor::create_connection(account.id).await?;
    let mx = session2
        .examine(&encoded_name)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
    session2.logout().await.ok();

    let mut updated_mailbox = local_mailbox.clone();
    updated_mailbox.exists = mx.exists;
    updated_mailbox.unseen = mx.unseen;
    updated_mailbox.uid_next = mx.uid_next;
    updated_mailbox.uid_validity = mx.uid_validity;
    MailBox::batch_upsert(&[updated_mailbox]).await?;

    Ok(SyncFolderResult {
        server_count: server_uids.len(),
        local_count_before: before_count,
        local_count_after: after_count,
        missing_count: missing.len(),
        fetched: missing.len(),
        new_messages,
        dedup_count,
        missing_messages,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct MailboxStatusEntry {
    pub mailbox_id: u64,
    pub mailbox_name: String,
    /// Message count reported by the IMAP server (from last sync)
    pub server_count: u32,
    /// Actual number of messages stored in the local index
    pub local_count: u64,
    /// Whether this mailbox is configured for syncing
    pub syncing: bool,
}

/// Return local mailboxes with their indexed message counts.
/// This is a fast, offline-only operation (no IMAP connection).
pub async fn get_mailbox_status(account_id: u64) -> BichonResult<Vec<MailboxStatusEntry>> {
    let account = AccountModel::check_account_exists(account_id).await?;
    let sync_folders = account.sync_folders.unwrap_or_default();
    let mailboxes = MailBox::list_all(account_id).await?;
    let mut result = Vec::with_capacity(mailboxes.len());
    for mb in &mailboxes {
        let local_count = ENVELOPE_INDEX_MANAGER
            .count_messages_in_mailbox(account_id, mb.id)
            .await?;
        result.push(MailboxStatusEntry {
            mailbox_id: mb.id,
            mailbox_name: mb.name.clone(),
            server_count: mb.exists,
            local_count,
            syncing: sync_folders.contains(&mb.name),
        });
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct MailboxVerifyResult {
    pub mailbox_id: u64,
    pub mailbox_name: String,
    /// Number of messages on the remote IMAP server
    pub remote_count: u32,
    /// Number of messages in the local index
    pub local_count: u64,
    /// Number of missing messages (remote - local), 0 if local >= remote
    pub missing_count: u64,
    /// Whether the mailbox is fully synced
    pub is_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct SyncVerifyResult {
    pub account_id: u64,
    /// Folders present on the server but not locally
    pub missing_folders: Vec<String>,
    /// Per-mailbox verification results
    pub mailboxes: Vec<MailboxVerifyResult>,
    /// Whether all mailboxes are fully synced and no folders are missing
    pub is_complete: bool,
}

/// Verify sync completeness by comparing local state with the IMAP server.
pub async fn verify_sync_completeness(account_id: u64) -> BichonResult<SyncVerifyResult> {
    let account = AccountModel::check_account_exists(account_id).await?;
    if !matches!(account.account_type, AccountType::IMAP) {
        return Err(raise_error!(
            "Verify is only supported for IMAP accounts.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    let remote_mailboxes = request_imap_all_mailbox_list(account_id).await?;
    let local_mailboxes = MailBox::list_all(account_id).await?;

    let missing_folders: Vec<String> = find_missing_mailboxes(&local_mailboxes, &remote_mailboxes)
        .into_iter()
        .map(|m| m.name)
        .collect();

    let intersecting = find_intersecting_mailboxes(&local_mailboxes, &remote_mailboxes);

    let mut mailbox_results = Vec::with_capacity(intersecting.len());

    for (local_mb, remote_mb) in &intersecting {
        let local_count = ENVELOPE_INDEX_MANAGER
            .count_messages_in_mailbox(account_id, local_mb.id)
            .await?;
        let remote_count = remote_mb.exists;
        let missing = (remote_count as u64).saturating_sub(local_count);
        mailbox_results.push(MailboxVerifyResult {
            mailbox_id: local_mb.id,
            mailbox_name: local_mb.name.clone(),
            remote_count,
            local_count,
            missing_count: missing,
            is_complete: missing == 0,
        });
    }

    let is_complete = missing_folders.is_empty() && mailbox_results.iter().all(|r| r.is_complete);

    info!(
        "Sync verification for account {}: complete={}, missing_folders={}, mailboxes_checked={}",
        account_id,
        is_complete,
        missing_folders.len(),
        mailbox_results.len()
    );

    Ok(SyncVerifyResult {
        account_id,
        missing_folders,
        mailboxes: mailbox_results,
        is_complete,
    })
}
