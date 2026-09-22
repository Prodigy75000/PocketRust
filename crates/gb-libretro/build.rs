// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Stamps a build identity into the core, so a binary can say which commit it
//! came from.
//!
//! This exists because `jniLibs/` is a shared mutable directory with no version
//! in it: two repos write cores there, a third assembles them into an APK, and
//! a reader cannot tell a fresh copy from a stale one by looking. On 2026-09-22
//! that was answered twice with bad instruments, once by comparing section
//! sizes (the APK copy is stripped, so hashes never match, and the total APK
//! size was byte-identical across two different cores because 16 KB page
//! padding absorbed the difference) and once by comparing file mtimes against
//! build times. A core fix and an APK build landed two seconds apart that day,
//! and only an after-the-fact hash check caught it.
//!
//! A greppable hash turns "when did this file appear" into "which commit is
//! this", which is the difference between evidence and inference.
//!
//! # Why the identity is passed IN rather than read here
//!
//! A build script's output is cached, and Cargo decides when to re-run it. A
//! hash computed here could therefore survive into a later build and name a
//! commit the binary is not from, which is worse than no hash at all: it would
//! be confidently wrong in exactly the situation it exists to resolve.
//!
//! So `POCKETRUST_BUILD_ID` is declared as a rerun trigger. The deploy script
//! computes it fresh every time and exports it, so the value changes whenever
//! the tree does, which forces a re-run. Reading git here is only a fallback
//! for an ordinary `cargo build`, and it is labelled so it cannot be mistaken
//! for the real thing.

use std::process::Command;

fn main() {
    // The one that matters. Set by scripts/deploy-android-debug.sh.
    println!("cargo:rerun-if-env-changed=POCKETRUST_BUILD_ID");
    // And watch the commit itself, so a plain `cargo build` restamps when HEAD
    // moves. Without this Cargo happily reuses a cached run and the binary
    // carries whatever commit was checked out the FIRST time the script ran:
    // measured 2026-09-22, a build made from 6a2673f identified itself as
    // c80d7c02a, a commit from a different day. A stamp that names the wrong
    // commit is worse than no stamp, because the whole point of it is to be the
    // one thing you can trust about a binary you did not watch being built.
    watch_git_head();

    let id = std::env::var("POCKETRUST_BUILD_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(fallback_id);

    // Keep it to something a `strings | grep` can pick out of a stripped .so,
    // and refuse anything that could break that.
    let id: String = id
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
        .take(32)
        .collect();
    let id = if id.is_empty() { "unknown".into() } else { id };

    println!("cargo:rustc-env=POCKETRUST_BUILD_ID={id}");
}

/// Ask Cargo to re-run this script whenever the checked-out commit changes.
///
/// `.git/HEAD` alone is not enough: it only changes when the BRANCH changes, so
/// committing on the branch you are already on would not trigger it. The ref it
/// points at is the file that moves per commit, so both are watched. A ref that
/// lives in `packed-refs` rather than as a loose file simply will not be found,
/// and then this does nothing, which is the same position as before and why
/// nothing here fails the build.
fn watch_git_head() {
    let Some(root) = git_dir() else { return };
    let head = root.join("HEAD");
    if !head.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", head.display());
    let Ok(contents) = std::fs::read_to_string(&head) else {
        return;
    };
    if let Some(r) = contents.strip_prefix("ref:").map(str::trim) {
        let path = root.join(r);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// The repository's `.git`, resolved from this crate rather than the cwd, which
/// during a build script is not guaranteed to be anywhere in particular.
fn git_dir() -> Option<std::path::PathBuf> {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
    manifest
        .ancestors()
        .map(|a| a.join(".git"))
        .find(|p| p.is_dir())
}

/// Best effort for a plain `cargo build`, where nothing exported an identity.
///
/// Suffixed `-local` rather than dressed up as a deploy stamp: this value can
/// be stale, since Cargo may not re-run this script, and a hash that might name
/// the wrong commit must not look authoritative.
fn fallback_id() -> String {
    let short = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    match short {
        Some(h) => format!("{h}-local"),
        None => "unknown".into(),
    }
}
