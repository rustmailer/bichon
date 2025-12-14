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


use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::{
    modules::{
        error::{code::ErrorCode, BichonResult},
        indexer::{envelope::Envelope, manager::ENVELOPE_INDEX_MANAGER},
        rest::response::DataPage,
    },
    raise_error,
};

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct SearchFilter {
    pub text: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub since: Option<i64>,
    pub before: Option<i64>,
    pub account_id: Option<u64>,
    pub mailbox_id: Option<u64>,
    pub thread_id: Option<u64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub message_id: Option<String>,
    pub has_attachment: Option<bool>,
    pub attachment_name: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct SearchRequest {
    filter: SearchFilter,
    page: u64,
    page_size: u64,
}
impl SearchRequest {
    pub fn validate(&self) -> BichonResult<()> {
        if self.page == 0 || self.page_size == 0 {
            return Err(raise_error!(
                "Both page and page_size must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if self.page_size > 500 {
            return Err(raise_error!(
                "The page_size exceeds the maximum allowed limit of 500.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        Ok(())
    }
}

pub async fn search_messages_impl(request: SearchRequest) -> BichonResult<DataPage<Envelope>> {
    request.validate()?;
    ENVELOPE_INDEX_MANAGER
        .search(request.filter, request.page, request.page_size, true)
        .await
}
