//! CLI-level contract for `bichon-admin reindex` confirmation.
//!
//! These spawn the real binary with a null stdin (no TTY), which is how a
//! Kubernetes init container / Job runs it. The timeout guard turns a
//! regression back into a blocking prompt into a test failure instead of a
//! hung CI job.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

struct CliOutput {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_reindex(args: &[&str]) -> CliOutput {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bichon-admin"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bichon-admin");

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait().expect("wait for bichon-admin") {
            Some(_) => {
                let output = child.wait_with_output().expect("collect output");
                return CliOutput {
                    code: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                };
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("bichon-admin did not finish within 60s; args={args:?}");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[test]
fn help_documents_yes() {
    let out = run_reindex(&["reindex", "--help"]);
    assert_eq!(out.code, Some(0), "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("--yes"),
        "--help must document --yes:\n{}",
        out.stdout
    );
}

#[test]
fn no_terminal_without_yes_exits_2_instead_of_silently_cancelling() {
    let root = TempDir::new().unwrap();
    let root_str = root.path().to_string_lossy().into_owned();

    let out = run_reindex(&["reindex", "--root", &root_str]);
    assert_eq!(
        out.code,
        Some(2),
        "stdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    assert!(
        out.stderr.contains("--yes"),
        "the error must point at --yes:\n{}",
        out.stderr
    );
    assert!(
        !out.stdout.contains("Cancelled."),
        "must not look like a user cancellation:\n{}",
        out.stdout
    );
}

#[test]
fn yes_skips_the_prompt_and_reaches_the_pre_checks() {
    let root = TempDir::new().unwrap();
    let root_str = root.path().to_string_lossy().into_owned();

    let out = run_reindex(&["reindex", "--yes", "--root", &root_str]);
    assert_eq!(
        out.code,
        Some(0),
        "stdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    assert!(
        out.stdout.contains("No index found"),
        "--yes must reach the pre-check instead of the cancel path:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("Cancelled."),
        "must not prompt:\n{}",
        out.stdout
    );
}

#[test]
fn yes_does_not_bypass_root_validation() {
    let root = TempDir::new().unwrap();
    let missing = root.path().join("does-not-exist");
    let missing_str = missing.to_string_lossy().into_owned();

    let out = run_reindex(&["reindex", "--yes", "--root", &missing_str]);
    assert_eq!(out.code, Some(1), "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("Directory does not exist"),
        "--yes must only skip confirmation, not validation:\n{}",
        out.stderr
    );
}

#[test]
fn yes_without_root_errors_instead_of_prompt_looping() {
    let out = run_reindex(&["reindex", "--yes"]);
    assert_eq!(out.code, Some(1), "stderr: {}", out.stderr);
    assert!(
        out.stderr.contains("pass --root"),
        "missing root must be an error, not an unanswered prompt loop:\n{}",
        out.stderr
    );
}
