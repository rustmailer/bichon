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

//! Data types shared between the server-side batch export API and its
//! clients (web UI and CLI). The export engine itself lives in the server
//! crate; this module only carries the wire format.

use crate::message::search::{AttachmentSearchFilter, EmailSearchFilter};
use serde::{Deserialize, Serialize};

/// Export file format for batch exports. `Mbox` packs whole messages; the
/// saved search must be of kind `Email`. `Attachments` packs the attachments
/// the saved search matches (deduplicated by content) into a zip; the saved
/// search must be of kind `Attachment`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
pub enum ExportFormat {
    #[default]
    Mbox,
    Attachments,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Mbox => "mbox",
            ExportFormat::Attachments => "attachments",
        }
    }
}

/// An ad-hoc export selection, built from the UI's current view (for example
/// "every image attachment" or "every PDF") instead of a saved search.
///
/// Which half is used is decided by `ExportFormat`: `Mbox` reads `email`,
/// `Attachments` reads `attachment`. The other half must be left unset — a
/// request that carries both is ambiguous and is rejected rather than guessed
/// at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportFilter {
    #[serde(default)]
    pub email: Option<EmailSearchFilter>,
    #[serde(default)]
    pub attachment: Option<AttachmentSearchFilter>,
}

/// Where an export's contents come from. Exactly one of `saved_search_id` and
/// `filter` is set; `label` names an ad-hoc export in the job list and on the
/// approval card (a saved search carries its own name).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportSource {
    #[serde(default)]
    pub saved_search_id: Option<String>,
    #[serde(default)]
    pub filter: Option<ExportFilter>,
    #[serde(default)]
    pub label: Option<String>,
}

/// One account that will be (or was) included in an export scope.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportAccount {
    pub id: u64,
    pub email: String,
    pub name: Option<String>,
}

/// Request body for previewing an export before starting it.
///
/// Either `saved_search_id` or `filter` selects the contents; see
/// [`ExportSource`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportPreviewRequest {
    #[serde(default)]
    pub saved_search_id: Option<String>,
    /// Ad-hoc selection, mutually exclusive with `saved_search_id`.
    #[serde(default)]
    pub filter: Option<ExportFilter>,
    /// Display name for an ad-hoc export; ignored for saved searches.
    #[serde(default)]
    pub label: Option<String>,
    /// Format to preview for. Defaults to `Mbox`; the saved-search kind must
    /// match (Email → Mbox, Attachment → Attachments).
    #[serde(default)]
    pub format: Option<ExportFormat>,
}

impl ExportPreviewRequest {
    pub fn source(&self) -> ExportSource {
        ExportSource {
            saved_search_id: self.saved_search_id.clone(),
            filter: self.filter.clone(),
            label: self.label.clone(),
        }
    }
}

/// What an export would contain, computed before the job starts so the
/// caller can confirm the scope (account count, email count) first.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportPreviewView {
    pub saved_search_id: String,
    pub saved_search_name: String,
    pub format: ExportFormat,
    pub accounts: Vec<ExportAccount>,
    pub total_emails: u64,
    pub total_size: u64,
}

/// Request body for starting a batch export. Either `saved_search_id` or
/// `filter` selects the contents; see [`ExportSource`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportCreateRequest {
    #[serde(default)]
    pub saved_search_id: Option<String>,
    /// Ad-hoc selection, mutually exclusive with `saved_search_id`.
    #[serde(default)]
    pub filter: Option<ExportFilter>,
    /// Display name for an ad-hoc export; ignored for saved searches.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub format: Option<ExportFormat>,
}

impl ExportCreateRequest {
    pub fn source(&self) -> ExportSource {
        ExportSource {
            saved_search_id: self.saved_search_id.clone(),
            filter: self.filter.clone(),
            label: self.label.clone(),
        }
    }
}

/// Snapshot of an export job, returned by the status and control endpoints.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportJobView {
    pub job_id: String,
    pub status: String,
    pub saved_search_id: String,
    pub saved_search_name: String,
    pub format: ExportFormat,
    pub accounts: Vec<ExportAccount>,
    pub total_emails: u64,
    pub total_size: u64,
    pub processed: u64,
    pub exported: u64,
    pub failed: u64,
    pub error: Option<String>,
    pub artifact_name: Option<String>,
    pub artifact_size: u64,
    /// SHA-256 of the finished mbox artifact, recorded at export time.
    /// Present only for finished exports.
    pub artifact_hash: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    /// Compliance verification status: idle, running, finished or failed.
    /// idle means the export has never been verified.
    #[serde(default)]
    pub verify_status: String,
    /// Messages checked so far and total when finished.
    #[serde(default)]
    pub verify_checked: u64,
    #[serde(default)]
    pub verify_matched: u64,
    #[serde(default)]
    pub verify_mismatched: u64,
    /// Verification error when the verification failed.
    #[serde(default)]
    pub verify_error: Option<String>,
    #[serde(default)]
    pub verify_started_at: Option<i64>,
    #[serde(default)]
    pub verify_finished_at: Option<i64>,
}

/// One per-message mismatch found by a compliance verification. The export
/// manifest records each message's content hash; a mismatch means the
/// archived message no longer matches what was exported (or the archive was
/// modified since the export ran).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportVerifyMismatch {
    pub envelope_id: String,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    /// Content hash recorded in the export manifest.
    pub expected_hash: Option<String>,
    /// Content hash recomputed from the archive at verify time.
    pub actual_hash: Option<String>,
    /// Why the message did not match. One of `blob_missing`,
    /// `attachment_missing` or `content_changed`.
    pub reason: String,
}

/// Result of a compliance verification of a finished export: the artifact's
/// recorded SHA-256 is recomputed and compared, and each exported message's
/// content hash is cross-checked against the live archive.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportVerifyView {
    pub job_id: String,
    pub status: String,
    pub artifact_name: Option<String>,
    /// SHA-256 recorded in the manifest at export time.
    pub expected_artifact_hash: Option<String>,
    /// SHA-256 recomputed from the on-disk artifact.
    pub actual_artifact_hash: Option<String>,
    pub artifact_hash_match: bool,
    /// Messages whose content hash still matches the archive.
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub verified_at: i64,
    pub mismatches: Vec<ExportVerifyMismatch>,
}

/// Live progress of a compliance verification of a finished export. The
/// verification runs in the background so large archives do not block the
/// API; poll this until `status` is `finished` or `failed`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ExportVerifyProgressView {
    pub job_id: String,
    /// `idle`, `running`, `finished` or `failed`.
    pub status: String,
    /// Messages checked so far.
    pub checked: u64,
    /// Total messages from the manifest.
    pub total: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub error: Option<String>,
    /// Present when `status` is `finished`.
    pub result: Option<ExportVerifyView>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The REST body carries the enum's variant names, while audit rows and
    /// approval targets carry `as_str()` (`"mbox"` / `"attachments"`). Pinning
    /// both spellings: the web client sends the former, the ledger stores the
    /// latter, and mixing them up fails only at runtime.
    #[test]
    fn format_wire_values_are_variant_names() {
        assert_eq!(
            serde_json::to_string(&ExportFormat::Mbox).unwrap(),
            "\"Mbox\""
        );
        assert_eq!(
            serde_json::to_string(&ExportFormat::Attachments).unwrap(),
            "\"Attachments\""
        );
        assert_eq!(ExportFormat::Mbox.as_str(), "mbox");
        assert_eq!(ExportFormat::Attachments.as_str(), "attachments");
    }

    /// Both request shapes parse: a saved search (the original wire format,
    /// still sent by the CLI) and an ad-hoc filter (what the attachments page
    /// sends).
    #[test]
    fn create_request_parses_saved_search_and_adhoc_filter() {
        let saved: ExportCreateRequest = serde_json::from_str(
            r#"{"saved_search_id":"ss-1","format":"Mbox"}"#,
        )
        .unwrap();
        assert_eq!(saved.source().saved_search_id.as_deref(), Some("ss-1"));
        assert!(saved.source().filter.is_none());

        let adhoc: ExportCreateRequest = serde_json::from_str(
            r#"{"filter":{"attachment":{"attachment_category":"image"}},
                "label":"All images","format":"Attachments"}"#,
        )
        .unwrap();
        let source = adhoc.source();
        assert!(source.saved_search_id.is_none());
        assert_eq!(source.label.as_deref(), Some("All images"));
        assert_eq!(
            source
                .filter
                .unwrap()
                .attachment
                .unwrap()
                .attachment_category
                .as_deref(),
            Some("image")
        );
    }
}
