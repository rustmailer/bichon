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

use duckdb::types::Value;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::modules::{account::migration::AccountModel, cache::imap::mailbox::MailBox};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct Envelope {
    pub id: u64,
    pub message_id: String,
    pub account_id: u64,
    pub account_email: Option<String>,
    pub mailbox_id: u64,
    pub mailbox_name: Option<String>,
    pub uid: u32,
    pub subject: String,
    pub text: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub date: i64,
    pub internal_date: i64,
    pub size: u32,
    pub thread_id: u64,
    pub attachment_count: usize,
    pub tags: Option<Vec<String>>,
}

impl Envelope {
    pub fn from_row(row: &duckdb::Row) -> duckdb::Result<Self> {
        let get_list = |col_name: &str| -> Vec<String> {
            row.get::<_, Value>(col_name)
                .map(|v| {
                    if let Value::List(inner_list) = v {
                        inner_list
                            .into_iter()
                            .filter_map(|item| {
                                if let Value::Text(s) = item {
                                    Some(s)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        vec![]
                    }
                })
                .unwrap_or_default()
        };
        let account_id = row.get("account_id")?;
        let mailbox_id = row.get("mailbox_id")?;
        let email = match AccountModel::get(account_id) {
            Ok(account) => account.email,
            Err(_) => "unknown".to_string(),
        };

        let mailbox_name = MailBox::find_mailbox(account_id, mailbox_id)
            .ok()
            .and_then(|m| m)
            .map(|m| m.name)
            .unwrap_or_else(|| "unknown".to_string());

        Ok(Self {
            id: row.get("id")?,
            message_id: row.get("message_id").unwrap_or_default(),
            account_id,
            account_email: Some(email),
            mailbox_id,
            mailbox_name: Some(mailbox_name),
            uid: row.get::<_, u64>("uid")? as u32,
            subject: row.get("subject").unwrap_or_default(),
            text: row.get("body").unwrap_or_default(),
            from: row.get("sender").unwrap_or_default(),
            to: get_list("recipients"),
            cc: get_list("cc"),
            bcc: get_list("bcc"),
            date: row.get("sent_at").unwrap_or(0),
            internal_date: row.get("received_at").unwrap_or(0),
            size: row.get::<_, u64>("size_bytes")? as u32,
            thread_id: row.get("thread_id").unwrap_or(0),
            attachment_count: row.get::<_, i32>("attachment_count")? as usize,
            tags: {
                let t = get_list("tags");
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            },
        })
    }
}
