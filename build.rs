use std::{io::Result, process::Command};

fn main() -> Result<()> {
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=Rstrtmgr");
    }
    let output = Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Failed to get git commit hash");
    let git_hash = String::from_utf8(output.stdout)
        .expect("Invalid UTF-8")
        .trim()
        .to_string();
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    Ok(())
}
