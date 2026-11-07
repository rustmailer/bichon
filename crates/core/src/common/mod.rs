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

use std::ops::Deref;

use mail_parser::{AddressList as ImapAddressList, Mailbox as ImapMailbox};
use serde::{Deserialize, Serialize};
pub mod auth;
pub mod paginated;
pub mod periodic;
pub mod rustls;
pub mod signal;
#[cfg(feature = "web-api")]
pub mod validator;

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct Addr {
    /// The optional display name associated with the email address (e.g., "John Doe").
    /// If `None`, no display name is specified.
    pub name: Option<String>,
    /// The optional email address (e.g., "john.doe@example.com").
    /// If `None`, the address is unavailable, though typically at least one of `name` or `address` is provided.
    pub address: Option<String>,
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.name, &self.address) {
            (Some(name), Some(address)) => write!(f, "{} <{}>", name, address),
            (None, Some(address)) => write!(f, "<{}>", address),
            (Some(name), None) => write!(f, "{}", name),
            (None, None) => write!(f, ""),
        }
    }
}

impl<'x> From<ImapMailbox<'x>> for Addr {
    fn from(original: ImapMailbox<'x>) -> Self {
        Addr {
            name: original.name().map(|s| s.to_string()),
            address: original.address().map(|s| s.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddrVec(pub Vec<Addr>);

impl Deref for AddrVec {
    type Target = Vec<Addr>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'x> From<ImapAddressList<'x>> for AddrVec {
    fn from(original: ImapAddressList<'x>) -> Self {
        // `mailboxes()` flattens groups, preserving document order.
        AddrVec(original.mailboxes().map(Addr::from).collect())
    }
}
