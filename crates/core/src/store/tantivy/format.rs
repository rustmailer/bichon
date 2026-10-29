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

//! Index format version marker.
//!
//! The word-level tokenizer shape changed (no language detection, no stemming),
//! which makes the terms of a legacy index unable to match new queries. So each
//! index directory carries an `INDEX_FORMAT` file holding a schema-generation
//! number:
//! - no marker and the directory is a tantivy index → legacy format (1),
//!   a reindex is required
//! - marker >= [`CURRENT_INDEX_FORMAT_VERSION`] → usable
//! - directory missing / empty → fresh install
//!
//! The file lives **inside each index directory** (not at the data root) on
//! purpose: the backup packs only `mail_metadata/` and `attachment_metadata/`
//! (`backup/prepare.rs`), never the `bichon-indices/` parent nor the data root,
//! so an in-index marker rides along with backup/restore for free. A marker at
//! the data root would be lost on every restore and force a spurious reindex.

use std::path::{Path, PathBuf};

use crate::migrate::is_tantivy_index_dir;
use crate::store::tantivy::fields::{F_CLEANUP_MOVED, F_IMAP_UID};

/// Current index schema generation.
///
/// This is a **generation id**, not just a boolean: it identifies which schema
/// an index was built with, so we can reason about "the previous schema" as well
/// as "needs a reindex".
/// - 1 = legacy: whichlang-stemmed word tokenizer, no trigram sibling fields
///       (a missing marker file is treated as 1)
/// - 2 = deterministic word tokenizer + trigram sibling fields
/// - 3 = + imap_uid field (stable archive-side IMAP UID)
///       + cleanup_moved flag (source-mailbox cleanup marker)
/// - 4 = + from_name / wire_from fields (sender display name and the original
///       address behind an account's header override rule)
///
/// Bump this whenever the schema changes **or** a reindex becomes necessary for
/// any other reason (e.g. a future tokenizer change that leaves the schema
/// untouched). The check compares the on-disk marker against this constant, so
/// bumping it alone is enough to gate older indexes behind the reindex flow.
pub const CURRENT_INDEX_FORMAT_VERSION: u32 = 4;

const INDEX_FORMAT_FILE: &str = "INDEX_FORMAT";

/// Whether an index directory is usable by the current binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFormatStatus {
    /// Directory missing or empty: fresh install, an index can be built directly.
    Fresh,
    /// Marker == current version.
    Current,
    /// Directory is a tantivy index with no marker (or a stale one): reindex required.
    NeedsUpgrade,
}

/// Write the current index format version to `dir/INDEX_FORMAT`.
pub fn write_index_format_version(dir: &Path) -> std::io::Result<()> {
    std::fs::write(
        dir.join(INDEX_FORMAT_FILE),
        format!("{}\n", CURRENT_INDEX_FORMAT_VERSION),
    )
}

/// Read the format version from an index directory; `None` when no marker file exists.
pub fn read_index_format_version(dir: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(dir.join(INDEX_FORMAT_FILE)).ok()?;
    content.trim().parse().ok()
}

/// Self-heal an index created by this binary whose marker write was lost — e.g.
/// a crash between `Index::create_in_dir` and [`write_index_format_version`].
///
/// At that moment the directory has no segment files yet, so the startup gate
/// sees `Fresh` and lets the server proceed; once mail is ingested it becomes a
/// real tantivy index with no marker and every later start would demand a
/// reindex. Stamping it here fixes that.
///
/// The marker is written **only** when the on-disk schema already proves the
/// current format (it carries the `imap_uid` field). Older generations never
/// have that field — note a v2 index *does* carry the trigram siblings, so the
/// discriminator has to be the newest field, not `subject_tri` — so they are
/// never stamped and still go through the reindex gate. Returns `true` when a
/// marker was written.
pub fn heal_index_format_marker(dir: &Path) -> bool {
    if read_index_format_version(dir).is_some() {
        return false;
    }
    let Ok(index) = tantivy::Index::open_in_dir(dir) else {
        return false;
    };
    // Check for a field the *current* generation introduced, not an older one:
    // a schema-3 index also has `imap_uid`, so testing only that would stamp a
    // stale index as current and then fail at query time. `from_name` is the
    // newest discriminator (generation 4).
    let schema = index.schema();
    if schema.get_field(F_IMAP_UID).is_err()
        || schema.get_field(F_CLEANUP_MOVED).is_err()
        || schema.get_field("from_name").is_err()
    {
        return false;
    }
    write_index_format_version(dir).is_ok()
}

/// Determine the format status of an index directory.
pub fn check_index_format(dir: &Path) -> IndexFormatStatus {
    // Directory missing or empty → fresh install
    let empty = !dir.exists()
        || std::fs::read_dir(dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
    if empty {
        return IndexFormatStatus::Fresh;
    }

    match read_index_format_version(dir) {
        Some(v) if v >= CURRENT_INDEX_FORMAT_VERSION => IndexFormatStatus::Current,
        // Marker present but stale → upgrade required
        Some(_) => IndexFormatStatus::NeedsUpgrade,
        // No marker: only a confirmed tantivy index counts as legacy; anything
        // else is treated as unknown / fresh.
        None => {
            if is_tantivy_index_dir(&PathBuf::from(dir)).unwrap_or(false) {
                IndexFormatStatus::NeedsUpgrade
            } else {
                IndexFormatStatus::Fresh
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("bichon-format-test")
            .join(prefix)
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a minimal tantivy index directory (meta.json + ≥3 index-extension files).
    fn make_tantivy_index_fixture(dir: &Path) {
        fs::write(dir.join("meta.json"), "{}").unwrap();
        for ext in ["store", "term", "idx"] {
            fs::write(dir.join(format!("seg.{ext}")), b"x").unwrap();
        }
    }

    #[test]
    fn missing_dir_is_fresh() {
        let dir = temp_dir("missing").join("does-not-exist");
        assert_eq!(check_index_format(&dir), IndexFormatStatus::Fresh);
    }

    #[test]
    fn empty_dir_is_fresh() {
        let dir = temp_dir("empty");
        assert_eq!(check_index_format(&dir), IndexFormatStatus::Fresh);
    }

    #[test]
    fn untagged_tantivy_index_needs_upgrade() {
        let dir = temp_dir("untagged");
        make_tantivy_index_fixture(&dir);
        assert_eq!(check_index_format(&dir), IndexFormatStatus::NeedsUpgrade);
    }

    #[test]
    fn current_tag_is_current() {
        let dir = temp_dir("current");
        make_tantivy_index_fixture(&dir);
        write_index_format_version(&dir).unwrap();
        assert_eq!(check_index_format(&dir), IndexFormatStatus::Current);
    }

    #[test]
    fn stale_tag_needs_upgrade() {
        let dir = temp_dir("stale");
        make_tantivy_index_fixture(&dir);
        fs::write(dir.join("INDEX_FORMAT"), "1\n").unwrap();
        assert_eq!(check_index_format(&dir), IndexFormatStatus::NeedsUpgrade);
    }

    #[test]
    fn non_index_dir_without_tag_is_fresh() {
        let dir = temp_dir("not-an-index");
        fs::write(dir.join("random.txt"), b"hello").unwrap();
        assert_eq!(check_index_format(&dir), IndexFormatStatus::Fresh);
    }

    /// A crash between creating the index and stamping it leaves an unmarked
    /// index. It must be healed — but only when its schema proves the current
    /// format, never for an older generation.
    #[test]
    fn heals_a_missing_marker_only_for_the_current_schema() {
        use crate::store::tantivy::schema::SchemaTools;

        // Current schema, no marker, no segment files yet → the gate sees Fresh.
        let dir = temp_dir("heal-current");
        let current = tantivy::Index::create_in_dir(&dir, SchemaTools::email_schema()).unwrap();
        assert_eq!(check_index_format(&dir), IndexFormatStatus::Fresh);

        assert!(heal_index_format_marker(&dir), "current-format index is healed");
        assert_eq!(
            read_index_format_version(&dir),
            Some(CURRENT_INDEX_FORMAT_VERSION)
        );
        assert!(!heal_index_format_marker(&dir), "a second call is a no-op");
        drop(current);

        // Legacy schema: no trigram sibling field, so it must never be stamped.
        let old_dir = temp_dir("heal-legacy");
        let mut b = tantivy::schema::Schema::builder();
        b.add_text_field("subject", tantivy::schema::TEXT);
        let legacy = tantivy::Index::create_in_dir(&old_dir, b.build()).unwrap();
        assert!(
            !heal_index_format_marker(&old_dir),
            "a legacy index must not be stamped as current"
        );
        assert_eq!(read_index_format_version(&old_dir), None);
        drop(legacy);
    }

    /// A v2 index carries the trigram siblings but no `imap_uid`. It must NOT be
    /// healed: stamping it would let the startup gate through and every later
    /// read would hit a missing required field.
    #[test]
    fn v2_schema_is_never_healed() {
        let dir = temp_dir("heal-v2");
        let mut b = tantivy::schema::Schema::builder();
        b.add_text_field("subject", tantivy::schema::TEXT);
        b.add_text_field(
            crate::store::tantivy::fields::F_SUBJECT_TRI,
            tantivy::schema::TEXT,
        );
        let v2 = tantivy::Index::create_in_dir(&dir, b.build()).unwrap();

        assert!(
            !heal_index_format_marker(&dir),
            "a v2 index (trigram siblings, no imap_uid) must not be stamped"
        );
        assert_eq!(read_index_format_version(&dir), None);
        drop(v2);
    }

    /// A schema that carries `imap_uid` but not `cleanup_moved` must not be
    /// healed. Both fields belong to generation 3; testing only for the older
    /// one would stamp such an index as current and every cleanup query would
    /// then fail on the missing field.
    #[test]
    fn an_index_without_the_cleanup_flag_is_never_healed() {
        let dir = temp_dir("heal-no-cleanup-flag");
        let mut b = tantivy::schema::Schema::builder();
        b.add_u64_field(
            crate::store::tantivy::fields::F_IMAP_UID,
            tantivy::schema::INDEXED | tantivy::schema::STORED,
        );
        let partial = tantivy::Index::create_in_dir(&dir, b.build()).unwrap();

        assert!(
            !heal_index_format_marker(&dir),
            "an index with imap_uid but no cleanup_moved must not be stamped"
        );
        assert_eq!(read_index_format_version(&dir), None);
        drop(partial);
    }
}
