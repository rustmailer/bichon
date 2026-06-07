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

use crate::{
    database::{delete_impl, find_impl, manager::DB_MANAGER, update_impl, upsert_impl, MemDbModel},
    error::BichonResult,
    utc_now,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum DownloadStatus {
    Running,
    Success,
    Failed,
    #[default]
    Cancelled,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum TriggerType {
    Manual,
    #[default]
    Scheduled,
    /// Triggered by an IMAP IDLE notification (RFC 2177). The download
    /// task treats it like `Scheduled`, but downstream observability
    /// can distinguish "real-time" syncs from the 10-second poll cycle.
    Idle,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum FolderStatus {
    #[default]
    Pending,
    Downloading,
    Success,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct FolderProgress {
    pub folder_name: String,
    pub planned: u64,
    pub current: u64,
    pub status: FolderStatus,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct DownloadSession {
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub status: DownloadStatus,
    pub message: Option<String>,
    pub trigger: TriggerType,
    pub folder_details: BTreeMap<String, FolderProgress>,
    pub current_folder: Option<String>,
    pub errors: Vec<AccountError>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct DownloadState {
    pub account_id: u64,
    pub active_session: Option<DownloadSession>,
    pub history: Vec<DownloadSession>,
    pub last_trigger_at: i64,
    pub last_finished_at: Option<i64>,
}

impl MemDbModel for DownloadState {
    fn collection() -> &'static str {
        "download_states"
    }
    fn key(&self) -> String {
        self.account_id.to_string()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct AccountError {
    pub error: String,
    pub at: i64,
}

impl DownloadState {
    pub fn empty(account_id: u64) -> Self {
        DownloadState {
            account_id,
            ..Default::default()
        }
    }

    pub async fn init(account_id: u64) -> BichonResult<()> {
        let now = utc_now!();
        let state = DownloadState {
            account_id,
            last_trigger_at: now,
            active_session: Some(DownloadSession {
                start_time: now,
                status: DownloadStatus::Running,
                trigger: TriggerType::Scheduled,
                ..Default::default()
            }),
            history: Default::default(),
            last_finished_at: Default::default(),
        };
        upsert_impl(DB_MANAGER.db(), state)
    }

    pub fn get(account_id: u64) -> BichonResult<Option<DownloadState>> {
        find_impl::<DownloadState>(DB_MANAGER.db(), &account_id.to_string())
    }

    pub fn start_new_session(account_id: u64, trigger: TriggerType) -> BichonResult<()> {
        Self::update_state(account_id, move |current| {
            let mut updated = current.clone();
            updated.last_trigger_at = utc_now!();

            if let Some(mut old_session) = updated.active_session.take() {
                if old_session.status == DownloadStatus::Running {
                    old_session.status = DownloadStatus::Cancelled;
                    old_session.end_time = Some(utc_now!());
                    old_session.message = Some("Interrupted by a new download session.".into());
                }
                updated.history.push(old_session);
                if updated.history.len() > 30 {
                    updated.history.remove(0);
                }
            }

            let new_session = DownloadSession {
                start_time: utc_now!(),
                status: DownloadStatus::Running,
                trigger,
                ..Default::default()
            };

            updated.active_session = Some(new_session);
            Ok(updated)
        })
    }

    pub fn update_session_status(
        account_id: u64,
        status: DownloadStatus,
        message: Option<String>,
    ) -> BichonResult<()> {
        Self::update_state(account_id, move |current| {
            let mut updated = current.clone();
            if let Some(mut session) = updated.active_session.take() {
                session.status = status.clone();
                if message.is_some() {
                    session.message = message;
                }
                if status == DownloadStatus::Running {
                    updated.active_session = Some(session);
                } else {
                    let now = utc_now!();
                    session.end_time = Some(now);
                    updated.last_finished_at = Some(now);
                    updated.history.push(session);
                    let to_remove = updated.history.len().saturating_sub(10);
                    if to_remove > 0 {
                        updated.history.drain(0..to_remove);
                    }
                }
            }
            Ok(updated)
        })
    }

    pub fn update_folder_progress(
        account_id: u64,
        folder_name: String,
        planned: u64,
        current: u64,
        status: FolderStatus,
        message: Option<String>,
    ) -> BichonResult<()> {
        Self::update_state(account_id, move |state| {
            let mut updated = state.clone();
            if let Some(ref mut session) = updated.active_session {
                session.current_folder = Some(folder_name.clone());

                let progress =
                    session
                        .folder_details
                        .entry(folder_name.clone())
                        .or_insert(FolderProgress {
                            folder_name,
                            ..Default::default()
                        });

                progress.planned = planned;
                progress.current = current;
                progress.status = status;
                progress.message = message;
            }
            Ok(updated)
        })
    }

    pub fn init_folder_details(account_id: u64, folders: Vec<String>) -> BichonResult<()> {
        Self::update_state(account_id, move |state| {
            let mut updated = state.clone();
            if let Some(ref mut session) = updated.active_session {
                for name in folders {
                    session.folder_details.insert(
                        name.clone(),
                        FolderProgress {
                            folder_name: name,
                            planned: 0,
                            current: 0,
                            status: FolderStatus::Pending,
                            message: None,
                        },
                    );
                }
            }
            Ok(updated)
        })
    }

    pub fn append_session_error(account_id: u64, error: String) -> BichonResult<()> {
        Self::update_state(account_id, move |current| {
            let mut updated = current.clone();
            let new_error = AccountError {
                error,
                at: utc_now!(),
            };
            let target = updated
                .active_session
                .as_mut()
                .or_else(|| updated.history.last_mut());
            if let Some(session) = target {
                session.errors.push(new_error);
                let to_remove = session.errors.len().saturating_sub(30);
                if to_remove > 0 {
                    session.errors.drain(0..to_remove);
                }
            }
            Ok(updated)
        })
    }

    fn update_state(
        account_id: u64,
        updater: impl FnOnce(DownloadState) -> BichonResult<DownloadState> + Send + 'static,
    ) -> BichonResult<()> {
        if Self::get(account_id)?.is_some() {
            update_impl(DB_MANAGER.db(), &account_id.to_string(), updater)?;
        }
        Ok(())
    }

    pub fn delete(account_id: u64) -> BichonResult<()> {
        if Self::get(account_id)?.is_none() {
            return Ok(());
        }

        delete_impl::<DownloadState>(DB_MANAGER.db(), &account_id.to_string())
    }
}
