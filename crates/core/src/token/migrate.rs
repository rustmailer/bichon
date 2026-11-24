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

//! One-way migration from plaintext bearer tokens to digests.
//!
//! Runs at database open, before any listener starts, and rewrites every
//! legacy record in a single memdb transaction. The operation is idempotent:
//! records that already carry a digest are skipped, so a crash mid-way simply
//! resumes on the next start.
//!
//! Records restored from an old backup are plaintext again and take the same
//! path. Downgrading to a build that predates this migration is not supported:
//! those builds can read the snapshot but the stored digest is not a usable
//! credential.

use bichon_memdb::MemDb;

use crate::database::{list_all_impl, with_transaction_exempt, MemDbModel};
use crate::error::{code::ErrorCode, BichonResult};
use crate::raise_error;
use crate::token::{
    digest_token, token_id_from_digest, token_prefix, AccessTokenModel, TokenScheme, TokenType,
};

/// Rewrite every plaintext token record so only a digest remains on disk.
///
/// MFA challenges are dropped instead: they live for five minutes and a user
/// in the middle of a login can simply retry.
pub fn migrate_legacy_tokens(db: &MemDb) -> BichonResult<()> {
    let all: Vec<AccessTokenModel> = list_all_impl(db)?;

    let mut upgrades = Vec::new();
    let mut challenges = Vec::new();
    for model in all {
        if model.token_scheme != TokenScheme::Plaintext {
            continue;
        }
        if model.token_type == TokenType::MfaChallenge {
            challenges.push(model.key());
        } else {
            upgrades.push(model);
        }
    }

    if upgrades.is_empty() && challenges.is_empty() {
        return Ok(());
    }

    with_transaction_exempt(db, move |txn| {
        let mut txn = txn;
        for key in challenges {
            txn = txn.delete(AccessTokenModel::collection(), key);
        }
        for model in upgrades {
            let old_key = model.key();
            let upgraded = upgrade(model);
            txn = txn
                .delete(AccessTokenModel::collection(), old_key)
                .insert(
                    AccessTokenModel::collection(),
                    upgraded.key(),
                    &upgraded,
                )
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        Ok(txn)
    })?;

    // The transaction's WAL entry still names the old plaintext keys, and the
    // previous snapshot still contains them. Rewrite both now so the goal —
    // no usable token on disk — is met immediately, not five minutes later.
    db.snapshot()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}

/// Migrate a single record found on the lazy path. Concurrent callers race
/// harmlessly: both compute the same digest, prefix and id, and the second
/// transaction rewrites the same key with the same value.
pub(crate) fn migrate_record_in(
    db: &MemDb,
    model: &AccessTokenModel,
) -> BichonResult<AccessTokenModel> {
    if model.token_scheme != TokenScheme::Plaintext {
        return Ok(model.clone());
    }

    let old_key = model.key();
    let upgraded = upgrade(model.clone());
    let share = upgraded.clone();
    let new_key = upgraded.key();
    with_transaction_exempt(db, move |txn| {
        txn.delete(AccessTokenModel::collection(), old_key)
            .insert(AccessTokenModel::collection(), new_key, &share)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    })?;
    Ok(upgraded)
}

/// Replace the plaintext material with its digest. The plaintext must be the
/// value stored in `token_material`; anything else would produce a record the
/// user's token cannot unlock.
fn upgrade(mut model: AccessTokenModel) -> AccessTokenModel {
    let plaintext = model.token_material;
    let digest = digest_token(&plaintext);
    model.token_prefix = token_prefix(&plaintext);
    model.token_id = token_id_from_digest(&digest);
    model.token_material = digest;
    model.token_scheme = TokenScheme::Blake3;
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{should_refresh_last_access, TokenScheme};

    fn legacy(user_id: u64, token: &str, token_type: TokenType) -> AccessTokenModel {
        // Serialize the pre-hashing shape and deserialize it, so the test also
        // pins the wire-compatibility contract (missing fields -> defaults).
        let json = serde_json::json!({
            "user_id": user_id,
            "token": token,
            "name": "legacy",
            "token_type": token_type,
            "created_at": 1,
            "updated_at": 1,
            "expire_at": null,
            "last_access_at": 0
        });
        serde_json::from_value(json).expect("legacy record must deserialize")
    }

    fn insert(db: &MemDb, model: &AccessTokenModel) {
        db.collection(AccessTokenModel::collection())
            .insert(model.key(), model)
            .unwrap();
    }

    fn all(db: &MemDb) -> Vec<AccessTokenModel> {
        db.collection(AccessTokenModel::collection())
            .list_all()
            .unwrap()
    }

    #[test]
    fn legacy_records_deserialize_as_plaintext() {
        let model = legacy(1, "legacy-token", TokenType::Api);
        assert_eq!(model.token_scheme, TokenScheme::Plaintext);
        assert_eq!(model.token_material, "legacy-token");
        assert!(model.token_prefix.is_empty());
        assert!(model.token_id.is_empty());
    }

    #[test]
    fn eager_migration_rewrites_api_and_webui_tokens() {
        let db = MemDb::in_memory();
        insert(&db, &legacy(1, "api-secret-123456", TokenType::Api));
        insert(&db, &legacy(2, "webui-secret-7890", TokenType::WebUI));

        migrate_legacy_tokens(&db).unwrap();

        let rows = all(&db);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.token_scheme, TokenScheme::Blake3);
            assert!(!row.token_material.contains("secret"));
            assert_eq!(
                row.token_material,
                digest_token(if row.user_id == 1 {
                    "api-secret-123456"
                } else {
                    "webui-secret-7890"
                })
            );
            assert!(!row.token_id.is_empty());
            assert!(!row.token_prefix.is_empty());
        }

        // The legacy plaintext key is gone.
        let coll = db.collection(AccessTokenModel::collection());
        assert!(coll
            .get::<AccessTokenModel>("api-secret-123456")
            .unwrap()
            .is_none());
    }

    #[test]
    fn migration_is_idempotent() {
        let db = MemDb::in_memory();
        insert(&db, &legacy(1, "only-once-123456", TokenType::Api));

        migrate_legacy_tokens(&db).unwrap();
        let after_first = all(&db);
        migrate_legacy_tokens(&db).unwrap();
        let after_second = all(&db);

        assert_eq!(after_first, after_second);
        assert_eq!(after_second.len(), 1);
    }

    #[test]
    fn mfa_challenges_are_dropped_not_migrated() {
        let db = MemDb::in_memory();
        insert(&db, &legacy(1, "challenge-123456", TokenType::MfaChallenge));
        insert(&db, &legacy(1, "api-secret-123456", TokenType::Api));

        migrate_legacy_tokens(&db).unwrap();

        let rows = all(&db);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].token_type, TokenType::Api);
    }

    #[test]
    fn lazy_lookup_migrates_and_still_authenticates() {
        let db = MemDb::in_memory();
        insert(&db, &legacy(1, "lazy-secret-123456", TokenType::WebUI));

        let found = AccessTokenModel::find_by_secret_in(&db, "lazy-secret-123456")
            .unwrap()
            .expect("the legacy token must resolve");
        assert_eq!(found.user_id, 1);
        assert_eq!(found.token_scheme, TokenScheme::Blake3);

        // Subsequent lookups hit the digest key directly.
        let again = AccessTokenModel::find_by_secret_in(&db, "lazy-secret-123456")
            .unwrap()
            .expect("the migrated token must resolve");
        assert_eq!(again.token_material, found.token_material);
        assert!(AccessTokenModel::find_by_secret_in(&db, "wrong")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_stored_digest_is_not_a_credential() {
        let db = MemDb::in_memory();
        insert(&db, &legacy(1, "real-secret-123456", TokenType::Api));
        migrate_legacy_tokens(&db).unwrap();

        let digest = all(&db)[0].token_material.clone();
        assert!(
            AccessTokenModel::find_by_secret_in(&db, &digest)
                .unwrap()
                .is_none(),
            "a digest read off the disk must not authenticate"
        );
        assert!(AccessTokenModel::find_by_secret_in(&db, "real-secret-123456")
            .unwrap()
            .is_some());
    }

    #[test]
    fn new_tokens_store_only_a_digest() {
        let model = AccessTokenModel::new_api_token(
            "fresh-secret-123456".to_string(),
            1,
            None,
            None,
            false,
        );
        assert_eq!(model.token_scheme, TokenScheme::Blake3);
        assert_ne!(model.token_material, "fresh-secret-123456");
        assert_eq!(model.token_material.len(), 64);
        assert_eq!(model.token_prefix, "fresh-secret");
        assert!(!model.token_id.is_empty());
    }

    #[test]
    fn startup_migration_removes_plaintext_from_disk() {
        use bichon_memdb::Durability;

        let unique = format!(
            "bichon-token-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();

        // The pre-upgrade snapshot exactly as an old binary wrote it.
        let legacy_json = serde_json::json!({
            "user_id": 1,
            "token": "disk-secret-123456",
            "name": "legacy",
            "token_type": "Api",
            "created_at": 1,
            "updated_at": 1,
            "expire_at": null,
            "last_access_at": 0
        });
        let snapshot = serde_json::json!({
            "last_seq": 0,
            "data": { "tokens": { "disk-secret-123456": legacy_json } }
        });
        std::fs::write(
            dir.join("snapshot.json"),
            serde_json::to_vec_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let db = bichon_memdb::MemDb::open_with(&dir, Durability::Batch { max_ops: 100 }).unwrap();
        migrate_legacy_tokens(&db).unwrap();

        let on_disk = std::fs::read_to_string(dir.join("snapshot.json")).unwrap();
        assert!(
            !on_disk.contains("disk-secret-123456"),
            "the snapshot must not retain the plaintext token: {on_disk}"
        );
        assert!(on_disk.contains("blake3"));
        let wal = std::fs::read_to_string(dir.join("wal.jsonl")).unwrap_or_default();
        assert!(
            !wal.contains("disk-secret-123456"),
            "the WAL must not retain the legacy key after the post-migration snapshot"
        );

        // A restart picks up the rewritten snapshot and the token still works.
        drop(db);
        let reopened = bichon_memdb::MemDb::open_with(&dir, Durability::Off).unwrap();
        assert!(AccessTokenModel::find_by_secret_in(&reopened, "disk-secret-123456")
            .unwrap()
            .is_some());
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_access_is_refreshed_at_most_once_a_minute() {
        assert!(should_refresh_last_access(0, 1_000));
        assert!(!should_refresh_last_access(1_000, 1_001));
        assert!(!should_refresh_last_access(1_000, 60_999));
        assert!(should_refresh_last_access(1_000, 61_000));
    }
}
