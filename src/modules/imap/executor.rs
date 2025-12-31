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

use crate::modules::account::migration::AccountModel;
use crate::modules::account::state::AccountRunningState;
use crate::modules::cache::imap::mailbox::MailBox;
use crate::modules::cache::imap::sync::flow::{generate_uid_sequence_hashset, DEFAULT_BATCH_SIZE};
use crate::modules::envelope::extractor::extract_envelope;
use crate::modules::error::code::ErrorCode;
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use crate::modules::indexer::schema::SchemaTools;
use crate::modules::{error::BichonResult, imap::manager::ImapConnectionManager};
use crate::raise_error;
use async_imap::types::{Mailbox, Name};
use bb8::{Pool, RunError};
use futures::TryStreamExt;
use std::collections::HashSet;
use tantivy::doc;
use tracing::info;

const BODY_FETCH_COMMAND: &str = "(UID INTERNALDATE RFC822.SIZE BODY.PEEK[])";

pub struct ImapExecutor {
    account_id: u64,
    pool: Pool<ImapConnectionManager>,
}

impl ImapExecutor {
    pub fn new(account_id: u64, pool: Pool<ImapConnectionManager>) -> Self {
        Self { account_id, pool }
    }

    pub async fn list_all_mailboxes(&self) -> BichonResult<Vec<Name>> {
        let mut session = self.get_connection().await?;
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
        let mut session = self.get_connection().await?;
        session
            .examine(mailbox_name)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))
    }

    pub async fn uid_search(&self, mailbox_name: &str, query: &str) -> BichonResult<HashSet<u32>> {
        let mut session = self.get_connection().await?;
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

    pub async fn append(
        &self,
        mailbox_name: impl AsRef<str>,
        flags: Option<&str>,
        internaldate: Option<&str>,
        content: impl AsRef<[u8]>,
    ) -> BichonResult<()> {
        let mut session = self.get_connection().await?;
        session
            .append(mailbox_name, flags, internaldate, content)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed))
    }

    pub async fn fetch_new_mail(
        &self,
        account: &AccountModel,
        mailbox: &MailBox,
        start_uid: u64,
        before: Option<&str>,
    ) -> BichonResult<()> {
        assert!(start_uid > 0, "start_uid must be greater than 0");

        let query = match before {
            Some(date) => format!("UID {start_uid}:* BEFORE {date}"),
            None => format!("UID {start_uid}:*"),
        };

        let uid_list = self.uid_search(&mailbox.encoded_name(), &query).await?;

        let len = uid_list.len();
        if len == 0 {
            return Ok(());
        }
        info!(
            "[account {}][mailbox {}] {} envelopes need to be fetched",
            account.id, mailbox.name, len
        );

        let mut uid_vec: Vec<u32> = uid_list.into_iter().collect();
        uid_vec.sort();
        let uid_batches = generate_uid_sequence_hashset(
            uid_vec,
            account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE) as usize,
            false,
        );

        let too_many = len as u32 > 5 * account.sync_batch_size.unwrap_or(DEFAULT_BATCH_SIZE);
        if too_many {
            AccountRunningState::set_initial_current_syncing_folder(
                account.id,
                mailbox.name.clone(),
                uid_batches.len() as u32,
            )
            .await?;
        }

        for (index, batch) in uid_batches.into_iter().enumerate() {
            if too_many {
                AccountRunningState::set_current_sync_batch_number(
                    account.id,
                    mailbox.name.clone(),
                    (index + 1) as u32,
                )
                .await?;
            }
            self.uid_batch_retrieve_emails(account.id, mailbox.id, &batch, &mailbox.encoded_name())
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

        let mut session = self.get_connection().await?;
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
            EML_INDEX_MANAGER.add_document( envelope.id, doc!(fields.f_id => envelope.id, fields.f_account_id => account_id, fields.f_mailbox_id => mailbox_id, fields.f_eml => body)).await;
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
        let mut session = self.get_connection().await?;
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
            EML_INDEX_MANAGER.add_document( envelope.id, doc!(fields.f_id => envelope.id, fields.f_account_id => account_id, fields.f_mailbox_id => mailbox_id, fields.f_eml => body)).await;
        }
        Ok(())
    }

    async fn get_connection(
        &self,
    ) -> BichonResult<bb8::PooledConnection<'_, ImapConnectionManager>> {
        match self.pool.get().await {
            Ok(connection) => Ok(connection),
            Err(e) => match e {
                RunError::User(e) => Err(e),
                RunError::TimedOut => {
                    let state = self.pool.state();
                    tracing::warn!(
                        "{}: connections={}, idle={}, \
                        get_started={}, get_direct={}, get_waited={}, get_timed_out={}, \
                        wait_time_ms={}, created={}, closed_broken={}, closed_invalid={}, \
                        closed_lifetime={}, closed_idle={}",
                        self.account_id,
                        state.connections,
                        state.idle_connections,
                        state.statistics.get_started,
                        state.statistics.get_direct,
                        state.statistics.get_waited,
                        state.statistics.get_timed_out,
                        state.statistics.get_wait_time.as_millis(),
                        state.statistics.connections_created,
                        state.statistics.connections_closed_broken,
                        state.statistics.connections_closed_invalid,
                        state.statistics.connections_closed_max_lifetime,
                        state.statistics.connections_closed_idle_timeout,
                    );
                    return Err(raise_error!(
                        "Timed out while attempting to acquire a connection from the pool".into(),
                        ErrorCode::ConnectionPoolTimeout
                    ));
                }
            },
        }
    }
}
