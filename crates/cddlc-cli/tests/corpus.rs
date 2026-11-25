//! Published CDDL, written by others (dgtlrift/cddlc#113): the RFC collection
//! that cabo/cddlc bundles, and WebDriver BiDi as google/cddlconv uses it (see
//! `tests/corpus/README.md`). Each schema is generated in every language and
//! format; one cddlc can't take is listed with the error it gives and the issue
//! that tracks it. A listed schema that starts working fails the test, so the
//! list stays current.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a schema is expected to do.
enum Expect {
    /// Lowers, and generates in every language and format.
    Ok,
    /// Fails with an error containing this text; the issue tracks the gap.
    Fails(&'static str, u32),
}

/// A schema: its files (rules it uses from another RFC come first), and what
/// it should do.
struct Case {
    files:  &'static [&'static str],
    expect: Expect,
}

const RECORD: &str = "a record array's optional members must come last";
const MAP_GROUP: &str = "a repeated group in a map";
const COMPUTED_KEYS: &str = "a map with more than one member with a computed key";
const OTHER_KEY: &str = "a computed key other than tstr alongside named members";

const CASES: &[Case] = &[
    Case { files: &["rfc/rfc8727.cddl"], expect: Expect::Fails(COMPUTED_KEYS, 136) },
    Case { files: &["rfc/rfc8927.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc8990.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc8990-cleaned.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9052.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9052.cddl", "rfc/rfc9053.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9054.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9090.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9115.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9164.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9165.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9171.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9171.cddl", "rfc/rfc9173.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9177.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9202.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9203.cddl"], expect: Expect::Fails(OTHER_KEY, 136) },
    Case { files: &["rfc/rfc9237.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9277.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9290.cddl"], expect: Expect::Fails(COMPUTED_KEYS, 136) },
    Case { files: &["rfc/rfc9321.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9052.cddl", "rfc/rfc9338.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9360.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9393.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9393-concise-swid-tag.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9393.cddl", "rfc/rfc9393-sign.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9393-sign1.cddl"], expect: Expect::Fails(OTHER_KEY, 136) },
    Case { files: &["rfc/rfc9393-concise-swid-tag.cddl", "rfc/rfc9393-tags.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9431.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9526.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9528.cddl"], expect: Expect::Fails(MAP_GROUP, 135) },
    Case { files: &["rfc/rfc9581.cddl"], expect: Expect::Fails(COMPUTED_KEYS, 136) },
    Case { files: &["rfc/rfc9594-error-handling.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9594-example-extended-scope-aif.cddl"], expect: Expect::Fails("parse error", 137) },
    Case { files: &["rfc/rfc9594-example-extended-scope-text.cddl"], expect: Expect::Fails("parse error", 137) },
    Case { files: &["rfc/rfc9237.cddl", "rfc/rfc9594-example-scope-aif.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9594-example-scope-text.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9594-get_creds.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9594-sign_info_entry-with-a-gene.cddl"], expect: Expect::Fails(RECORD, 134) },
    Case { files: &["rfc/rfc9594-sign_info-parameter.cddl"], expect: Expect::Ok },
    Case { files: &["rfc/rfc9597.cddl"], expect: Expect::Ok },
    Case { files: &["webdriver-bidi/webdriver-bidi.cddl"], expect: Expect::Ok },
];

const LANGS: &[&str] = &["rust", "c", "cpp", "csharp", "nodejs", "python", "dart", "go", "java", "lua"];

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus")
}

/// Runs `cddlc generate` on `files`; the error output when it fails.
fn generate(files: &[PathBuf], lang: &str, format: &str, out: &Path) -> Result<(), String> {
    let res = Command::new(env!("CARGO_BIN_EXE_cddlc"))
        .args(["generate", "-l", lang, "--format", format, "-o"])
        .arg(out)
        .args(files)
        .output()
        .expect("run cddlc");
    if res.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&res.stderr).into_owned())
    }
}

#[test]
fn every_corpus_file_is_listed() {
    let listed: Vec<&str> = CASES.iter().flat_map(|c| c.files.iter().copied()).collect();
    for dir in ["rfc", "webdriver-bidi"] {
        for entry in std::fs::read_dir(corpus().join(dir)).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if name.ends_with(".cddl") {
                let rel = format!("{dir}/{name}");
                assert!(listed.contains(&rel.as_str()), "{rel} has no case in CASES");
            }
        }
    }
}

#[test]
fn published_schemas() {
    let tmp = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for case in CASES {
        let files: Vec<PathBuf> = case.files.iter().map(|f| corpus().join(f)).collect();
        let name = case.files.join(" + ");
        match &case.expect {
            Expect::Ok => {
                for lang in LANGS {
                    for format in ["cbor", "json"] {
                        let out = tmp.path().join(format!("{}-{lang}-{format}", case.files.len()));
                        if let Err(e) = generate(&files, lang, format, &out) {
                            problems.push(format!("{name} ({lang}, {format}): {}", e.trim()));
                        }
                        let _ = std::fs::remove_dir_all(&out);
                    }
                }
            }
            Expect::Fails(text, issue) => match generate(&files, "rust", "cbor", &tmp.path().join("x")) {
                Ok(()) => problems.push(format!(
                    "{name} now generates: mark it Ok (and see whether dgtlrift/cddlc#{issue} is done)"
                )),
                Err(e) if e.contains(text) => {}
                Err(e) => problems.push(format!(
                    "{name}: expected an error containing {text:?} (dgtlrift/cddlc#{issue}), got: {}", e.trim()
                )),
            },
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}
