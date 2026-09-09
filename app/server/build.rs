//! Stamps the binary with a build identifier so a running qw-web can say
//! which commit it was built from — the Security tab shows it, and the
//! deploy script compares it to decide whether a restart actually took.
//!
//! `QW_WEB_BUILD` wins if set (CI / reproducible builds); otherwise it is
//! `<commit date> <short hash>[ +dirty]` from git, or `unknown` off a
//! tarball with no git.

use std::process::Command;

fn main() {
    // A new commit, a staged change, or a UI edit should refresh the stamp.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../ui/index.html");
    println!("cargo:rerun-if-env-changed=QW_WEB_BUILD");

    let build = std::env::var("QW_WEB_BUILD")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(from_git);

    println!("cargo:rustc-env=QW_WEB_BUILD={build}");
}

fn from_git() -> String {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    };

    let hash = match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(h) => h,
        None => return "unknown".into(),
    };
    let date = git(&["show", "-s", "--format=%cs", "HEAD"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let mut s = if date.is_empty() {
        hash
    } else {
        format!("{date} {hash}")
    };
    if dirty {
        s.push_str(" +dirty");
    }
    s
}
