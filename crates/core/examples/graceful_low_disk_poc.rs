//! Standalone PoC for the low-disk → graceful-shutdown wiring in bichon-core.
//!
//! Run with:
//!     cargo run -p bichon-core --example graceful_low_disk_poc
//!
//! It reproduces the production handler shape:
//!
//!     low disk detected by bichon-blob
//!         -> handler enables below-floor writes (shutdown drain)
//!         -> handler raises SIGNAL_MANAGER's graceful-shutdown signal
//!         -> a subscriber receives it (the server would stop serving here)
//!         -> the process keeps running; queued blobs can still be written
//!
//! The disk floor is faked with `min_free_bytes = u64::MAX`, so every real
//! volume counts as "below the floor".

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bichon_blob::{Codec, Config, Engine, Error};
use bichon_core::common::signal::{SignalManager, SIGNAL_MANAGER};

fn unique_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir()
        .join(format!("bichon-graceful-poc-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
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

fn main() {
    println!("=== bichon graceful-shutdown PoC ===");
    let mut failed = 0usize;

    // 1. A shutdown requested before a subscriber exists must not be lost
    //    (the server subscribes after BLOB_MANAGER is opened at startup).
    {
        print!("[1/3] request-before-subscribe is not lost ... ");
        let manager = SignalManager::new();
        manager.trigger_shutdown();
        let mut receiver = manager.subscribe();
        if receiver.try_recv().is_ok() {
            println!("PASS");
        } else {
            failed += 1;
            println!("FAIL: late subscriber missed the shutdown request");
        }
    }

    let root = unique_dir();

    // 2. Low disk drives the production sequence: handler enables the drain
    //    write and raises the real SIGNAL_MANAGER signal.
    {
        print!("[2/3] low disk raises the graceful-shutdown signal ... ");
        let config = Config {
            min_free_bytes: u64::MAX,
            disk_check_interval_secs: 5,
            ..Config::default()
        };
        let engine = Arc::new(Engine::open(&root, config).expect("open blob engine"));

        let mut receiver = SIGNAL_MANAGER.subscribe();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_cb = Arc::clone(&hits);
        let engine_cb = Arc::downgrade(&engine);

        engine.set_low_disk_callback(move || {
            hits_cb.fetch_add(1, Ordering::SeqCst);
            if let Some(engine) = engine_cb.upgrade() {
                // This is what the shutdown drain needs.
                engine.allow_writes_below_free_floor(true);
            }
            // This is what stops the server gracefully.
            SIGNAL_MANAGER.trigger_shutdown();
        });

        let fired = wait_until(|| hits.load(Ordering::SeqCst) >= 1, Duration::from_secs(7));
        let signalled = wait_until(|| receiver.try_recv().is_ok(), Duration::from_secs(2));
        if fired && signalled {
            println!("PASS");
        } else {
            failed += 1;
            println!(
                "FAIL: handler fired={fired}, shutdown signal received={signalled}"
            );
        }

        // 3. During the drain, writes below the floor must still succeed.
        print!("[3/3] drain writes below the floor are allowed ... ");
        match engine.put([9u8; 32], b"queued-email", Codec::None) {
            Ok(()) => match engine.get(&[9u8; 32]) {
                Ok(Some(value)) if value == b"queued-email" => println!("PASS"),
                other => {
                    failed += 1;
                    println!("FAIL: drain write unreadable: {other:?}");
                }
            },
            Err(Error::DiskFull(e)) => {
                failed += 1;
                println!("FAIL: drain write refused with DiskFull: {e}");
            }
            Err(e) => {
                failed += 1;
                println!("FAIL: drain write failed: {e}");
            }
        }

        // Still below the floor, so the watchdog keeps checking; the handler
        // must not fire again and the process must still be alive.
        thread::sleep(Duration::from_secs(6));
        let count = hits.load(Ordering::SeqCst);
        if count != 1 {
            failed += 1;
            println!("      FAIL: handler fired {count} times for one episode");
        }

        engine.shutdown().expect("shutdown engine");
    }

    let _ = fs::remove_dir_all(&root);

    if failed == 0 {
        println!("\nALL SCENARIOS PASSED");
    } else {
        eprintln!("\n{failed} SCENARIO(S) FAILED");
        std::process::exit(1);
    }
}
