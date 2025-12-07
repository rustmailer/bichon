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


use crate::modules::account::state::AccountRunningState;
use crate::modules::cache::imap::mailbox::MailBox;
use crate::modules::cache::imap::sync::flow::{generate_uid_sequence_hashset, BATCH_SIZE};
use crate::modules::envelope::extractor::extract_envelope;
use crate::modules::error::code::ErrorCode;
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use crate::modules::indexer::schema::SchemaTools;
use crate::modules::settings::dir::DATA_DIR_MANAGER;
use crate::modules::{error::BichonResult, imap::manager::ImapConnectionManager};
use crate::raise_error;
use tokio::fs;
use async_imap::types::{Mailbox, Name};
use bb8::Pool;
use futures::TryStreamExt;
use std::collections::HashSet;
use tantivy::doc;
use tracing::info;

const BODY_FETCH_COMMAND: &str = "(UID INTERNALDATE RFC822.SIZE BODY.PEEK[])";

pub struct ImapExecutor {
    pool: Pool<ImapConnectionManager>,
}

impl ImapExecutor {
    pub fn new(pool: Pool<ImapConnectionManager>) -> Self {
        Self { pool }
    }

    pub async fn list_all_mailboxes(&self) -> BichonResult<Vec<Name>> {
        let mut session = self.pool.get().await?;
        let list = session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
        let result = list
            .try_collect::<Vec<Name>>()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
        Ok(result)
    }

    pub async fn examine_mailbox(&self, mailbox_name: &str) -> BichonResult<Mailbox> {
        let mut session = self.pool.get().await?;
        session
            .examine(mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))
    }

    pub async fn uid_search(&self, mailbox_name: &str, query: &str) -> BichonResult<HashSet<u32>> {
        let mut session = self.pool.get().await?;
        session
            .examine(mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
        let result = session
            .uid_search(query)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
        Ok(result)
    }

    pub async fn fetch_new_mail(
        &self,
        account_id: u64,
        mailbox: &MailBox,
        start_uid: u64,
    ) -> BichonResult<()> {
        assert!(start_uid > 0, "start_uid must be greater than 0");
        let uid_list = self
            .uid_search(
                &mailbox.encoded_name(),
                format!("UID {start_uid}:*").as_str(),
            )
            .await?;

        let len = uid_list.len();
        if len == 0 {
            return Ok(());
        }
        info!(
            "[account {}][mailbox {}] {} envelopes need to be fetched",
            account_id, mailbox.name, len
        );

        let mut uid_vec: Vec<u32> = uid_list.into_iter().collect();
        uid_vec.sort();
        let uid_batches = generate_uid_sequence_hashset(uid_vec, BATCH_SIZE as usize, false);

        let too_many = len as u32 > 10 * BATCH_SIZE;
        if too_many {
            AccountRunningState::set_initial_current_syncing_folder(
                account_id,
                mailbox.name.clone(),
                uid_batches.len() as u32,
            )
            .await?;
        }

        for (index, batch) in uid_batches.into_iter().enumerate() {
            if too_many {
                AccountRunningState::set_current_sync_batch_number(
                    account_id,
                    mailbox.name.clone(),
                    (index + 1) as u32,
                )
                .await?;
            }
            self.uid_batch_retrieve_emails(account_id, mailbox.id, &batch, &mailbox.encoded_name())
                .await?;
        }
        Ok(())
    }

    pub async fn batch_retrieve_emails(
        &self,
        account_id: u64,
        mailbox_id: u64,
        page: u64,
        page_size: u64,
        encoded_mailbox_name: &str,
        desc: bool,
    ) -> BichonResult<usize> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");

        let mut session = self.pool.get().await?;
        let total = session
            .examine(encoded_mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?
            .exists as u64;

        if total == 0 {
            return Ok(0);
        }

        let (start, end) = if desc {
            // Fetch messages starting from the newest (descending order)
            let end = total.saturating_sub((page - 1) * page_size);
            if end == 0 {
                return Ok(0);
            }
            // Calculate start as end - page_size + 1 to avoid off-by-one errors
            let start = end.saturating_sub(page_size - 1).max(1);
            (start, end)
        } else {
            // Fetch messages starting from the oldest (ascending order)
            let start = (page - 1) * page_size + 1;
            if start > total {
                return Ok(0);
            }
            // Calculate end, capped by the total number of messages
            let end = (start + page_size - 1).min(total);
            (start, end)
        };

        let sequence_set = format!("{}:{}", start, end);
        info!(
            "Fetching mailbox '{}' messages: sequence {} (page {}, page_size {}, desc={})",
            encoded_mailbox_name, sequence_set, page, page_size, desc
        );

        let mut stream = session
            .fetch(sequence_set.as_str(), BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;

        let mut count = 0;
        let fields = SchemaTools::eml_fields();
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?
        {
            let envelope = extract_envelope(&fetch, account_id, mailbox_id)?;
            ENVELOPE_INDEX_MANAGER
                .add_document(envelope.id, envelope.to_document(mailbox_id)?)
                .await;
            let body = fetch.body().ok_or_else(|| {
                raise_error!("missing a body".into(), ErrorCode::ImapUnexpectedResult)
            })?;

            // Store EML to disk
            let eml_path = DATA_DIR_MANAGER.eml_dir.join(format!("{}", envelope.id));
            fs::write(&eml_path, body).await?;

            EML_INDEX_MANAGER.add_document( envelope.id, doc!(fields.f_id => envelope.id, fields.f_account_id => account_id, fields.f_mailbox_id => mailbox_id, fields.f_mbox_id => 0u64, fields.f_mbox_offset => 0u64, fields.f_mbox_len => body.len() as u64)).await;
            count += 1;
        }
        Ok(count)
    }

    pub async fn uid_batch_retrieve_emails(
        &self,
        account_id: u64,
        mailbox_id: u64,
        uid_set: &str,
        encoded_mailbox_name: &str,
    ) -> BichonResult<()> {
        let mut session = self.pool.get().await?;
        session
            .examine(encoded_mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;

        let mut stream = session
            .uid_fetch(uid_set, BODY_FETCH_COMMAND)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?;
        let fields = SchemaTools::eml_fields();
        while let Some(fetch) = stream
            .try_next()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))?
        {
            let envelope = extract_envelope(&fetch, account_id, mailbox_id)?;
            ENVELOPE_INDEX_MANAGER
                .add_document(envelope.id, envelope.to_document(mailbox_id)?)
                .await;
            let body = fetch.body().ok_or_else(|| {
                raise_error!("missing a body".into(), ErrorCode::ImapUnexpectedResult)
            })?;
            EML_INDEX_MANAGER.add_document( envelope.id, doc!(fields.f_id => envelope.id, fields.f_account_id => account_id, fields.f_mailbox_id => mailbox_id, fields.f_mbox_id => 0u64, fields.f_mbox_offset => 0u64, fields.f_mbox_len => body.len() as u64)).await;
        }
        Ok(())
    }
}
