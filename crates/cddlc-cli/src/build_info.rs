//! Which cddlc this is (dgtlrift/cddlc#10), as `build.rs` recorded it.

/// The release: its tag, or `v` and the crate version when the commit has none.
pub const RELEASE: &str = env!("CDDLC_BUILD_RELEASE");
/// The commit, with `-dirty` for a modified tree.
pub const COMMIT: &str = env!("CDDLC_BUILD_COMMIT");
/// The branch.
pub const BRANCH: &str = env!("CDDLC_BUILD_BRANCH");
/// The git remote (`origin`), without credentials.
pub const REMOTE: &str = env!("CDDLC_BUILD_REMOTE");

/// `cddlc --version`.
pub const VERSION: &str = env!("CDDLC_BUILD_RELEASE");
/// `cddlc --version --verbose` (`-V` long form).
pub const LONG_VERSION: &str = concat!(
    env!("CDDLC_BUILD_RELEASE"),
    "\ncommit: ", env!("CDDLC_BUILD_COMMIT"),
    "\nbranch: ", env!("CDDLC_BUILD_BRANCH"),
    "\nremote: ", env!("CDDLC_BUILD_REMOTE"),
);

/// The lines after `@generated` in every generated file's header: no timestamp, so the
/// output depends only on the inputs and this cddlc.
pub fn provenance(inputs: &[std::path::PathBuf]) -> Vec<String> {
    let inputs: Vec<String> = inputs.iter().map(|p| p.display().to_string()).collect();
    vec![
        format!("cddlc {RELEASE} ({COMMIT})"),
        format!("branch {BRANCH}, {REMOTE}"),
        format!("from {}", inputs.join(", ")),
    ]
}
