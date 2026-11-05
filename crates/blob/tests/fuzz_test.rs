/// Fuzz-style tests: randomized operation sequences, corruption injection,
/// and crash-recovery stress testing.
use bichon_blob::{Codec, Config, Engine};
use rand::RngExt;
use std::collections::HashMap;
use tempfile::TempDir;

/// Number of iterations for each randomized test.
const FUZZ_OPS: usize = 2000;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_key(i: u64) -> [u8; 32] {
    let mut key = [0u8; 32];
    key[0..8].copy_from_slice(&i.to_le_bytes());
    key
}

fn make_value(rng: &mut impl RngExt) -> Vec<u8> {
    let size = match rng.random_range(0..100) {
        0..=4   => rng.random_range(0..64),              // tiny
        5..=9   => 0,                                  // empty
        10..=79 => rng.random_range(64..4096),            // small
        80..=89 => rng.random_range(4096..65536),         // medium
        90..=94 => rng.random_range(65536..500_000),      // large
        _       => rng.random_range(500_000..2_000_000),  // xl (near max)
    };
    let mut v = vec![0u8; size];
    rng.fill(&mut v[..]);
    v
}

fn random_codec(rng: &mut impl RngExt) -> Codec {
    match rng.random_range(0..4) {
        0 => Codec::None,
        1 => Codec::Zstd,
        _ => Codec::Lz4,
    }
}

// ── Fuzz: random operation sequence ─────────────────────────────────────────

#[test]
fn fuzz_random_ops() {
    let mut rng = rand::rng();
    let dir = TempDir::new().unwrap();
    let mut config = Config::default();
    config.flush_interval_secs = 0; // manual flush only
    config.gc_interval_secs = 0;

    let engine = Engine::open(dir.path(), config).unwrap();

    // Oracle: track expected values in memory
    let mut oracle: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let mut next_key = 0u64;

    for _ in 0..FUZZ_OPS {
        match rng.random_range(0..100) {
            // 45%: put new key
            0..=44 => {
                let key = make_key(next_key);
                next_key += 1;
                let value = make_value(&mut rng);
                let codec = random_codec(&mut rng);
                engine.put(key, &value, codec).unwrap();
                oracle.insert(key, value);
            }
            // 20%: put overwrite existing key
            45..=64 => {
                if oracle.is_empty() { continue; }
                let idx = rng.random_range(0..oracle.len());
                let key = *oracle.iter().nth(idx).unwrap().0;
                let value = make_value(&mut rng);
                let codec = random_codec(&mut rng);
                engine.put(key, &value, codec).unwrap();
                oracle.insert(key, value);
            }
            // 15%: read and verify
            65..=79 => {
                if oracle.is_empty() { continue; }
                let idx = rng.random_range(0..oracle.len());
                let key = *oracle.iter().nth(idx).unwrap().0;
                let expected = oracle.get(&key).unwrap();
                let got = engine.get(&key).unwrap();
                assert_eq!(got.as_ref(), Some(expected), "key mismatch on read");
            }
            // 10%: delete
            80..=89 => {
                if oracle.is_empty() { continue; }
                let idx = rng.random_range(0..oracle.len());
                let key = *oracle.iter().nth(idx).unwrap().0;
                engine.delete(&key).unwrap();
                oracle.remove(&key);
                let got = engine.get(&key).unwrap();
                assert_eq!(got, None, "deleted key should return None");
            }
            // 5%: read non-existent key
            90..=94 => {
                let key = make_key(next_key + rng.random_range(1000u64..10000));
                let got = engine.get(&key).unwrap();
                assert_eq!(got, None, "non-existent key should return None");
            }
            // 5%: flush
            _ => {
                engine.flush().unwrap();
            }
        }
    }

    // Final verification: all oracle entries must match
    for (key, expected) in &oracle {
        let got = engine.get(key).unwrap();
        assert_eq!(got.as_ref(), Some(expected), "final verification: key mismatch");
    }
}

// ── Fuzz: crash + reopen cycle ──────────────────────────────────────────────

#[test]
fn fuzz_crash_reopen_cycles() {
    let mut rng = rand::rng();
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().to_path_buf();

    let mut oracle: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let mut next_key = 0u64;
    let cycles = 20;

    for _cycle in 0..cycles {
        // Open database
        let mut config = Config::default();
        config.flush_interval_secs = 0;
        config.gc_interval_secs = 0;
        let engine = Engine::open(&dir_path, config).unwrap();

        // Do some work
        let ops = rng.random_range(50..200);
        for _ in 0..ops {
            match rng.random_range(0..100) {
                0..=50 => {
                    let key = make_key(next_key);
                    next_key += 1;
                    let value = make_value(&mut rng);
                    if engine.put(key, &value, Codec::Zstd).is_ok() {
                        oracle.insert(key, value);
                    }
                }
                51..=65 => {
                    if oracle.is_empty() { continue; }
                    let idx = rng.random_range(0..oracle.len());
                    let key = *oracle.iter().nth(idx).unwrap().0;
                    engine.delete(&key).unwrap();
                    oracle.remove(&key);
                }
                66..=85 => {
                    if oracle.is_empty() { continue; }
                    let idx = rng.random_range(0..oracle.len());
                    let key = *oracle.iter().nth(idx).unwrap().0;
                    let expected = oracle.get(&key).unwrap();
                    if let Ok(Some(got)) = engine.get(&key) {
                        assert_eq!(&got, expected, "pre-crash read mismatch");
                    }
                }
                _ => {
                    let _ = engine.flush();
                }
            }
        }

        // Simulate crash: drop without shutdown
        drop(engine);
    }

    // Final reopen: all oracle entries must be intact
    let config = Config::default();
    let engine = Engine::open(&dir_path, config).unwrap();
    for (key, expected) in &oracle {
        let got = engine.get(key).unwrap();
        assert_eq!(got.as_ref(), Some(expected), "after {} crash cycles", cycles);
    }
}

// ── Fuzz: batch operations ──────────────────────────────────────────────────

#[test]
fn fuzz_batch_ops() {
    let mut rng = rand::rng();
    let dir = TempDir::new().unwrap();

    let config = Config::default();
    let engine = Engine::open(dir.path(), config).unwrap();

    let mut oracle: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let mut next_key = 0u64;

    for _ in 0..200 {
        match rng.random_range(0..100) {
            // 50%: batch write
            0..=49 => {
                let batch_size = rng.random_range(1..30);
                let entries: Vec<_> = (0..batch_size)
                    .map(|_| {
                        let key = make_key(next_key);
                        next_key += 1;
                        let value = make_value(&mut rng);
                        oracle.insert(key, value.clone());
                        (key, value, Codec::Zstd)
                    })
                    .collect();
                engine.put_batch(&entries).unwrap();
            }
            // 30%: verify random subset
            50..=79 => {
                if oracle.is_empty() { continue; }
                let n = rng.random_range(1..=20.min(oracle.len()));
                for _ in 0..n {
                    let idx = rng.random_range(0..oracle.len());
                    let (key, expected) = oracle.iter().nth(idx).unwrap();
                    let got = engine.get(key).unwrap();
                    assert_eq!(got.as_ref(), Some(expected));
                }
            }
            // 20%: batch delete
            _ => {
                if oracle.is_empty() { continue; }
                let n = rng.random_range(1..=20.min(oracle.len()));
                let keys: Vec<[u8; 32]> = (0..n)
                    .map(|_| {
                        let idx = rng.random_range(0..oracle.len());
                        let key = *oracle.iter().nth(idx).unwrap().0;
                        oracle.remove(&key);
                        key
                    })
                    .collect();
                engine.delete_batch(&keys).unwrap();
            }
        }
    }

    for (key, expected) in &oracle {
        let got = engine.get(key).unwrap();
        assert_eq!(got.as_ref(), Some(expected));
    }
}

// ── Fuzz: GC stress ─────────────────────────────────────────────────────────

#[test]
fn fuzz_gc_stress() {
    let mut rng = rand::rng();
    let dir = TempDir::new().unwrap();

    let mut config = Config::default();
    config.gc_deleted_ratio = 0.1; // aggressive GC trigger
    let engine = Engine::open(dir.path(), config.clone()).unwrap();

    let mut oracle: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let mut next_key = 0u64;

    for round in 0..10 {
        // Write a batch of keys
        let n = rng.random_range(50..150);
        let mut round_keys: Vec<[u8; 32]> = Vec::new();
        let value_size = rng.random_range(100..10000);
        let value: Vec<u8> = (0..value_size).map(|_| rng.random::<u8>()).collect();

        for _ in 0..n {
            let key = make_key(next_key);
            next_key += 1;
            engine.put(key, &value, Codec::None).unwrap();
            oracle.insert(key, value.clone());
            round_keys.push(key);
        }

        // Delete some fraction
        let delete_frac: f64 = rng.random_range(0.2..0.8);
        let delete_count = (round_keys.len() as f64 * delete_frac) as usize;
        for _ in 0..delete_count {
            let idx = rng.random_range(0..round_keys.len());
            let key = round_keys.swap_remove(idx);
            engine.delete(&key).unwrap();
            oracle.remove(&key);
        }

        // Seal and run GC
        engine.seal_active_segment().unwrap();
        let _ = engine.gc().unwrap();

        // Verify all remaining oracle entries
        for (key, expected) in &oracle {
            let got = engine.get(key).unwrap();
            assert_eq!(
                got.as_ref(),
                Some(expected),
                "GC round {}: key mismatch",
                round
            );
        }
    }

    // Restart after all those compactions: recovery must resume from the
    // compacted files' physical end, not truncate their tails.
    drop(engine);
    let engine = Engine::open(dir.path(), config).unwrap();
    for (key, expected) in &oracle {
        let got = engine.get(key).unwrap();
        assert_eq!(
            got.as_ref(),
            Some(expected),
            "entry lost by a restart after GC"
        );
    }
}

// ── Fuzz: bitflip corruption resilience ─────────────────────────────────────

#[test]
fn fuzz_corruption_resilience() {
    let mut rng = rand::rng();
    let dir = TempDir::new().unwrap();
    let dir_path = dir.path().to_path_buf();

    let config = Config::default();
    let engine = Engine::open(&dir_path, config).unwrap();

    // Write known data
    let mut good_keys: Vec<[u8; 32]> = Vec::new();
    let n = 100u64;
    for i in 0..n {
        let key = make_key(i);
        let value = vec![i as u8; 2048];
        engine.put(key, &value, Codec::None).unwrap();
        good_keys.push(key);
    }
    engine.flush().unwrap();

    // Seal so segment file is durable on disk
    engine.seal_active_segment().unwrap();
    engine.shutdown().unwrap();
    drop(engine);

    // Find segment files and corrupt random bytes
    let seg_dir = dir_path.join("segments");
    let mut seg_files: Vec<_> = std::fs::read_dir(&seg_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with(".seg")
        })
        .collect();
    seg_files.sort_by_key(|e| e.file_name());

    // Corrupt 3 random bytes in the first segment
    if let Some(seg) = seg_files.first() {
        let path = seg.path();
        let mut data = std::fs::read(&path).unwrap();
        if data.len() > 100 {
            for _ in 0..3 {
                let pos = rng.random_range(50..data.len());
                data[pos] ^= 0xFF; // flip all bits
            }
            std::fs::write(&path, &data).unwrap();
        }
    }

    // Reopen: recovery must succeed (not panic), even if some keys are lost
    let config = Config::default();
    let engine = Engine::open(&dir_path, config).unwrap();

    // At least some uncorrupted keys should still be readable
    let mut readable = 0;
    let mut corrupted = 0;
    for key in &good_keys {
        match engine.get(key) {
            Ok(Some(_)) => readable += 1,
            Ok(None) => { /* key might be lost due to corruption */ }
            Err(_) => corrupted += 1,
        }
    }
    // The vast majority should still be ok (corruption only hit 3 bytes in one segment)
    assert!(
        readable + corrupted > 0,
        "at least some outcomes should be observable"
    );
    assert!(
        readable > n as usize / 2,
        "majority of keys should survive localized corruption ({} of {})",
        readable,
        n
    );
}
