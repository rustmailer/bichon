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

use crate::{
    modules::{
        account::{migration::AccountModel, state::AccountRunningState},
        cache::{
            imap::{
                find_intersecting_mailboxes, find_missing_mailboxes,
                mailbox::MailBox,
                sync::rebuild::{
                    rebuild_mailbox_cache, rebuild_mailbox_cache_by_date,
                    DEFAULT_MAX_CONCURRENT_PER_ACCOUNT,
                },
            },
            SEMAPHORE,
        },
        error::{code::ErrorCode, BichonError, BichonResult},
        imap::executor::ImapExecutor,
        indexer::manager::ENVELOPE_INDEX_MANAGER,
    },
    raise_error,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

pub const DEFAULT_BATCH_SIZE: u32 = 50;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchDirection {
    Since,
    Before,
}

pub async fn fetch_and_save_by_date(
    account: &AccountModel,
    date: &str,
    mailbox: &MailBox,
    direction: FetchDirection,
) -> BichonResult<usize> {
    let account_id = account.id;
    let mut session = ImapExecutor::create_connection(account_id).await?;

    let search_criteria = match direction {
        FetchDirection::Since => format!("SINCE {date}"),
        FetchDirection::Before => format!("BEFORE {date}"),
    };

    let uid_list =
        ImapExecutor::uid_search(&mut session, &mailbox.encoded_name(), &search_criteria).await?;

    let len = uid_list.len();
    if len == 0 {
        return Ok(0);
    }

    let folder_limit = account.folder_limit;
    // sort small -> bigger
    let mut uid_vec: Vec<u32> = uid_list.into_iter().collect();
    uid_vec.sort();

    if let Some(limit) = folder_limit {
        let limit = limit.max(100) as usize;
        if len > limit {
            uid_vec = match direction {
                FetchDirection::Since => uid_vec.split_off(len - limit),
                FetchDirection::Before => {
                    uid_vec.truncate(limit);
                    uid_vec
                }
            };
        }
    }

    // let semaphore = Arc::new(Semaphore::new(5));

    let uid_batches = generate_uid_sequence_hashset(
        uid_vec,
        account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize,
        false,
    );
    AccountRunningState::set_initial_current_syncing_folder(
        account_id,
        mailbox.name.clone(),
        uid_batches.len() as u32,
    )
    .await?;
    for (index, batch) in uid_batches.into_iter().enumerate() {
        AccountRunningState::set_current_sync_batch_number(
            account_id,
            mailbox.name.clone(),
            (index + 1) as u32,
        )
        .await?;

        // Fetch metadata for the current batch of UIDs
        ImapExecutor::uid_batch_retrieve_emails(
            &mut session,
            account_id,
            mailbox.id,
            &batch,
            &mailbox.encoded_name(),
        )
        .await?;
    }
    session.logout().await.ok();
    Ok(len)
}

pub async fn fetch_and_save_full_mailbox(
    account: &AccountModel,
    mailbox: &MailBox,
    total: u32,
) -> BichonResult<usize> {
    let mailbox_id = mailbox.id;
    let account_id = account.id;
    let folder_limit = account.folder_limit;
    let total_to_fetch = match folder_limit {
        Some(limit) if limit < total => total.min(limit.max(100)),
        _ => total,
    };
    let page_size = if let Some(limit) = folder_limit {
        limit
            .max(100)
            .min(account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE))
    } else {
        account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE)
    };

    let total_batches = total_to_fetch.div_ceil(page_size);
    let desc = folder_limit.is_some();

    let mut inserted_count = 0;

    AccountRunningState::set_initial_current_syncing_folder(
        account_id,
        mailbox.name.clone(),
        total_batches,
    )
    .await?;
    info!(
        "Starting full mailbox sync for '{}', total={}, limit={:?}, batches={}, desc={}",
        mailbox.name, total, folder_limit, total_batches, desc
    );

    let mut session = ImapExecutor::create_connection(account_id).await?;

    for page in 1..=total_batches {
        AccountRunningState::set_current_sync_batch_number(account_id, mailbox.name.clone(), page)
            .await?;
        let count = ImapExecutor::batch_retrieve_emails(
            &mut session,
            account_id,
            mailbox_id,
            page as u64,
            page_size as u64,
            &mailbox.encoded_name(),
            desc,
        )
        .await?;
        inserted_count += count;
        info!(
            "Batch insertion completed for mailbox: {}, current page: {}, inserted count: {}",
            &mailbox.name, page, count
        );
    }
    session.logout().await.ok();
    Ok(inserted_count)
}

/// # Example
///
/// ```rust
/// use std::collections::HashSet;
///
/// let mut uids = HashSet::new();
/// uids.extend([1, 2, 3, 5, 6, 7, 9, 10, 11, 15]);
///
/// let chunks = generate_uid_sequence_hashset(uids, 6, false);
/// assert_eq!(chunks, vec![
///     "1:3,5:7".to_string(),
///     "9:11,15".to_string()
/// ]);
/// ```
///
/// This splits the UIDs into chunks of 6, compresses each chunk into ranges,
/// and returns a vector like: `["1:3,5:7", "9:11,15"]`.
///
pub fn generate_uid_sequence_hashset(
    unique_nums: Vec<u32>,
    chunk_size: usize,
    desc: bool,
) -> Vec<String> {
    assert!(!unique_nums.is_empty());
    // let mut nums: Vec<u32> = unique_nums.into_iter().collect();
    // nums.sort();
    let mut nums = unique_nums;
    if desc {
        nums.reverse();
    }

    let mut result = Vec::new();

    for chunk in nums.chunks(chunk_size) {
        let compressed = compress_uid_list(chunk.to_vec());
        result.push(compressed);
    }

    result
}

pub fn compress_uid_list(nums: Vec<u32>) -> String {
    if nums.is_empty() {
        return String::new();
    }

    let mut sorted_nums = nums;
    sorted_nums.sort();

    let mut result = Vec::new();
    let mut current_range_start = sorted_nums[0];
    let mut current_range_end = sorted_nums[0];

    for &n in sorted_nums.iter().skip(1) {
        if n == current_range_end + 1 {
            current_range_end = n;
        } else {
            if current_range_start == current_range_end {
                result.push(current_range_start.to_string());
            } else {
                result.push(format!("{}:{}", current_range_start, current_range_end));
            }
            current_range_start = n;
            current_range_end = n;
        }
    }

    if current_range_start == current_range_end {
        result.push(current_range_start.to_string());
    } else {
        result.push(format!("{}:{}", current_range_start, current_range_end));
    }

    result.join(",")
}

pub async fn reconcile_mailboxes(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    local_mailboxes: &[MailBox],
) -> BichonResult<()> {
    let start_time = Instant::now();
    let existing_mailboxes = find_intersecting_mailboxes(local_mailboxes, remote_mailboxes);
    let account_id = account.id;
    if !existing_mailboxes.is_empty() {
        let mut mailboxes_to_update = Vec::with_capacity(existing_mailboxes.len());
        for (local_mailbox, remote_mailbox) in &existing_mailboxes {
            if local_mailbox.uid_validity != remote_mailbox.uid_validity {
                if remote_mailbox.uid_validity.is_none() {
                    warn!(
                        "Account {}: Mailbox '{}' has invalid uid_validity (None). Skipping sync for this mailbox.",
                        account_id, local_mailbox.name
                    );
                    continue;
                }
                info!(
                    "Account {}: Mailbox '{}' detected with changed uid_validity (local: {:#?}, remote: {:#?}). \
                    The mailbox data may be invalid, resetting its envelopes and rebuilding the cache.",
                    account_id, local_mailbox.name, &local_mailbox.uid_validity, &remote_mailbox.uid_validity
                );

                match &account.date_since {
                    Some(date_since) => {
                        rebuild_mailbox_cache_by_date(
                            account,
                            local_mailbox.id,
                            &date_since.since_date()?,
                            remote_mailbox,
                            FetchDirection::Since,
                        )
                        .await?;
                    }
                    None => match &account.date_before {
                        Some(r) => {
                            rebuild_mailbox_cache_by_date(
                                account,
                                local_mailbox.id,
                                &r.calculate_date()?,
                                remote_mailbox,
                                FetchDirection::Before,
                            )
                            .await?;
                        }
                        None => {
                            rebuild_mailbox_cache(account, local_mailbox, remote_mailbox).await?
                        }
                    },
                }
            } else {
                perform_incremental_sync(account, local_mailbox, remote_mailbox).await?;
            }
            if let Some(state) = AccountRunningState::get(account.id).await? {
                if !state.is_initial_sync_completed {
                    AccountRunningState::set_folder_initial_sync_completed(
                        account_id,
                        local_mailbox.name.clone(),
                    )
                    .await?;
                }
            }

            mailboxes_to_update.push(remote_mailbox.clone());
        }
        //The metadata of this mailbox must only be updated after a successful synchronization;
        //otherwise, it may cause synchronization errors and result in missing emails in the local sync results.
        MailBox::batch_upsert(&mailboxes_to_update).await?;
    }

    debug!(
        "Checked mailbox folders for account ID: {}. Compared local and server folders to identify changes. Elapsed time: {} seconds",
        account.id,
        start_time.elapsed().as_secs()
    );

    let missing_mailboxes = find_missing_mailboxes(local_mailboxes, remote_mailboxes);
    if !missing_mailboxes.is_empty() {
        MailBox::batch_insert(&missing_mailboxes).await?;
        let mut handles = Vec::new();

        for mailbox in &missing_mailboxes {
            if mailbox.exists > 0 {
                let account = account.clone();
                let mailbox = mailbox.clone();

                let local_semaphore = Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_PER_ACCOUNT));

                let global_permit = match SEMAPHORE.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        error!(
                            "Failed to acquire global semaphore permit for account {} mailbox '{}': {:#?}",
                            account.id, &mailbox.name, err
                        );
                        continue;
                    }
                };

                let local_permit = match local_semaphore.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(err) => {
                        error!(
                            "Failed to acquire local semaphore permit for account {} mailbox '{}': {:#?}",
                            account.id, &mailbox.name, err
                        );
                        drop(global_permit);
                        continue;
                    }
                };

                let handle: tokio::task::JoinHandle<Result<(), BichonError>> =
                    tokio::spawn(async move {
                        let _global_permit = global_permit;
                        let _local_permit = local_permit;

                        match &account.date_since {
                            Some(date_since) => {
                                rebuild_mailbox_cache_by_date(
                                    &account,
                                    mailbox.id,
                                    &date_since.since_date()?,
                                    &mailbox,
                                    FetchDirection::Since,
                                )
                                .await
                            }
                            None => match &account.date_before {
                                Some(r) => {
                                    rebuild_mailbox_cache_by_date(
                                        &account,
                                        mailbox.id,
                                        &r.calculate_date()?,
                                        &mailbox,
                                        FetchDirection::Before,
                                    )
                                    .await
                                }
                                None => rebuild_mailbox_cache(&account, &mailbox, &mailbox).await,
                            },
                        }
                    });
                handles.push(handle);
            }
        }

        for task in handles {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(err),
                Err(e) => return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)),
            }
        }
    }
    Ok(())
}

//only check new emails and sync
pub async fn perform_incremental_sync(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
) -> BichonResult<()> {
    if remote_mailbox.exists > 0 {
        let local_max_uid = ENVELOPE_INDEX_MANAGER
            .get_max_uid(account.id, local_mailbox.id)
            .await?;
        match local_max_uid {
            Some(max_uid) => {
                let mut session = ImapExecutor::create_connection(account.id).await?;
                let before_date = account
                    .date_before
                    .as_ref()
                    .map(|r| r.calculate_date())
                    .transpose()?;

                ImapExecutor::fetch_new_mail(
                    &mut session,
                    account,
                    local_mailbox,
                    max_uid + 1,
                    before_date.as_deref(),
                )
                .await?;
                session.logout().await.ok();
            }
            None => {
                info!(
                    "No maximum UID found in index for mailbox, assuming local cache is missing."
                );

                match &account.date_since {
                    Some(date_since) => {
                        fetch_and_save_by_date(
                            account,
                            date_since.since_date()?.as_str(),
                            remote_mailbox,
                            FetchDirection::Since,
                        )
                        .await?;
                    }
                    None => {
                        fetch_and_save_full_mailbox(account, remote_mailbox, remote_mailbox.exists)
                            .await?;
                    }
                }
            }
        }
    }

    Ok(())
}
