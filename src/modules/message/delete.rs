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

use crate::modules::error::BichonResult;
use crate::modules::indexer::manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER};
use std::collections::HashMap;

pub async fn delete_messages_impl(request: HashMap<u64, Vec<u64>>) -> BichonResult<()> {
    EML_INDEX_MANAGER
        .delete_email_multi_account(&request)
        .await?;
    ENVELOPE_INDEX_MANAGER
        .delete_envelopes_multi_account(request)
        .await
}
