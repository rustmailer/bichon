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
    cache::imap::mailbox::MailBox,
    error::BichonResult,
    store::tantivy::envelope::ENVELOPE_MANAGER,
};

pub async fn delete_mailbox_impl(account_id: u64, mailbox_id: u64) -> BichonResult<()> {
    let mailbox = MailBox::get(mailbox_id)?;

    let name = mailbox.name;
    let delimiter = mailbox.delimiter.unwrap_or("/".to_owned());
    let all_mailboxes = MailBox::list_all(account_id)?;

    let prefix = format!("{}{}", name, delimiter);
    let ids_to_delete: Vec<u64> = all_mailboxes
        .into_iter()
        .filter(|m| m.id == mailbox_id || m.name.starts_with(&prefix))
        .map(|m| m.id)
        .collect();

    if ids_to_delete.is_empty() {
        return Ok(());
    }

    for id in &ids_to_delete {
        MailBox::delete(*id)?;
    }

    ENVELOPE_MANAGER
        .delete_mailbox_envelopes(account_id, ids_to_delete)
        .await?;
    Ok(())
}
