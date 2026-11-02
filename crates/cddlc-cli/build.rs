//! Records which cddlc this is, for `--version` and generated files' headers
//! (dgtlrift/cddlc#10): the release tag if the commit has one, the commit (`-dirty` with
//! local changes), the branch and the remote, from the cddlc superproject's checkout (this
//! crate is its submodule) or this crate's own. Outside a git checkout (a crates.io
//! package) each is `unknown`. `CDDLC_BUILD_RELEASE`, `_COMMIT`, `_BRANCH` and `_REMOTE`
//! override them, for builds from an exported tree.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// `https://user:token@host/path` → `https://host/path`.
fn without_credentials(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme), Some(at)) if at > scheme => format!("{}{}", &url[..scheme + 3], &url[at + 1..]),
        _ => url.to_owned(),
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    // The superproject (dgtlrift/cddlc) when this is its submodule, else this checkout.
    let root = git(&manifest, &["rev-parse", "--show-superproject-working-tree"])
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.clone());
    let commit = git(&root, &["rev-parse", "HEAD"]).map(|c| {
        let dirty = git(&root, &["status", "--porcelain", "--untracked-files=no"]).is_some();
        if dirty { format!("{c}-dirty") } else { c }
    });
    let release = git(&root, &["describe", "--tags", "--exact-match", "HEAD"]);
    let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    let remote = git(&root, &["remote", "get-url", "origin"]).map(|u| without_credentials(&u));

    let pick = |var: &str, found: Option<String>| {
        println!("cargo:rerun-if-env-changed={var}");
        std::env::var(var).ok().or(found).unwrap_or_else(|| "unknown".to_owned())
    };
    let release = pick(
        "CDDLC_BUILD_RELEASE",
        Some(release.unwrap_or_else(|| format!("v{}", std::env::var("CARGO_PKG_VERSION").unwrap()))),
    );
    println!("cargo:rustc-env=CDDLC_BUILD_RELEASE={release}");
    println!("cargo:rustc-env=CDDLC_BUILD_COMMIT={}", pick("CDDLC_BUILD_COMMIT", commit));
    println!("cargo:rustc-env=CDDLC_BUILD_BRANCH={}", pick("CDDLC_BUILD_BRANCH", branch));
    println!("cargo:rustc-env=CDDLC_BUILD_REMOTE={}", pick("CDDLC_BUILD_REMOTE", remote));
    // A new commit or checkout reruns this.
    for p in ["HEAD", "index"] {
        if let Some(gitdir) = git(&root, &["rev-parse", "--git-dir"]) {
            let gitdir = if Path::new(&gitdir).is_absolute() { PathBuf::from(gitdir) } else { root.join(gitdir) };
            println!("cargo:rerun-if-changed={}", gitdir.join(p).display());
        }
    }
}
