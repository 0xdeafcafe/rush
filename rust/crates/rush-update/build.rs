//! Stamps a rush built from a checkout with its commit, as Go's toolchain
//! stamps a go-built one (a pseudo-version, v0.0.0-<UTC time>-<commit>),
//! so `rush version` and the top bar say which rush it is
//! (internal/update/update.go:55 Current). A RUSH_VERSION set for the
//! build wins; with no git, there's none and rush says its crate version.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .env("TZ", "UTC")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-env-changed=RUSH_VERSION");
    if std::env::var_os("RUSH_VERSION").is_some() {
        return;
    }
    // A new commit moves HEAD's log; checking out another moves HEAD.
    for p in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", p]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let Some(stamp) = git(&[
        "log",
        "-1",
        "--format=%cd-%H",
        "--date=format-local:%Y%m%d%H%M%S",
    ]) else {
        return;
    };
    let Some((at, commit)) = stamp.split_once('-') else {
        return;
    };
    if at.len() != 14 || commit.len() < 12 {
        return;
    }
    println!("cargo:rustc-env=RUSH_VERSION=v0.0.0-{at}-{}", &commit[..12]);
}
