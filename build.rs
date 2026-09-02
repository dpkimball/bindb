//! Inject non-sensitive build provenance for `bindb --version`.
//!
//! Prefers CI env (`GITHUB_SHA`, `BINDB_BUILD_DATE`); falls back to local `git`
//! and UTC date. Never embeds private repository URLs.

use std::process::Command;

fn main() {
    let sha = std::env::var("GITHUB_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(git_sha)
        .unwrap_or_else(|| "unknown".to_string());
    let short = short_sha(&sha);

    let date = std::env::var("BINDB_BUILD_DATE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(utc_date)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=BINDB_GIT_SHA={short}");
    println!("cargo:rustc-env=BINDB_BUILD_DATE={date}");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=BINDB_BUILD_DATE");
}

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_string())
    }
}

fn short_sha(sha: &str) -> String {
    let trimmed = sha.trim();
    if trimmed.len() >= 7 {
        trimmed[..7].to_string()
    } else if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn utc_date() -> Option<String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let date = String::from_utf8(output.stdout).ok()?;
    let date = date.trim();
    if date.is_empty() {
        None
    } else {
        Some(date.to_string())
    }
}
