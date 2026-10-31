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

use crate::context::Initialize;
use crate::migrate::{write_storage_version, CURRENT_STORAGE_VERSION};
use crate::settings::cli::SETTINGS;
use crate::{
    error::{code::ErrorCode, BichonResult},
    raise_error,
};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub(crate) const MEMDB_DIR: &str = "memdb";
const INDICES: &str = "bichon-indices";
const MAIL_METADATA: &str = "mail_metadata";
const ATTACHMENT_METADATA: &str = "attachment_metadata";
const STORAGE: &str = "bichon-storage";
const TMP_DIR: &str = "tmp";
const LOG_DIR: &str = "logs";
const EXPORTS_DIR: &str = "exports";
const BACKUP_STAGING: &str = "backup-staging";

const TLS_CERT: &str = "cert.pem";
const TLS_KEY: &str = "key.pem";

pub static DATA_DIR_MANAGER: LazyLock<DataDirManager> =
    LazyLock::new(|| DataDirManager::new(PathBuf::from(SETTINGS.root_dir())));

#[derive(Debug)]
pub struct DataDirManager {
    pub root_dir: PathBuf,
    /// Root of the search indices (`bichon-indices`), either under the data
    /// root or under `bichon_index_dir` when the operator overrides it.
    pub index_dir: PathBuf,
    pub memdb_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub envelope_dir: PathBuf,
    pub attachment_dir: PathBuf,
    pub storage_dir: PathBuf,
    pub log_dir: PathBuf,
    pub exports_dir: PathBuf,
    /// Scratch space inside the data root used by Pro/Enterprise backup
    /// preparers to stage online snapshots (e.g. `VACUUM INTO` copies) that
    /// must be included in the backup. Lives under the data root so
    /// it is backed up automatically and cleaned by `finalize`.
    pub backup_staging_dir: PathBuf,
}

impl Initialize for DataDirManager {
    async fn initialize() -> BichonResult<()> {
        std::fs::create_dir_all(&DATA_DIR_MANAGER.root_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        std::fs::create_dir_all(&DATA_DIR_MANAGER.log_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        std::fs::create_dir_all(&DATA_DIR_MANAGER.temp_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        std::fs::create_dir_all(&DATA_DIR_MANAGER.storage_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        std::fs::create_dir_all(&DATA_DIR_MANAGER.exports_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        std::fs::create_dir_all(&DATA_DIR_MANAGER.backup_staging_dir)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Write STORAGE_VERSION on fresh install (no existing data)
        let version_path = DATA_DIR_MANAGER.root_dir.join("STORAGE_VERSION");
        if !version_path.exists() && !DATA_DIR_MANAGER.storage_dir.join("blobs").exists() {
            write_storage_version(&DATA_DIR_MANAGER.root_dir, CURRENT_STORAGE_VERSION)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }

        Ok(())
    }
}

impl DataDirManager {
    /// The blob store directory. Everything inside it counts as archived
    /// email/attachment storage (segments plus the blob index/metadata), per
    /// the product definition of the dashboard's storage figure.
    pub fn blob_dir(&self) -> PathBuf {
        self.storage_dir.join("blobs")
    }

    pub fn new(root_dir: PathBuf) -> Self {
        let index_dir = if let Some(ref index_dir) = SETTINGS.bichon_index_dir {
            PathBuf::from(index_dir).join(INDICES)
        } else {
            root_dir.join(INDICES)
        };

        let storage_dir = if let Some(ref data_dir) = SETTINGS.bichon_data_dir {
            PathBuf::from(data_dir).join(STORAGE)
        } else {
            root_dir.join(STORAGE)
        };

        Self {
            root_dir: root_dir.clone(),
            index_dir: index_dir.clone(),
            memdb_dir: root_dir.join(MEMDB_DIR),
            tls_key: root_dir.join(TLS_KEY),
            tls_cert: root_dir.join(TLS_CERT),
            log_dir: root_dir.join(LOG_DIR),
            envelope_dir: index_dir.join(MAIL_METADATA),
            attachment_dir: index_dir.join(ATTACHMENT_METADATA),
            temp_dir: root_dir.join(TMP_DIR),
            storage_dir,
            exports_dir: root_dir.join(EXPORTS_DIR),
            backup_staging_dir: root_dir.join(BACKUP_STAGING),
        }
    }
}

/// Resolved index / data directories, shared with admin commands that do not
/// parse `Settings` so they see exactly the same layout as [`DataDirManager`].
#[derive(Debug, Clone)]
pub struct ResolvedDirs {
    /// `<index_parent>/bichon-indices` — the root that holds both indexes.
    pub index_root: PathBuf,
    pub envelope_dir: PathBuf,
    pub attachment_dir: PathBuf,
    pub blobs_dir: PathBuf,
}

/// Resolve the index and blob directories from a root plus optional index/data
/// parent directories.
///
/// Uses the same constants as [`DataDirManager::new`] so admin and server never
/// disagree on the layout. `index_parent` / `data_parent` fall back to `root`.
pub fn resolve_dirs(
    root: &Path,
    index_parent: Option<&Path>,
    data_parent: Option<&Path>,
) -> ResolvedDirs {
    let index_dir = index_parent
        .map(|p| append_default_leaf(p, INDICES))
        .unwrap_or_else(|| root.join(INDICES));
    let storage_dir = data_parent
        .map(|p| append_default_leaf(p, STORAGE))
        .unwrap_or_else(|| root.join(STORAGE));

    ResolvedDirs {
        envelope_dir: index_dir.join(MAIL_METADATA),
        attachment_dir: index_dir.join(ATTACHMENT_METADATA),
        blobs_dir: storage_dir.join("blobs"),
        index_root: index_dir,
    }
}

/// Append `leaf` to `parent` unless `parent` already names it. Operators
/// routinely paste the resolved directory shown by the prompts
/// (`<root>/bichon-indices`, `<root>/bichon-storage`) instead of the parent,
/// and appending twice would silently look at (or create) an empty directory.
fn append_default_leaf(parent: &Path, leaf: &str) -> PathBuf {
    match parent.file_name().and_then(|name| name.to_str()) {
        Some(name) if name.eq_ignore_ascii_case(leaf) => parent.to_path_buf(),
        _ => parent.join(leaf),
    }
}

/// Free bytes on the mount point holding `dir`; `u64::MAX` (optimistic) when the
/// mount point cannot be located. Mirrors `import::check_temp_disk_space`.
pub fn available_space(dir: &Path) -> u64 {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for disk in disks.list() {
        if canonical.starts_with(disk.mount_point()) {
            return disk.available_space();
        }
    }
    u64::MAX
}

/// Resolve the on-disk path for a feature-owned embedded database under its own
/// subdirectory, migrating a pre-existing root-level database (plus any live
/// sidecars) there on first use.
///
/// Embedded stores are not single files — SQLite in WAL mode maintains `-wal` /
/// `-shm` sidecars and redb memory-maps its file — so dropping them straight
/// into the data root clutters the directory and risks treating the sidecars as
/// loose files. Every feature database gets a subdirectory instead:
///
///   <root>/audit/audit.db          (Pro audit log)
///   <root>/integrity/integrity.db  (Pro integrity checker)
///   <root>/timestamp/anchor.db     (Pro timestamp anchoring)
///   <root>/imap-uid/imap-uid.redb  (IMAP stable-UID store)
///
/// Shared by the community IMAP UID store and the Pro feature databases (see
/// `crates/bichon-pro/src/db_paths.rs`).
pub fn feature_db_path(feature: &str, file_name: &str) -> PathBuf {
    let root = DATA_DIR_MANAGER.root_dir.clone();
    migrate_from_root(&root, &root.join(feature), file_name);
    root.join(feature).join(file_name)
}

/// Move `<root>/<file>` and any live sidecars into `dir` once, when the new
/// location does not exist yet. Idempotent and best-effort: failures are
/// ignored so a locked or already-moved file can never block opening the
/// database.
fn migrate_from_root(root: &Path, dir: &Path, file_name: &str) {
    if dir.join(file_name).exists() || !root.join(file_name).exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let old = root.join(format!("{file_name}{suffix}"));
        if old.exists() {
            let _ = std::fs::rename(old, dir.join(format!("{file_name}{suffix}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(suffix: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bichon-core-dbpaths-{suffix}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn migrate_from_root_moves_db_and_sidecars_once() {
        let root = tmp_root("audit");
        let dir = root.join("audit");
        std::fs::write(root.join("audit.db"), "db").unwrap();
        std::fs::write(root.join("audit.db-wal"), "wal").unwrap();

        migrate_from_root(&root, &dir, "audit.db");

        assert_eq!(std::fs::read_to_string(dir.join("audit.db")).unwrap(), "db");
        assert_eq!(
            std::fs::read_to_string(dir.join("audit.db-wal")).unwrap(),
            "wal"
        );
        assert!(!root.join("audit.db").exists());
        assert!(!root.join("audit.db-wal").exists());

        // Idempotent — a second call neither errors nor duplicates.
        migrate_from_root(&root, &dir, "audit.db");
        assert_eq!(std::fs::read_to_string(dir.join("audit.db")).unwrap(), "db");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_dirs_accepts_parent_or_already_resolved_dir() {
        let root = Path::new("/data");
        let as_parent = resolve_dirs(root, Some(Path::new("/ssd")), Some(Path::new("/hdd")));
        assert_eq!(
            as_parent.index_root,
            Path::new("/ssd").join("bichon-indices")
        );
        assert_eq!(
            as_parent.blobs_dir,
            Path::new("/hdd").join("bichon-storage").join("blobs")
        );

        let index = Path::new("/ssd").join("bichon-indices");
        let storage = Path::new("/hdd").join("bichon-storage");
        let as_dir = resolve_dirs(root, Some(&index), Some(&storage));
        assert_eq!(as_dir.index_root, as_parent.index_root);
        assert_eq!(as_dir.blobs_dir, as_parent.blobs_dir);
    }

    #[test]
    fn migrate_from_root_handles_redb_single_file() {
        let root = tmp_root("uid");
        let dir = root.join("imap-uid");
        std::fs::write(root.join("imap-uid.redb"), "store").unwrap();

        migrate_from_root(&root, &dir, "imap-uid.redb");

        assert_eq!(
            std::fs::read_to_string(dir.join("imap-uid.redb")).unwrap(),
            "store"
        );
        assert!(!root.join("imap-uid.redb").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_from_root_is_noop_when_new_already_exists() {
        let root = tmp_root("existing");
        std::fs::create_dir_all(root.join("integrity")).unwrap();
        std::fs::write(root.join("integrity").join("integrity.db"), "new").unwrap();
        std::fs::write(root.join("integrity.db"), "old").unwrap();

        migrate_from_root(&root, &root.join("integrity"), "integrity.db");

        // The already-present new file wins; the stale root-level file is
        // left alone so a partial previous migration is never overwritten.
        assert_eq!(
            std::fs::read_to_string(root.join("integrity").join("integrity.db")).unwrap(),
            "new"
        );
        assert!(root.join("integrity.db").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
