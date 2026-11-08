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

//! The restore-point manifest: the single source of truth for what a backup
//! contains, and the atomic commit point that makes a restore point exist.
//!
//! S3 layout (see [`crate::backup::engine`]):
//!
//! ```text
//! <prefix>/v1/meta/LATEST            pointer to the newest manifest
//! <prefix>/v1/meta/m-<id>.json.gz    one manifest per run (full reference list)
//! <prefix>/v1/obj/sha256/<hex>       immutable, content-addressed objects
//! ```
//!
//! A manifest is written *last* in a run, after every object it references —
//! its presence is the commit point. LATEST is written after the manifest.
//! Manifests are fully self-describing: each lists every object it references
//! (no chain diffs), so any single manifest can be validated and restored
//! independently. `previous` links a manifest to its predecessor for chain
//! walks (reachability / retention), not for object resolution.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::backup::artifact::{Artifact, ArtifactKey};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;

/// Schema version of the manifest. Bump on incompatible changes.
pub const MANIFEST_VERSION: u32 = 1;

/// Name of the pointer object under `meta/`.
pub const LATEST_NAME: &str = "LATEST";

/// Prefix of manifest object names under `meta/` (`m-<id>.json.gz`).
pub const MANIFEST_PREFIX: &str = "m-";

/// An immutable object reference: content-addressed key + integrity metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    /// Full store-relative key, e.g. `obj/sha256/ab12...`.
    pub key: String,
    /// Hex SHA-256 of the object's bytes.
    pub sha256: String,
    /// Size in bytes.
    pub bytes: u64,
}

/// A blob segment reference with its on-disk segment id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRef {
    pub seg_id: u32,
    #[serde(flatten)]
    pub obj: ObjectRef,
}

/// One audit delta covering rows `seq_from < seq <= seq_to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditDelta {
    pub seq_from: i64,
    pub seq_to: i64,
    #[serde(flatten)]
    pub obj: ObjectRef,
}

/// Audit chain: a baseline snapshot plus ordered deltas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditObjects {
    pub base: ObjectRef,
    #[serde(default)]
    pub deltas: Vec<AuditDelta>,
    /// Highest audit seq covered by `base` + `deltas`.
    pub cursor: i64,
    /// Cumulative bytes of all deltas since this baseline (0 right after a
    /// rebase). Powers the auto-rebase trigger (chain > 50% of the base,
    /// design doc §7) and the Pro status view; `#[serde(default)]` keeps
    /// pre-rebase manifests (which lack the field) readable.
    #[serde(default)]
    pub cumulative_delta_bytes: u64,
}

/// An optional tantivy index copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TantivyRef {
    /// `envelope` or `attachment`.
    pub name: String,
    #[serde(flatten)]
    pub obj: ObjectRef,
}

/// The object slots of a manifest (community + Pro/Enterprise).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestObjects {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memdb: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blob_segments: Vec<SegmentRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<AuditObjects>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<ObjectRef>,
    /// Source-UID tombstones package (`tombstones/`). Absent in backups
    /// written before this artifact existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstones: Option<ObjectRef>,
    /// Pro dual-control frozen message sets (`approval-details/`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_details: Option<ObjectRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<ObjectRef>,
    /// Deployment identity package (`instance-id` + optional `license.txt`).
    /// Absent in backups written before this artifact existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<ObjectRef>,
    /// Tantivy index copies (envelope + attachment). Derived data —
    /// recreated after a restore instead of being copied back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tantivy: Vec<TantivyRef>,
}

/// One backup run's restore point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// `m-<uuid-v7>`, unique per run and time-ordered.
    pub id: String,
    /// RFC 3339 (UTC) creation time.
    pub created: String,
    /// `"manual"` or `"schedule"`.
    pub trigger: String,
    /// Id of the previously committed manifest, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub objects: ManifestObjects,
}

impl Manifest {
    /// Create a fresh, empty manifest.
    pub fn new(id: String, trigger: &str, previous: Option<String>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            created: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            id,
            trigger: trigger.to_string(),
            previous,
            objects: ManifestObjects::default(),
        }
    }

    /// The store-relative object key under which `hash` lives.
    pub fn object_key(hash: &[u8; 32]) -> String {
        format!("obj/sha256/{}", hex::encode(hash))
    }

    /// Map one delivered artifact into its manifest slot.
    pub fn add_artifact(&mut self, art: &Artifact) -> BichonResult<()> {
        let obj = ObjectRef {
            key: Self::object_key(&art.sha256),
            sha256: hex::encode(art.sha256),
            bytes: art.bytes,
        };
        match &art.logical {
            ArtifactKey::Memdb => self.objects.memdb = Some(obj),
            ArtifactKey::BlobSegment(seg_id) => {
                self.objects.blob_segments.push(SegmentRef {
                    seg_id: *seg_id,
                    obj,
                });
            }
            ArtifactKey::TantivyEnvelope => {
                self.objects.tantivy.push(TantivyRef {
                    name: "envelope".to_string(),
                    obj,
                });
            }
            ArtifactKey::TantivyAttachment => {
                self.objects.tantivy.push(TantivyRef {
                    name: "attachment".to_string(),
                    obj,
                });
            }
            ArtifactKey::AuditBase => {
                let audit = self.audit_mut()?;
                audit.base = obj;
            }
            ArtifactKey::AuditDelta(seq_from, seq_to) => {
                let audit = self.audit_mut()?;
                audit.deltas.push(AuditDelta {
                    seq_from: *seq_from,
                    seq_to: *seq_to,
                    obj,
                });
            }
            ArtifactKey::Integrity => self.objects.integrity = Some(obj),
            ArtifactKey::Tombstones => self.objects.tombstones = Some(obj),
            ArtifactKey::ApprovalDetails => self.objects.approval_details = Some(obj),
            ArtifactKey::TimestampAnchor => self.objects.timestamp = Some(obj),
            ArtifactKey::DeploymentIdentity => self.objects.deployment = Some(obj),
        }
        Ok(())
    }

    /// Set the audit chain cursor (highest backed-up seq). A delta-only run
    /// legitimately carries no base in its *own* manifest — the base lives in
    /// an earlier manifest of the chain — so no base is required here.
    pub fn set_audit_cursor(&mut self, cursor: i64) -> BichonResult<()> {
        self.audit_mut()?.cursor = cursor;
        Ok(())
    }

    fn audit_mut(&mut self) -> BichonResult<&mut AuditObjects> {
        if self.objects.audit.is_none() {
            self.objects.audit = Some(AuditObjects::default());
        }
        Ok(self.objects.audit.as_mut().expect("just inserted"))
    }

    /// Every object key this manifest references (for GC / verify).
    pub fn object_keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        for slot in [
            &self.objects.memdb,
            &self.objects.integrity,
            &self.objects.tombstones,
            &self.objects.approval_details,
            &self.objects.timestamp,
            &self.objects.deployment,
        ]
        .into_iter()
        .flatten()
        {
            keys.push(slot.key.as_str());
        }
        for seg in &self.objects.blob_segments {
            keys.push(seg.obj.key.as_str());
        }
        if let Some(audit) = &self.objects.audit {
            keys.push(audit.base.key.as_str());
            for d in &audit.deltas {
                keys.push(d.obj.key.as_str());
            }
        }
        for t in &self.objects.tantivy {
            keys.push(t.obj.key.as_str());
        }
        keys
    }

    /// Every object reference `(key, bytes)` for size checks.
    pub fn object_refs(&self) -> Vec<(&str, u64)> {
        let mut out = Vec::new();
        for slot in [
            &self.objects.memdb,
            &self.objects.integrity,
            &self.objects.tombstones,
            &self.objects.approval_details,
            &self.objects.timestamp,
            &self.objects.deployment,
        ]
        .into_iter()
        .flatten()
        {
            out.push((slot.key.as_str(), slot.bytes));
        }
        for seg in &self.objects.blob_segments {
            out.push((seg.obj.key.as_str(), seg.obj.bytes));
        }
        if let Some(audit) = &self.objects.audit {
            out.push((audit.base.key.as_str(), audit.base.bytes));
            for d in &audit.deltas {
                out.push((d.obj.key.as_str(), d.obj.bytes));
            }
        }
        for t in &self.objects.tantivy {
            out.push((t.obj.key.as_str(), t.obj.bytes));
        }
        out
    }

    /// Gzip + JSON encode.
    pub fn encode_gz(&self) -> BichonResult<Vec<u8>> {
        let json = serde_json::to_vec(self).map_err(|e| {
            raise_error!(
                format!("backup: manifest encode failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&json).map_err(|e| {
            raise_error!(
                format!("backup: manifest gzip failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        enc.finish().map_err(|e| {
            raise_error!(
                format!("backup: manifest gzip failed: {e}"),
                ErrorCode::InternalError
            )
        })
    }

    /// Decode from gzip + JSON.
    pub fn decode_gz(bytes: &[u8]) -> BichonResult<Self> {
        let mut dec = flate2::read::GzDecoder::new(bytes);
        let mut json = Vec::new();
        dec.read_to_end(&mut json).map_err(|e| {
            raise_error!(
                format!("backup: manifest gunzip failed: {e}"),
                ErrorCode::InternalError
            )
        })?;
        serde_json::from_slice(&json).map_err(|e| {
            raise_error!(
                format!("backup: manifest decode failed: {e}"),
                ErrorCode::InternalError
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::artifact::Artifact;
    use std::path::PathBuf;

    fn artifact(logical: ArtifactKey, byte: u8, len: usize) -> Artifact {
        Artifact {
            local: PathBuf::from("unused"),
            logical,
            sha256: [byte; 32],
            bytes: len as u64,
        }
    }

    #[test]
    fn manifest_roundtrips_through_gzip() {
        let mut m = Manifest::new("m-1".to_string(), "manual", Some("m-0".to_string()));
        m.add_artifact(&artifact(ArtifactKey::Memdb, 0x01, 100)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::BlobSegment(7), 0x02, 200)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::AuditBase, 0x03, 300)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::AuditDelta(1, 42), 0x04, 40)).unwrap();
        m.set_audit_cursor(42).unwrap();

        let bytes = m.encode_gz().unwrap();
        let decoded = Manifest::decode_gz(&bytes).unwrap();
        assert_eq!(decoded, m);

        assert_eq!(decoded.objects.blob_segments[0].seg_id, 7);
        let audit = decoded.objects.audit.unwrap();
        assert_eq!(audit.cursor, 42);
        assert_eq!(audit.deltas[0].seq_from, 1);
        assert_eq!(audit.deltas[0].seq_to, 42);
    }

    #[test]
    fn object_key_is_content_addressed() {
        let hash = [0xAB; 32];
        let key = Manifest::object_key(&hash);
        assert!(key.starts_with("obj/sha256/"));
        assert_eq!(key.len(), "obj/sha256/".len() + 64);
    }

    #[test]
    fn object_keys_lists_every_reference() {
        let mut m = Manifest::new("m-1".to_string(), "manual", None);
        m.add_artifact(&artifact(ArtifactKey::Memdb, 0x01, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::BlobSegment(1), 0x03, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::BlobSegment(2), 0x04, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::AuditBase, 0x05, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::AuditDelta(1, 2), 0x06, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::Integrity, 0x07, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::TimestampAnchor, 0x08, 1)).unwrap();
        m.add_artifact(&artifact(ArtifactKey::DeploymentIdentity, 0x09, 1)).unwrap();

        let keys = m.object_keys();
        assert_eq!(
            keys.len(),
            8,
            "memdb+2 segs+audit base+delta+integrity+timestamp+deployment"
        );
        assert_eq!(m.object_refs().len(), 8);
        // Content addressing ⇒ distinct bytes ⇒ distinct keys.
        let uniq: std::collections::HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(uniq.len(), keys.len(), "all references must be distinct objects");
    }

    #[test]
    fn cursor_is_set_on_delta_only_manifest() {
        let mut m = Manifest::new("m-1".to_string(), "manual", None);
        // A delta-only run has no base in its own manifest (the base lives in
        // an earlier manifest of the chain) yet still records a cursor.
        m.set_audit_cursor(5).unwrap();
        assert_eq!(m.objects.audit.unwrap().cursor, 5);
    }

    #[test]
    fn old_manifests_without_cumulative_delta_load_as_zero() {
        // A manifest written before `cumulative_delta_bytes` existed must
        // still decode (the field defaults to 0), so pre-rebase chains stay
        // readable.
        let legacy = br#"{"version":1,"id":"m-legacy","created":"2026-09-21T00:00:00Z","trigger":"manual","objects":{"audit":{"base":{"key":"obj/sha256/ab","sha256":"ab","bytes":100},"deltas":[],"cursor":7}}}"#;
        let m = Manifest::decode_gz(&{
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut enc, legacy).unwrap();
            enc.finish().unwrap()
        })
        .unwrap();
        let audit = m.objects.audit.unwrap();
        assert_eq!(audit.cursor, 7);
        assert_eq!(audit.cumulative_delta_bytes, 0);
    }

    #[test]
    fn tombstones_artifact_is_wired_into_the_manifest() {
        let mut m = Manifest::new("m-tomb".to_string(), "manual", None);
        let art = artifact(ArtifactKey::Tombstones, 0x77, 4096);
        m.add_artifact(&art).unwrap();
        let slot = m
            .objects
            .tombstones
            .as_ref()
            .expect("tombstones slot must be populated");
        assert_eq!(slot.bytes, 4096);
        assert_eq!(slot.sha256, hex::encode([0x77u8; 32]));
        assert!(m.object_keys().contains(&slot.key.as_str()));
        assert!(m
            .object_refs()
            .iter()
            .any(|(_, bytes)| *bytes == 4096));

        // Older manifests have no such field; they must still deserialize.
        let json = serde_json::json!({
            "version": MANIFEST_VERSION,
            "id": "m-old",
            "created": "2026-01-01T00:00:00.000Z",
            "trigger": "manual",
            "objects": {}
        });
        let old: Manifest = serde_json::from_value(json).unwrap();
        assert!(old.objects.tombstones.is_none());
    }
}