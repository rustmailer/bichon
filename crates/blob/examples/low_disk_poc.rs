//! Standalone PoC for the low-disk behaviour added to `bichon-blob`.
//!
//! Run with:
//!     cargo run -p bichon-blob --example low_disk_poc
//!
//! It uses a temporary directory on the real disk and fakes "low disk" by
//! setting `min_free_bytes` to `u64::MAX` (every real volume is below that).
//! The scenarios verify:
//!   1. appends are refused before writing when below the floor
//!   2. a refused append leaves the segment file untouched
//!   3. the low-disk handler fires once for a late registration, enables drain
//!      writes, and the process keeps running (no self-exit)
//!   4. the handler stays quiet when the volume has plenty of free space
//!   5. a torn tail entry is repaired on reopen instead of failing the open
//!   6. GC on a sealed segment leaves no temp_*.seg behind

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bichon_blob::{Codec, Config, Engine, Error};
use tempfile::TempDir;

fn key(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn wait_until(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    pred()
}

fn temp_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir.join("segments"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("temp_"))
                .collect()
        })
        .unwrap_or_default()
}

fn scenario_preflight_and_drain() -> Result<(), String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let mut config = Config::default();
    // Every real volume is below this floor, so the preflight always trips.
    config.min_free_bytes = u64::MAX;

    let engine = Engine::open(dir.path(), config).map_err(|e| e.to_string())?;

    // 1. Refused, and with a typed DiskFull error.
    match engine.put(key(1), b"payload", Codec::None) {
        Err(Error::DiskFull(msg)) => {
            println!("      refused: {msg}");
        }
        other => return Err(format!("expected DiskFull, got {other:?}")),
    }

    // 2. The refusal must not have written a single byte.
    let seg = dir.path().join("segments").join("00000001.seg");
    let len = fs::metadata(&seg).map_err(|e| e.to_string())?.len();
    if len != 0 {
        return Err(format!("segment grew to {len} bytes after a refused append"));
    }

    // 3. Once the graceful-shutdown drain enables writes, the same append
    //    must go through even though we are still below the floor.
    engine.allow_writes_below_free_floor(true);
    engine
        .put(key(1), b"payload", Codec::None)
        .map_err(|e| format!("drain write was rejected: {e}"))?;
    let got = engine
        .get(&key(1))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "drain write not readable".to_string())?;
    if got != b"payload" {
        return Err("drain write returned wrong payload".into());
    }

    engine.shutdown().map_err(|e| e.to_string())?;
    Ok(())
}

fn scenario_disk_watchdog() -> Result<(), String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let config = Config {
        min_free_bytes: u64::MAX,
        disk_check_interval_secs: 5,
        ..Config::default()
    };

    let engine = Arc::new(Engine::open(dir.path(), config).map_err(|e| e.to_string())?);

    // Before any handler exists, appends are refused.
    if !matches!(
        engine.put(key(2), b"before-handler", Codec::None),
        Err(Error::DiskFull(_))
    ) {
        return Err("append was not refused before a low-disk handler existed".into());
    }

    // Wait for the watchdog's first check so registration below is the
    // "handler registered after the disk was already low" path.
    thread::sleep(Duration::from_millis(500));

    // Mimic the production handler: allow below-floor writes, then request a
    // graceful shutdown. Here "requesting shutdown" is just counting the call.
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_cb = Arc::clone(&hits);
    let engine_cb = Arc::downgrade(&engine);
    engine.set_low_disk_callback(move || {
        hits_cb.fetch_add(1, Ordering::SeqCst);
        if let Some(engine) = engine_cb.upgrade() {
            engine.allow_writes_below_free_floor(true);
        }
    });

    // Registering a handler while already below the floor must fire it
    // immediately, not only on the next watchdog tick.
    if !wait_until(|| hits.load(Ordering::SeqCst) >= 1, Duration::from_secs(2)) {
        return Err("low-disk handler did not fire on late registration".into());
    }

    // The handler enabled drain writes: the append must now succeed even
    // though the volume is still below the floor.
    engine
        .put(key(2), b"after-handler", Codec::None)
        .map_err(|e| format!("handler did not allow the drain write: {e}"))?;

    // Still below the floor, so the watchdog keeps checking, but the handler
    // must fire only once per episode.
    thread::sleep(Duration::from_secs(6));
    let count = hits.load(Ordering::SeqCst);
    if count != 1 {
        return Err(format!("handler fired {count} times for one episode"));
    }

    engine.shutdown().map_err(|e| e.to_string())?;
    Ok(())
}

fn scenario_torn_tail_recovery() -> Result<(), String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let engine = Engine::open(dir.path(), Config::default()).map_err(|e| e.to_string())?;

    let payload = vec![b'x'; 100];
    engine
        .put(key(3), &payload, Codec::None)
        .map_err(|e| e.to_string())?;

    let seg = dir.path().join("segments").join("00000001.seg");
    let first_len = fs::metadata(&seg).map_err(|e| e.to_string())?.len();

    engine
        .put(key(4), &payload, Codec::None)
        .map_err(|e| e.to_string())?;
    engine.shutdown().map_err(|e| e.to_string())?;
    drop(engine);

    // Simulate a write that died mid-entry: cut the last byte of the last
    // entry. Its 50-byte header is intact, the data is short.
    let total = fs::metadata(&seg).map_err(|e| e.to_string())?.len();
    if total <= first_len + 1 {
        return Err("second entry is unexpectedly small".into());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&seg)
        .map_err(|e| e.to_string())?;
    file.set_len(total - 1).map_err(|e| e.to_string())?;
    drop(file);

    // Reopening must repair the torn tail (and truncate it) instead of
    // failing with "failed to fill whole buffer".
    let engine = Engine::open(dir.path(), Config::default())
        .map_err(|e| format!("reopen failed on a torn tail: {e}"))?;

    let got = engine
        .get(&key(3))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "complete entry lost after recovery".to_string())?;
    if got != payload {
        return Err("complete entry corrupted after recovery".into());
    }

    let repaired_len = fs::metadata(&seg).map_err(|e| e.to_string())?.len();
    if repaired_len != first_len {
        return Err(format!(
            "torn tail not truncated: expected {first_len} bytes, found {repaired_len}"
        ));
    }

    engine.shutdown().map_err(|e| e.to_string())?;
    Ok(())
}

fn scenario_gc_leaves_no_temp_files() -> Result<(), String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let engine = Engine::open(dir.path(), Config::default()).map_err(|e| e.to_string())?;

    let payload = vec![b'y'; 200];
    for i in 0..10u8 {
        engine
            .put(key(10 + i), &payload, Codec::None)
            .map_err(|e| e.to_string())?;
    }
    for i in 0..8u8 {
        engine.delete(&key(10 + i)).map_err(|e| e.to_string())?;
    }

    engine.seal_active_segment().map_err(|e| e.to_string())?;
    let stats = engine
        .gc()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "GC found no candidate segment".to_string())?;
    println!(
        "      compacted segment {}: {} -> {} bytes (kept {}, skipped {})",
        stats.segment_id,
        stats.bytes_before,
        stats.bytes_after,
        stats.entries_kept,
        stats.entries_skipped
    );

    let leftovers = temp_files(dir.path());
    if !leftovers.is_empty() {
        return Err(format!("GC left temp files behind: {leftovers:?}"));
    }

    for i in 8..10u8 {
        let got = engine
            .get(&key(10 + i))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("kept key {} disappeared after GC", 10 + i))?;
        if got != payload {
            return Err(format!("kept key {} corrupted after GC", 10 + i));
        }
    }

    engine.shutdown().map_err(|e| e.to_string())?;
    Ok(())
}

fn scenario_no_false_positive() -> Result<(), String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let config = Config {
        // Every real volume has at least one byte free, so the floor is never
        // breached and the handler must stay quiet.
        min_free_bytes: 1,
        disk_check_interval_secs: 5,
        ..Config::default()
    };

    let engine = Engine::open(dir.path(), config).map_err(|e| e.to_string())?;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_cb = Arc::clone(&hits);
    engine.set_low_disk_callback(move || {
        hits_cb.fetch_add(1, Ordering::SeqCst);
    });

    // One full watchdog interval plus slack.
    thread::sleep(Duration::from_secs(6));

    let count = hits.load(Ordering::SeqCst);
    if count != 0 {
        return Err(format!("handler fired {count} times with plenty of space"));
    }

    engine
        .put(key(5), b"normal-write", Codec::None)
        .map_err(|e| format!("normal write failed: {e}"))?;

    engine.shutdown().map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    println!("=== bichon-blob low-disk PoC ===");

    let scenarios: &[(&str, fn() -> Result<(), String>)] = &[
        (
            "preflight refuses below floor / drain writes allowed",
            scenario_preflight_and_drain,
        ),
        (
            "watchdog fires handler once and does not exit",
            scenario_disk_watchdog,
        ),
        (
            "no handler when free space is sufficient",
            scenario_no_false_positive,
        ),
        (
            "torn tail is repaired on reopen",
            scenario_torn_tail_recovery,
        ),
        (
            "GC leaves no temp_*.seg behind",
            scenario_gc_leaves_no_temp_files,
        ),
    ];

    let mut failed = 0usize;
    for (index, (name, run)) in scenarios.iter().enumerate() {
        print!("[{}/{}] {name} ... ", index + 1, scenarios.len());
        match run() {
            Ok(()) => println!("PASS"),
            Err(e) => {
                failed += 1;
                println!("FAIL: {e}");
            }
        }
    }

    if failed == 0 {
        println!("\nALL SCENARIOS PASSED");
    } else {
        eprintln!("\n{failed} SCENARIO(S) FAILED");
        std::process::exit(1);
    }
}
