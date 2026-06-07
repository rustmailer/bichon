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

use std::collections::{HashMap, HashSet};

use mailbox::MailBox;

pub mod download;
pub mod idle;
pub mod mailbox;
pub mod mailbox_cache;
pub mod task;

pub fn find_missing_mailboxes(
    local_mailboxes: &[MailBox],
    server_mailboxes: &[MailBox],
) -> Vec<MailBox> {
    let local_names: HashSet<_> = local_mailboxes.iter().map(|m| &m.name).collect();
    server_mailboxes
        .iter()
        .filter(|m| !local_names.contains(&m.name))
        .cloned()
        .collect()
}

pub fn find_intersecting_mailboxes(
    local_mailboxes: &[MailBox],
    remote_mailboxes: &[MailBox],
) -> Vec<(MailBox, MailBox)> {
    let local_map: HashMap<_, _> = local_mailboxes
        .iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();
    remote_mailboxes
        .iter()
        .filter_map(|m| {
            local_map
                .get(&m.name)
                .map(|local_mailbox| (local_mailbox.clone(), m.clone()))
        })
        .collect()
}
