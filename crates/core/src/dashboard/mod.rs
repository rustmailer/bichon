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
    store::tantivy::{
        attachment::ATTACHMENT_MANAGER,
        envelope::ENVELOPE_MANAGER,
        fields::{F_CONTENT_HASH, F_ID},
        schema::SchemaTools,
    },
    users::permissions::Permission,
};
//use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tantivy::{schema::Value, TantivyDocument};

use crate::{
    bichon_version, raise_error,
    {
        account::migration::AccountModel,
        common::auth::ClientContext,
        error::{code::ErrorCode, BichonResult},
        settings::dir::DATA_DIR_MANAGER,
        utils::get_total_size,
    },
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct DashboardStats {
    pub account_count: usize,                            // Number of accounts
    pub email_count: u64,                                // Total number of emails
    pub attachment_count: u64,                           // Total number of attachments
    pub total_size_bytes: u64,                           // Total size of all emails (in bytes)
    pub storage_usage_bytes: u64,                        // Actual storage used (in bytes)
    pub index_usage_bytes: u64,                          // Index storage size (in bytes)
    pub recent_activity: Vec<TimeBucket>,                // Email activity over recent days
    pub top_senders: Vec<Group>,                         // Top 10 senders
    pub top_accounts: Vec<Group>,                        // Top 10 accounts
    pub with_attachment_count: u64,                      // Emails with attachments
    pub without_attachment_count: u64,                   // Emails without attachments
    pub top_largest_emails: Vec<LargestEmail>,           // Top 10 largest emails
    pub top_largest_attachments: Vec<LargestAttachment>, // Top 10 largest attachments
    pub system_version: String, // The semantic version string of the currently running backend service
}

impl DashboardStats {
    pub async fn get(context: ClientContext) -> BichonResult<Self> {
        let has_all_accounts = context.has_permission(None, Permission::ACCOUNT_MANAGE_ALL);
        let authorized_ids: Option<HashSet<u64>> = if has_all_accounts {
            None
        } else {
            Some(context.user.account_access_map.keys().cloned().collect())
        };

        let mut stat = ENVELOPE_MANAGER.get_dashboard_stats(&authorized_ids)?;

        stat.top_largest_emails = ENVELOPE_MANAGER.top_10_largest_emails(&authorized_ids)?;
        stat.top_largest_attachments =
            ATTACHMENT_MANAGER.top_10_largest_attachments(&authorized_ids)?;

        stat.account_count = if has_all_accounts {
            AccountModel::count()?
        } else {
            authorized_ids.as_ref().map(|ids| ids.len()).unwrap_or(0)
        };

        stat.email_count = ENVELOPE_MANAGER.total_emails(&authorized_ids)?;
        stat.attachment_count = ATTACHMENT_MANAGER.total_attachments(&authorized_ids)?;
        stat.storage_usage_bytes = get_total_size(&DATA_DIR_MANAGER.storage_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        stat.index_usage_bytes = get_total_size(&&DATA_DIR_MANAGER.envelope_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        stat.system_version = bichon_version!().to_string();

        Ok(stat)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct TimeBucket {
    pub timestamp_ms: i64, // Timestamp in milliseconds
    pub count: u64,        // Number of emails in this time bucket
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Group {
    pub key: String,
    pub count: u64, // Number of emails from this sender
    /// Set for `top_accounts` only: the account the bucket was grouped by.
    /// `key` holds the account email, which is NOT unique across accounts
    /// (e.g. a NoSync import and an IMAP account for the same mailbox), so
    /// clients must resolve names/search targets through this id. Aggregations
    /// keyed by an inherently unique value (sender address, attachment type,
    /// ...) leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct LargestEmail {
    pub subject: String, // Email subject
    pub size_bytes: u64, // Email size in bytes
    pub id: String,
}

impl LargestEmail {
    pub fn from_tantivy_doc(document: &TantivyDocument) -> BichonResult<Self> {
        let fields = SchemaTools::email_fields();
        let value = document.get_first(fields.f_size).ok_or_else(|| {
            raise_error!(
                "miss 'size' field in tantivy document".into(),
                ErrorCode::InternalError
            )
        })?;
        let size_bytes = value.as_u64().ok_or_else(|| {
            raise_error!("'size' field is not a u64".into(), ErrorCode::InternalError)
        })?;
        let value = document.get_first(fields.f_subject).ok_or_else(|| {
            raise_error!("'subject' field not found".into(), ErrorCode::InternalError)
        })?;
        let subject = value.as_str().map(|s| s.to_string()).ok_or_else(|| {
            raise_error!(
                "'subject' field is not a string".into(),
                ErrorCode::InternalError
            )
        })?;

        let value = document.get_first(fields.f_id).ok_or_else(|| {
            raise_error!(
                format!("'{}' field not found", F_ID),
                ErrorCode::InternalError
            )
        })?;
        let id = value.as_str().map(|s| s.to_string()).ok_or_else(|| {
            raise_error!(
                format!("'{}' field is not a string", F_ID),
                ErrorCode::InternalError
            )
        })?;

        let envelope = LargestEmail {
            subject,
            size_bytes,
            id,
        };

        Ok(envelope)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct LargestAttachment {
    pub name: String,    // Attachment name
    pub size_bytes: u64, // Attachment size in bytes
    pub id: String,
    pub content_hash: String,
}

impl LargestAttachment {
    pub fn from_tantivy_doc(document: &TantivyDocument) -> BichonResult<Self> {
        let fields = SchemaTools::attachment_fields();
        let value = document.get_first(fields.f_size).ok_or_else(|| {
            raise_error!(
                "miss 'size' field in tantivy document".into(),
                ErrorCode::InternalError
            )
        })?;
        let size_bytes = value.as_u64().ok_or_else(|| {
            raise_error!("'size' field is not a u64".into(), ErrorCode::InternalError)
        })?;
        let name = document
            .get_first(fields.f_name_exact)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let value = document.get_first(fields.f_id).ok_or_else(|| {
            raise_error!(
                format!("'{}' field not found", F_ID),
                ErrorCode::InternalError
            )
        })?;
        let id = value.as_str().map(|s| s.to_string()).ok_or_else(|| {
            raise_error!(
                format!("'{}' field is not a string", F_ID),
                ErrorCode::InternalError
            )
        })?;

        let value = document.get_first(fields.f_content_hash).ok_or_else(|| {
            raise_error!(
                format!("'{}' field not found", F_CONTENT_HASH),
                ErrorCode::InternalError
            )
        })?;
        let content_hash = value.as_str().map(|s| s.to_string()).ok_or_else(|| {
            raise_error!(
                format!("'{}' field is not a string", F_CONTENT_HASH),
                ErrorCode::InternalError
            )
        })?;

        let attachment = LargestAttachment {
            name,
            size_bytes,
            id,
            content_hash,
        };

        Ok(attachment)
    }
}
