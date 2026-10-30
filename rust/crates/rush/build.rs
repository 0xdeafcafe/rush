//! A rush built from a checkout says the commit it was built from, as a
//! released one does (`rush 0.1.0 (3f2a9c1b7d0e)`): RUSH_COMMIT, unless
//! the build sets it (the release workflow does). Not RUSH_VERSION: that
//! names a release, and a rush without one doesn't update itself.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-env-changed=RUSH_COMMIT");
    if std::env::var_os("RUSH_COMMIT").is_some() {
        return;
    }
    // A new commit moves HEAD's log; checking out another moves HEAD.
    for p in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", p]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(commit) = git(&["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=RUSH_COMMIT={commit}");
    }
}
