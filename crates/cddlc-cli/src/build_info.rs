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
    "\ncommit: ",
    env!("CDDLC_BUILD_COMMIT"),
    "\nbranch: ",
    env!("CDDLC_BUILD_BRANCH"),
    "\nremote: ",
    env!("CDDLC_BUILD_REMOTE"),
);

/// The lines after `@generated` in every generated file's header: which cddlc made
/// it, and from which sources. No timestamp, so the output depends only on the
/// inputs and this cddlc.
pub fn provenance(inputs: &[std::path::PathBuf]) -> Vec<String> {
    let mut lines = vec![
        format!("cddlc {RELEASE} ({COMMIT})"),
        format!("branch {BRANCH}, {REMOTE}"),
    ];
    for p in inputs {
        lines.push(match std::fs::read(p) {
            Ok(bytes) => format!(
                "source {} (sha256 {}, {} bytes)",
                p.display(),
                sha256_hex(&bytes),
                bytes.len()
            ),
            Err(_) => format!("source {}", p.display()),
        });
    }
    lines
}

/// The comment block at the top of each input, for every generated file's header;
/// an input whose block matches an earlier one's adds nothing.
pub fn boilerplate(inputs: &[std::path::PathBuf]) -> Vec<String> {
    let mut blocks: Vec<Vec<String>> = Vec::new();
    for p in inputs {
        let Ok(src) = std::fs::read_to_string(p) else {
            continue;
        };
        let block = cddlc_codegen::cddl_boilerplate(&src);
        if !block.is_empty() && !blocks.contains(&block) {
            blocks.push(block);
        }
    }
    blocks.join(&String::new())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
