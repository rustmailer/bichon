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

//! Artifacts: the unit of delivery from storage contributors to a backup run.
//!
//! The contributor protocol: instead of copying data into a staging dir for
//! a whole-tree copier to pick up, each contributor *delivers* its exact
//! products as
//! [`Artifact`]s. The manager collects them and hands them to the engine,
//! which content-addresses each (object key = sha256) and records the
//! logical → object mapping in the manifest.

use std::path::PathBuf;

/// Logical identity of an artifact inside a manifest. This is the only place
/// a human-meaningful name exists; on S3 every object is keyed by content
/// hash (see `crate::backup::manifest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKey {
    /// Full memdb in-memory snapshot (gzip `snapshot.json`), streamed from
    /// memory during capture.
    Memdb,
    /// A sealed, immutable blob segment file.
    BlobSegment(u32),
    /// Tantivy envelope index copy (always captured).
    TantivyEnvelope,
    /// Tantivy attachment index copy (always captured).
    TantivyAttachment,
    /// Full audit snapshot — the baseline produced by a rebase.
    AuditBase,
    /// Audit delta covering rows `seq_from < seq <= seq_to`.
    AuditDelta(i64, i64),
    /// Full integrity database copy.
    Integrity,
    /// Full timestamp anchor database copy.
    TimestampAnchor,
    /// Source-UID tombstones (`tombstones/` packaged as one artifact). Without
    /// them a restored archive would re-download messages the operator had
    /// deleted.
    Tombstones,
    /// Pro dual-control frozen message sets (`approval-details/`). The
    /// approval ledger lives in memdb and references these files; without them
    /// a restored queue holds requests whose exact targets are unreadable.
    ApprovalDetails,
    /// Deployment identity: a small JSON package holding `instance-id` (the
    /// id licenses are bound to) and, when present, `license.txt`. Restoring
    /// it keeps the licence valid on the restored data set instead of forcing
    /// a reissue after every disaster recovery.
    DeploymentIdentity,
}

impl ArtifactKey {
    /// Stable, human-meaningful component name for transfer stats and the UI.
    /// Grouped coarser than the enum: every blob segment is `"blob"`, audit
    /// base and deltas are `"audit"` — a per-*segment* breakdown would be
    /// noise in the backup history.
    pub fn label(&self) -> &'static str {
        match self {
            ArtifactKey::Memdb => "memdb",
            ArtifactKey::BlobSegment(_) => "blob",
            ArtifactKey::TantivyEnvelope => "envelope-index",
            ArtifactKey::TantivyAttachment => "attachment-index",
            ArtifactKey::AuditBase | ArtifactKey::AuditDelta(_, _) => "audit",
            ArtifactKey::Tombstones => "tombstones",
            ArtifactKey::ApprovalDetails => "approval-details",
            ArtifactKey::Integrity => "integrity",
            ArtifactKey::TimestampAnchor => "timestamp",
            ArtifactKey::DeploymentIdentity => "deployment",
        }
    }
}

/// One product of a contributor, ready for the engine to upload.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// Local file to upload.
    pub local: PathBuf,
    /// Where this artifact belongs in the manifest.
    pub logical: ArtifactKey,
    /// SHA-256 of `local`'s bytes — the S3 object key is derived from it.
    pub sha256: [u8; 32],
    /// Size of `local` in bytes.
    pub bytes: u64,
}
