use std::path::PathBuf;

use bichon_blob::{Codec, Config, Engine};
use bichon_core::{
    error::{code::ErrorCode, BichonResult},
    migrate::write_storage_version,
    raise_error,
};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use fjall::{Config as FjallConfig, Database};
use indicatif::{ProgressBar, ProgressStyle};

fn hex_key_to_raw(hex_bytes: &[u8]) -> BichonResult<[u8; 32]> {
    let hex_str = std::str::from_utf8(hex_bytes).map_err(|e| {
        raise_error!(
            format!("invalid UTF-8 in fjall key: {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    let mut raw = [0u8; 32];
    hex::decode_to_slice(hex_str, &mut raw).map_err(|e| {
        raise_error!(
            format!("invalid hex in fjall key '{hex_str}': {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    Ok(raw)
}

fn migrate_keyspace(
    engine: &Engine,
    db: &Database,
    ks_name: &str,
    label: &str,
    batch_size: usize,
) -> BichonResult<u64> {
    let ks = db
        .keyspace(ks_name, || {
            panic!("{ks_name} keyspace not found in fjall database")
        })
        .map_err(|e| {
            raise_error!(
                format!("failed to open fjall keyspace '{ks_name}': {e:#?}"),
                ErrorCode::InternalError
            )
        })?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} [{elapsed_precise}]").unwrap(),
    );
    pb.set_message(format!("Scanning {label} blobs..."));

    let mut count: u64 = 0;
    let mut batch: Vec<([u8; 32], Vec<u8>, Codec)> = Vec::with_capacity(batch_size);

    for item in ks.iter() {
        let (key_bytes, value) = item.into_inner().map_err(|e| {
            raise_error!(
                format!("fjall iter error in '{ks_name}': {e:#?}"),
                ErrorCode::InternalError
            )
        })?;

        if value.is_empty() {
            continue;
        }
        // MAX_VALUE_SIZE = 100 MB (bichon_blob::types)
        if value.len() > 100 * 1024 * 1024 {
            let raw_key = hex_key_to_raw(&key_bytes)?;
            eprintln!(
                "{}",
                console::style(format!(
                    "WARN: skipping oversized blob key={} ({} bytes)",
                    hex::encode(raw_key),
                    value.len()
                ))
                .yellow()
            );
            continue;
        }
        let raw_key = hex_key_to_raw(&key_bytes)?;
        batch.push((raw_key, value.to_vec(), Codec::Zstd));

        if batch.len() >= batch_size {
            engine
                .put_batch(&batch)
                .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
            count += batch.len() as u64;
            pb.set_message(format!("{label}: {} blobs migrated...", count));
            batch.clear();
        }
    }

    if !batch.is_empty() {
        engine
            .put_batch(&batch)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
        count += batch.len() as u64;
    }

    pb.finish_with_message(format!("{label}: {} blobs migrated", count));
    Ok(count)
}

pub fn handle_migrate_v1(theme: &ColorfulTheme) {
    println!(
        "\n{}",
        style("MIGRATION: Bichon v1.x Storage → v2.x (Fjall → bichon-blob)")
            .bold()
            .yellow()
    );
    println!(
        "{}\n",
        style(
            "This migrates blob storage from the fjall engine to bichon-blob.\n\
              Tantivy indexes and metadata (memdb) are NOT affected."
        )
        .dim()
    );

    let root_dir: String = Input::with_theme(theme)
        .with_prompt("Enter --bichon-root-dir (same value used by the old server)")
        .validate_with(|input: &String| -> Result<(), &str> {
            let path = PathBuf::from(input);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }
            if !path.exists() {
                return Err("Directory does not exist.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();
    let root_dir = PathBuf::from(root_dir.trim());

    let data_base = {
        let input: String = Input::with_theme(theme)
            .with_prompt("Enter --bichon-data-dir (leave blank to use root directory)")
            .allow_empty(true)
            .interact_text()
            .unwrap();
        if input.trim().is_empty() {
            root_dir.clone()
        } else {
            let path = PathBuf::from(input.trim());
            if !path.exists() {
                eprintln!(
                    "{}",
                    style(format!("Data directory does not exist: {}", path.display())).red()
                );
                return;
            }
            path
        }
    };

    let fjall_path = data_base.join("bichon-storage");
    let blob_path = fjall_path.join("blobs");

    if !fjall_path.exists() {
        println!(
            "{}",
            style(format!(
                "Fjall database not found at '{}'. Is this really a v1.x install?",
                fjall_path.display()
            ))
            .red()
        );
        return;
    }

    if blob_path.exists() {
        println!(
            "{}",
            style(format!(
                "Target blob directory '{}' already exists.\n\
                 If you have already migrated, you can remove the old fjall files manually.\n\
                 Otherwise, delete this directory and re-run the migration.",
                blob_path.display()
            ))
            .yellow()
        );
        return;
    }

    let batch_size: usize = {
        let input: String = Input::with_theme(theme)
            .with_prompt(
                "Enter batch size (affects memory usage, higher = faster but uses more RAM)",
            )
            .default("1000".to_string())
            .validate_with(|s: &String| match s.trim().parse::<usize>() {
                Ok(n) if n > 0 => Ok(()),
                _ => Err("Please enter a valid positive number"),
            })
            .interact_text()
            .unwrap_or("1000".to_string());
        input.trim().parse::<usize>().unwrap_or(1000)
    };

    println!(
        "{} Using batch size: {}\n",
        style("✓").green(),
        style(batch_size).cyan().bold()
    );

    // Open old Fjall database (read-only by nature of the iter API)
    println!("\n{}", style("Opening fjall database...").dim());
    let db = match Database::open(FjallConfig::new(&fjall_path)) {
        Ok(db) => db,
        Err(e) => {
            println!(
                "{}",
                style(format!("Failed to open fjall database: {e:#?}")).red()
            );
            return;
        }
    };

    // Open new bichon-blob engine
    println!("{}", style("Initializing bichon-blob engine...").dim());
    let mut config = Config::default();
    config.default_codec = Codec::Zstd;
    config.compress_threshold = 1024;
    config.flush_interval_secs = 0;
    config.gc_interval_secs = 0;

    let engine = match Engine::open(&blob_path, config) {
        Ok(e) => e,
        Err(e) => {
            println!(
                "{}",
                style(format!("Failed to open bichon-blob engine: {e:#?}")).red()
            );
            return;
        }
    };

    // Migrate email blobs
    let email_count = match migrate_keyspace(&engine, &db, "email", "Email", batch_size) {
        Ok(n) => n,
        Err(e) => {
            println!("{}", style(format!("Email migration failed: {e:#?}")).red());
            let _ = engine.shutdown();
            return;
        }
    };

    // Migrate attachment blobs
    let attach_count = match migrate_keyspace(&engine, &db, "attachments", "Attachment", batch_size)
    {
        Ok(n) => n,
        Err(e) => {
            println!(
                "{}",
                style(format!("Attachment migration failed: {e:#?}")).red()
            );
            let _ = engine.shutdown();
            return;
        }
    };

    // Flush and shutdown
    println!(
        "\n{}",
        style("Flushing and shutting down blob engine...").dim()
    );
    if let Err(e) = engine.flush() {
        println!("{}", style(format!("flush warning: {e:#?}")).yellow());
    }
    if let Err(e) = engine.shutdown() {
        println!("{}", style(format!("shutdown error: {e:#?}")).red());
        return;
    }

    // Write STORAGE_VERSION = 2
    if let Err(e) = write_storage_version(&root_dir, 2) {
        println!(
            "{}",
            style(format!("Failed to write STORAGE_VERSION: {e:#?}")).red()
        );
        return;
    }

    // The 2.x search analyzer differs from the one that built the v1.x indexes,
    // and this migration leaves those indexes untouched. Offer the one-time
    // rebuild now, so the operator does not have to go through a second
    // maintenance stop; declining keeps the indexes intact and the warning below
    // tells them what to run later.
    let want_rebuild = Confirm::with_theme(theme)
        .with_prompt(
            "The search analyzer changed, so the indexes must be rebuilt once. Rebuild the search indexes now?",
        )
        .default(true)
        .interact()
        .unwrap_or(false);

    let rebuilt = want_rebuild
        && crate::reindex::run_without_confirm(crate::reindex::ReindexArgs {
            root: Some(root_dir.to_string_lossy().into_owned()),
            // Same env names the server reads, so a customized index directory
            // is picked up automatically. The data parent is the one just
            // answered above (or the root when left blank) — using the env
            // fallback here would ignore a directory the operator typed.
            index_parent: std::env::var("BICHON_INDEX_DIR").ok(),
            data_parent: Some(data_base.to_string_lossy().into_owned()),
            ..Default::default()
        }) == 0;

    println!(
        "\n{}",
        style(format!(
            "✅ Migration complete!\n\
         \n\
         📊 {} email blobs, {} attachment blobs migrated\n\
         \n\
         📖 **Next steps:**\n\
         Refer to the official migration guide for:\n\
         • How to verify the new storage\n\
         • Cleanup commands for legacy files\n\
         • Rollback instructions if needed\n\
         \n\
         🔗 {}\n\
         \n\
         ⚠️  **Important:** Old data is preserved until you manually remove it.\n\
         Do not delete anything until you have verified the new server works correctly.",
            email_count,
            attach_count,
            "https://github.com/rustmailer/bichon/wiki/Bichon-v2.x-Migration-Guide"
        ))
        .green()
        .bold()
    );

    if !rebuilt {
        println!(
            "\n{}",
            style(if want_rebuild {
                "⚠️  The search index rebuild did not finish. Run `bichon-admin reindex` before starting the server, or search will not work."
            } else {
                "🔁 Search indexes still need a one-time rebuild. Run `bichon-admin reindex` before starting the server, otherwise the server will ask you to."
            })
            .yellow()
            .bold()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bichon_core::utils::compute_content_hash;

    // ── hex_key_to_raw ────────────────────────────────────────────────

    #[test]
    fn hex_key_to_raw_valid() {
        // "hello" blake3 hex = 64 chars
        let hash_hex = compute_content_hash(b"hello");
        assert_eq!(hash_hex.len(), 64);

        let raw = hex_key_to_raw(hash_hex.as_bytes()).unwrap();
        // Decoding 64 hex chars → 32 bytes
        assert_eq!(raw.len(), 32);
        // Round-trip: raw → hex should match original
        assert_eq!(hex::encode(raw), hash_hex);
    }

    #[test]
    fn hex_key_to_raw_invalid_utf8() {
        // 0xFF is not valid UTF-8
        let invalid = vec![0xFFu8; 64];
        let err = hex_key_to_raw(&invalid).unwrap_err();
        assert!(err.to_string().contains("invalid UTF-8"));
    }

    #[test]
    fn hex_key_to_raw_invalid_hex() {
        // "zz" is valid UTF-8 but not valid hex
        let invalid = b"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        let err = hex_key_to_raw(invalid).unwrap_err();
        assert!(err.to_string().contains("invalid hex"));
    }

    #[test]
    fn hex_key_to_raw_wrong_length() {
        let short = b"abcd";
        let err = hex_key_to_raw(short).unwrap_err();
        assert!(err.to_string().contains("invalid hex"));
    }

    #[test]
    fn hex_key_to_raw_different_content() {
        let a = hex_key_to_raw(compute_content_hash(b"a").as_bytes()).unwrap();
        let b = hex_key_to_raw(compute_content_hash(b"b").as_bytes()).unwrap();
        assert_ne!(a, b);
    }

    // ── migrate_keyspace integration ──────────────────────────────────

    #[test]
    fn migrate_keyspace_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let fjall_path = tmp.path().join("fjall");
        let blob_path = tmp.path().join("blobs");

        use fjall::KeyspaceCreateOptions;

        // --- Setup: create a Fjall database with test blobs ---
        let fjall_db = Database::open(FjallConfig::new(&fjall_path)).unwrap();
        let ks = fjall_db
            .keyspace("test_ks", KeyspaceCreateOptions::default)
            .unwrap();

        // Insert test blobs using hex string keys (matching v1.x convention)
        let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..10 {
            let data = format!("blob data {}", i).into_bytes();
            let hash = compute_content_hash(&data);
            ks.insert(hash.as_bytes(), data.clone()).unwrap();
            expected.push((hash, data));
        }

        // --- Setup: create bichon-blob engine and run migration ---
        {
            let mut config = Config::default();
            config.flush_interval_secs = 0;
            config.gc_interval_secs = 0;
            let engine = Engine::open(&blob_path, config).unwrap();

            let count = migrate_keyspace(&engine, &fjall_db, "test_ks", "Test", 100).unwrap();
            assert_eq!(count, expected.len() as u64);

            engine.flush().unwrap();
            engine.shutdown().unwrap();
            // engine dropped here → LOCK released
        }

        // --- Verify: re-open engine and check all blobs ---
        let mut config = Config::default();
        config.flush_interval_secs = 0;
        config.gc_interval_secs = 0;
        let engine2 = Engine::open(&blob_path, config).unwrap();

        for (hex_hash, expected_data) in &expected {
            let mut raw_key = [0u8; 32];
            hex::decode_to_slice(hex_hash, &mut raw_key).unwrap();
            let got = engine2.get(&raw_key).unwrap();
            assert_eq!(
                got.as_deref(),
                Some(expected_data.as_slice()),
                "mismatch for key {}",
                hex_hash
            );
        }

        // Verify non-existent key returns None
        let fake_hash = compute_content_hash(b"nonexistent");
        let mut fake_key = [0u8; 32];
        hex::decode_to_slice(&fake_hash, &mut fake_key).unwrap();
        assert!(engine2.get(&fake_key).unwrap().is_none());

        engine2.shutdown().unwrap();
    }

    #[test]
    fn migrate_empty_keyspace() {
        let tmp = tempfile::tempdir().unwrap();
        let fjall_path = tmp.path().join("fjall");
        let blob_path = tmp.path().join("blobs");

        let fjall_db = Database::open(FjallConfig::new(&fjall_path)).unwrap();
        let _ks = fjall_db
            .keyspace("empty_ks", fjall::KeyspaceCreateOptions::default)
            .unwrap();

        let mut config = Config::default();
        config.flush_interval_secs = 0;
        config.gc_interval_secs = 0;
        let engine = Engine::open(&blob_path, config).unwrap();

        let count = migrate_keyspace(&engine, &fjall_db, "empty_ks", "Empty", 100).unwrap();
        assert_eq!(count, 0);

        engine.shutdown().unwrap();
    }

    #[test]
    fn hex_key_roundtrip_with_real_content_hash() {
        // Simulate the exact data flow from v1.x to v2.x
        let eml_data = b"From: sender@example.com\r\nSubject: Test\r\n\r\nHello world";
        let hash_hex = compute_content_hash(eml_data); // 64-char hex string

        // v1.x: key stored as hash_hex.as_bytes()
        let fjall_key = hash_hex.as_bytes().to_vec();
        assert_eq!(fjall_key.len(), 64);

        // Migration: hex decode → raw 32 bytes
        let raw_key = hex_key_to_raw(&fjall_key).unwrap();
        assert_eq!(raw_key.len(), 32);

        // v2.x: engine.put(raw_key, data)
        // Verify round-trip: raw_key → hex → compare
        let hex_roundtrip = hex::encode(raw_key);
        assert_eq!(hex_roundtrip, hash_hex);
    }
}
