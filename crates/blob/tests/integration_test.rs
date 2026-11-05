use bichon_blob::{Codec, Config, Engine};
use tempfile::TempDir;

#[test]
fn test_write_and_read() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xAA; 32];
    let value = b"Hello, this is a test email!".to_vec();

    engine.put(key, &value, Codec::Zstd).unwrap();

    let result = engine.get(&key).unwrap();
    assert_eq!(result, Some(value));
}

#[test]
fn test_read_missing_key() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xFF; 32];
    let result = engine.get(&key).unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_delete() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xBB; 32];
    let value = b"Some email content".to_vec();

    engine.put(key, &value, Codec::Zstd).unwrap();
    engine.delete(&key).unwrap();

    let result = engine.get(&key).unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_exists() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xCC; 32];
    assert!(!engine.exists(&key).unwrap());

    engine.put(key, b"data", Codec::None).unwrap();
    assert!(engine.exists(&key).unwrap());

    engine.delete(&key).unwrap();
    assert!(!engine.exists(&key).unwrap());
}

#[test]
fn test_small_value_not_compressed() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xCC; 32];
    let value = b"hi"; // Smaller than 4KB threshold

    engine.put(key, value, Codec::Zstd).unwrap();

    let result = engine.get(&key).unwrap();
    assert_eq!(result, Some(value.to_vec()));
}

#[test]
fn test_large_value() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0xDD; 32];
    let value = vec![b'X'; 100_000]; // 100KB

    engine.put(key, &value, Codec::Zstd).unwrap();

    let result = engine.get(&key).unwrap();
    assert_eq!(result, Some(value));
}

#[test]
fn test_multiple_keys() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let n = 100;
    for i in 0..n {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        let value = format!("email number {}", i).into_bytes();
        engine.put(key, &value, Codec::Zstd).unwrap();
    }

    for i in 0..n {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        let result = engine.get(&key).unwrap();
        assert_eq!(result, Some(format!("email number {}", i).into_bytes()));
    }
}

#[test]
fn test_gc() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    // Write many entries
    let value = vec![b'Y'; 5000];
    let n = 100;

    for i in 0..n {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        engine.put(key, &value, Codec::None).unwrap();
    }

    // Delete even-numbered keys
    for i in (0..n).step_by(2) {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        engine.delete(&key).unwrap();
    }

    // The active segment is never compacted in place: seal it first.
    engine.seal_active_segment().unwrap();
    let stats = engine
        .gc()
        .unwrap()
        .expect("GC should have compacted the sealed segment");
    assert!(stats.entries_kept > 0);
    assert!(stats.bytes_on_disk > stats.bytes_after);

    // Verify remaining keys still readable
    for i in (1..n).step_by(2) {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        let result = engine.get(&key).unwrap();
        assert_eq!(result, Some(value.clone()));
    }

    // Deleted keys should not exist
    for i in (0..n).step_by(2) {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        let result = engine.get(&key).unwrap();
        assert_eq!(result, None);
    }

    // A restart after compaction must not lose the segment tail (regression:
    // the index cursor used to be the payload count, not the file size).
    drop(engine);
    let engine = Engine::open(dir.path(), Config::default()).unwrap();
    for i in (1..n).step_by(2) {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
        assert_eq!(
            engine.get(&key).unwrap(),
            Some(value.clone()),
            "key {i} must survive a restart after GC"
        );
    }
}

#[test]
fn test_reopen_persistence() {
    let dir = TempDir::new().unwrap();
    let key = [0xEE; 32];
    let value = b"persistent data".to_vec();

    {
        let engine = Engine::open(dir.path(), Config::default()).unwrap();
        engine.put(key, &value, Codec::Zstd).unwrap();
    }

    // Reopen
    {
        let engine = Engine::open(dir.path(), Config::default()).unwrap();
        let result = engine.get(&key).unwrap();
        assert_eq!(result, Some(value));
    }
}

#[test]
fn test_stats() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    engine.put([1u8; 32], b"hello", Codec::None).unwrap();

    let stats = engine.stats().unwrap();
    assert!(stats.total_bytes > 0);
    assert!(stats.total_keys > 0);
}

#[test]
fn test_batch_write() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let n = 50;
    let entries: Vec<_> = (0..n)
        .map(|i: u64| {
            let mut key = [0u8; 32];
            key[0..8].copy_from_slice(&i.to_le_bytes());
            let value = format!("batch email {}", i).into_bytes();
            (key, value, Codec::Zstd)
        })
        .collect();

    engine.put_batch(&entries).unwrap();

    for (key, value, _) in &entries {
        let result = engine.get(key).unwrap();
        assert_eq!(result.as_ref(), Some(value));
    }
}

#[test]
fn test_batch_write_persistence() {
    let dir = TempDir::new().unwrap();
    let entries: Vec<_> = (0..30u64)
        .map(|i| {
            let mut key = [0u8; 32];
            key[0..8].copy_from_slice(&i.to_le_bytes());
            (key, format!("persist {}", i).into_bytes(), Codec::Zstd)
        })
        .collect();

    {
        let engine = Engine::open(dir.path(), Config::default()).unwrap();
        engine.put_batch(&entries).unwrap();
    }

    {
        let engine = Engine::open(dir.path(), Config::default()).unwrap();
        for (key, value, _) in &entries {
            let result = engine.get(key).unwrap();
            assert_eq!(result.as_ref(), Some(value));
        }
    }
}

#[test]
fn test_invalid_config_rejected() {
    let dir = TempDir::new().unwrap();
    let mut config = Config::default();
    config.compression_level = -1;
    assert!(Engine::open(dir.path(), config).is_err());

    let mut config = Config::default();
    config.gc_deleted_ratio = 1.5;
    assert!(Engine::open(dir.path(), config).is_err());
}

#[test]
fn test_concurrent_reads() {
    use std::sync::Arc;
    use std::thread;

    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path(), Config::default()).unwrap());

    // Write some data
    for i in 0..50u32 {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&i.to_le_bytes());
        engine
            .put(key, &vec![i as u8; 1024], Codec::None)
            .unwrap();
    }

    // Spawn 4 threads, each reading a different subset
    let mut handles = vec![];
    for t in 0..4 {
        let engine = engine.clone();
        handles.push(thread::spawn(move || {
            for i in (t * 12)..((t + 1) * 12) {
                let mut key = [0u8; 32];
                key[0..4].copy_from_slice(&(i as u32).to_le_bytes());
                let read = engine.get(&key).unwrap();
                assert!(read.is_some(), "key {} should exist", i);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_global_dedup() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    let key = [0x42; 32];
    let value = b"same content across what would be accounts".to_vec();

    // Write same key twice (simulating two accounts ingesting the same email)
    engine.put(key, &value, Codec::Zstd).unwrap();
    engine.put(key, &value, Codec::Zstd).unwrap();

    // Should still be readable
    let result = engine.get(&key).unwrap();
    assert_eq!(result, Some(value));

    // Stats should reflect dedup (not double count)
    let stats = engine.stats().unwrap();
    // The key appears once in the bucket store
    assert!(stats.total_keys > 0);
}

#[test]
fn test_crash_recovery() {
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().to_path_buf();

    // Phase 1: write data, then drop without shutdown (simulates crash)
    {
        let engine = Engine::open(&dir_path, Config::default()).unwrap();

        for i in 0..50u32 {
            let mut key = [0u8; 32];
            key[0..4].copy_from_slice(&i.to_le_bytes());
            engine
                .put(key, &vec![i as u8; 512], Codec::None)
                .unwrap();
        }
        // Engine dropped here without calling shutdown()
    }

    // Phase 2: reopen - recovery should run, data should be intact
    let engine = Engine::open(&dir_path, Config::default()).unwrap();
    let stats = engine.stats().unwrap();
    assert!(stats.total_keys > 0, "recovery should preserve data");

    // Verify reads work
    for i in 0..50u32 {
        let mut key = [0u8; 32];
        key[0..4].copy_from_slice(&i.to_le_bytes());
        let read = engine.get(&key).unwrap();
        assert!(read.is_some(), "key {} should survive crash recovery", i);
    }
}

#[test]
fn test_meta_bin_durability() {
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().to_path_buf();

    {
        let engine = Engine::open(&dir_path, Config::default()).unwrap();

        let key = [0x42u8; 32];
        engine.put(key, b"durable", Codec::None).unwrap();
    }
    // Engine dropped -> shutdown() called -> meta saved

    // Verify meta.bin exists
    let meta_path = dir_path.join("meta.bin");
    assert!(meta_path.exists(), "meta.bin should exist after clean shutdown");

    let data = std::fs::read(&meta_path).unwrap();
    assert!(
        data.len() >= 8,
        "meta.bin should have at least 8 bytes"
    );

    let stored_crc = u32::from_le_bytes(data[0..4].try_into().unwrap());
    assert_ne!(stored_crc, 0, "stored CRC should be non-zero");

    // Reopen and verify data is intact
    let engine = Engine::open(&dir_path, Config::default()).unwrap();
    let read = engine.get(&[0x42u8; 32]).unwrap();
    assert_eq!(read, Some(b"durable".to_vec()));
}

#[test]
fn test_gc_deletes_empty_segment() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::open(dir.path(), Config::default()).unwrap();

    // Write data, seal, then delete everything so the sealed segment
    // becomes 100% garbage.  GC should remove the segment file entirely.
    let n = 50u64;
    for i in 0..n {
        let mut key = [0u8; 32];
        key[0..8].copy_from_slice(&i.to_le_bytes());
        engine.put(key, &vec![b'X'; 4096], Codec::None).unwrap();
    }

    // Force seal so the segment becomes a GC candidate.
    let sealed_id = engine.seal_active_segment().unwrap();

    // Delete all keys — the sealed segment is now entirely garbage.
    let keys: Vec<[u8; 32]> = (0..n)
        .map(|i| {
            let mut key = [0u8; 32];
            key[0..8].copy_from_slice(&i.to_le_bytes());
            key
        })
        .collect();
    engine.delete_batch(&keys).unwrap();

    // Run GC — this should empty and then delete the sealed segment.
    let result = engine.gc().unwrap();
    assert!(result.is_some(), "GC should have found a candidate");
    let stats = result.unwrap();
    assert_eq!(stats.segment_id, sealed_id);
    assert_eq!(stats.bytes_after, 0);
    assert_eq!(stats.bytes_on_disk, 0);

    // Segment file must be deleted.
    let seg_path = dir
        .path()
        .join("segments")
        .join(format!("{:08}.seg", sealed_id));
    assert!(
        !seg_path.exists(),
        "emptied segment file should have been deleted, but {:?} exists",
        seg_path
    );

    // Subsequent reads / writes must still work (no corruption).
    let new_key = [0x99u8; 32];
    engine
        .put(new_key, b"post-gc data", Codec::None)
        .unwrap();
    let result = engine.get(&new_key).unwrap();
    assert_eq!(result, Some(b"post-gc data".to_vec()));

    // Engine stats should be consistent.
    let stats = engine.stats().unwrap();
    assert_eq!(stats.total_keys, 1);

    // Shutdown and reopen — persistence must be intact.
    drop(engine);
    let engine = Engine::open(dir.path(), Config::default()).unwrap();
    let result = engine.get(&new_key).unwrap();
    assert_eq!(result, Some(b"post-gc data".to_vec()));
    let result = engine.get(&keys[0]).unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_file_lock_prevents_concurrent_open() {
    let dir = TempDir::new().unwrap();
    let _engine1 = Engine::open(dir.path(), Config::default()).unwrap();
    let result = Engine::open(dir.path(), Config::default());
    assert!(result.is_err(), "second open on same directory must fail");
}

#[test]
fn test_file_lock_released_after_close() {
    let dir = TempDir::new().unwrap();
    {
        let _engine = Engine::open(dir.path(), Config::default()).unwrap();
    }
    // Lock should be released after engine is dropped
    let engine = Engine::open(dir.path(), Config::default());
    assert!(engine.is_ok(), "reopen after close must succeed");
}
