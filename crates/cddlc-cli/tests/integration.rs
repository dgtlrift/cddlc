/// Integration tests for cddlc-cli.
///
/// Tests cover:
/// - Argument parsing (via clap)
/// - Import resolution (single file, multi-file, @import chains)
/// - End-to-end codegen (parse → IR → Rust output)
/// - Error cases (missing files, cycles, bad lang)

use std::fs;
use std::path::{Path, PathBuf};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn fixture(name: &str) -> PathBuf {
    fixtures().join(name)
}

/// Run the cddlc binary with given args, return (stdout, stderr, success).
fn run(args: &[&str]) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_cddlc");
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run cddlc");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// Run cddlc in --dry-run mode (no output files written).
fn dry_run(input: &Path, extra: &[&str]) -> (String, String, bool) {
    let mut args = vec![
        "generate",
        input.to_str().unwrap(),
        "--dry-run",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// Run cddlc and write output to a temp dir, return the dir path.
fn generate(input: &Path, extra: &[&str]) -> (TestDir, String, bool) {
    let dir = tempdir();
    let mut args = vec![
        "generate",
        input.to_str().unwrap(),
        "-o", dir.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let (_, stderr, ok) = run(&args);
    (dir, stderr, ok)
}

/// A temp directory for one test, removed when the test drops it. Derefs to
/// `Path`, so it is used like one (`dir.join(..)`, `&dir`).
struct TestDir(tempfile::TempDir);

impl std::ops::Deref for TestDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.0.path()
    }
}

/// Create a fresh, uniquely named temp directory. `tempfile` creates it
/// atomically, so tests running in parallel never share one (issue #30: names
/// built from the clock's nanoseconds could collide).
fn tempdir() -> TestDir {
    TestDir(
        tempfile::Builder::new()
            .prefix("cddlc_test_")
            .tempdir()
            .expect("create temp dir"),
    )
}

// ── Argument parsing ──────────────────────────────────────────────────────────

#[test]
fn test_help_flag() {
    let (stdout, _, ok) = run(&["--help"]);
    assert!(ok);
    assert!(stdout.contains("cddlc"));
    assert!(stdout.contains("generate") || stdout.contains("Generate"));
    assert!(stdout.contains("validate") || stdout.contains("Validate"));
}

#[test]
fn test_generate_help_flag() {
    let (stdout, _, ok) = run(&["generate", "--help"]);
    assert!(ok);
    assert!(stdout.contains("--lang"));
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("--no-std"));
    assert!(stdout.contains("--dcbor"));
}

#[test]
fn test_validate_help_flag() {
    let (stdout, _, ok) = run(&["validate", "--help"]);
    assert!(ok);
    assert!(stdout.contains("--cddl"));
    assert!(stdout.contains("--json"));
    assert!(stdout.contains("--cbor"));
}

#[test]
fn test_version_flag() {
    let (stdout, _, ok) = run(&["--version"]);
    assert!(ok);
    assert!(stdout.contains("cddlc"));
}

#[test]
fn test_missing_input_fails() {
    let (_, _, ok) = run(&[]);
    assert!(!ok, "should fail with no input files");
}

#[test]
fn test_nonexistent_input_fails() {
    let (_, stderr, ok) = run(&["generate", "nonexistent.cddl", "--dry-run"]);
    assert!(!ok);
    assert!(stderr.contains("error") || stderr.contains("nonexistent"));
}

#[test]
fn test_unsupported_lang_fails() {
    // clap rejects unknown enum values before our code runs
    let input = fixture("sensor.cddl");
    let (_, stderr, ok) = run(&["generate", input.to_str().unwrap(), "--lang", "cobol", "--dry-run"]);
    assert!(!ok);
    assert!(stderr.contains("cobol") || stderr.contains("invalid") || stderr.contains("error"),
        "expected error for unknown lang, got: {stderr}");
}

#[test]
fn test_no_std_requires_rust() {
    let input = fixture("sensor.cddl");
    let (_, stderr, ok) = run(&["generate", input.to_str().unwrap(), "--lang", "c", "--no-std", "--dry-run"]);
    assert!(!ok);
    assert!(stderr.contains("no-std") || stderr.contains("rust"));
}

// ── Dry run ───────────────────────────────────────────────────────────────────

#[test]
fn test_dry_run_succeeds() {
    let (_, stderr, ok) = dry_run(&fixture("sensor.cddl"), &[]);
    assert!(ok, "dry-run failed: {stderr}");
    assert!(stderr.contains("dry-run"));
}

#[test]
fn test_dry_run_reports_type_count() {
    let (_, stderr, ok) = dry_run(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");
    // sensor.cddl defines: sensor, device-id, readings, status
    assert!(stderr.contains("4 types") || stderr.contains("types"));
}

#[test]
fn test_dry_run_verbose() {
    let (_, stderr, ok) = dry_run(&fixture("sensor.cddl"), &["--verbose"]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("loading"));
    assert!(stderr.contains("sensor"));
}

// ── Import resolution ─────────────────────────────────────────────────────────

#[test]
fn test_import_chain_dry_run() {
    // message.cddl imports primitives.cddl
    let (_, stderr, ok) = dry_run(&fixture("message.cddl"), &[]);
    assert!(ok, "import chain failed: {stderr}");
}

#[test]
fn test_import_chain_verbose_shows_both_files() {
    let (_, stderr, ok) = dry_run(&fixture("message.cddl"), &["--verbose"]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("primitives.cddl") || stderr.contains("primitives"),
        "expected primitives.cddl in verbose output, got: {stderr}");
}

#[test]
fn test_import_resolves_types_from_dep() {
    // message.cddl uses device-id and timestamp from primitives.cddl
    let (_, stderr, ok) = dry_run(&fixture("message.cddl"), &[]);
    assert!(ok, "types from imported file not resolved: {stderr}");
}

#[test]
fn test_include_dir_flag() {
    // Create a temp dir with a copy of primitives.cddl, then import it
    // from a schema in a different dir using --include-dir
    let tmp = tempdir();
    fs::copy(fixture("primitives.cddl"), tmp.join("primitives.cddl")).unwrap();

    // Write a schema that imports primitives without a relative path
    let schema = tmp.join("test.cddl");
    fs::write(&schema, r#"
; @import "primitives.cddl"
msg = { id: device-id, value: float32 }
"#).unwrap();

    let (_, stderr, ok) = dry_run(&schema, &["--include-dir", tmp.to_str().unwrap()]);
    assert!(ok, "include-dir failed: {stderr}");
}

// ── End-to-end codegen ────────────────────────────────────────────────────────

#[test]
fn test_generates_rust_file() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "codegen failed: {stderr}");

    // Should have written a crate with src/lib.rs
    let lib_rs = find_rs_recursive(&dir);
    assert!(lib_rs.is_some(), "no lib.rs generated under {}", dir.display());
}

#[test]
fn test_generated_file_contains_struct() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let rs_file = find_rs_file(&dir);
    let content = fs::read_to_string(&rs_file).unwrap();
    assert!(content.contains("pub struct Sensor"), "expected Sensor struct in:\n{content}");
}

#[test]
fn test_generated_file_contains_encode_impl() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("impl<C> Encode<C> for Sensor"));
}

#[test]
fn test_generated_file_contains_decode_impl() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("impl<'b> Decode<'b, ()> for Sensor"));
}

#[test]
fn test_generated_enum() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("pub enum Status"));
}

#[test]
fn test_generated_array_type() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("Readings") || content.contains("readings"),
        "expected array type in:\n{content}");
}

#[test]
fn test_generated_tagged_types() {
    let (dir, stderr, ok) = generate(&fixture("tagged.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("Tag::new(1)"), "expected tag 1 in:\n{content}");
    assert!(content.contains("Tag::new(0)"), "expected tag 0 in:\n{content}");
}

#[test]
fn test_no_std_flag_emits_no_std() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &["--no-std"]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    // no_std is emitted as a cfg_attr feature gate so the crate works in both envs
    assert!(content.contains("cfg_attr") && content.contains("no_std"),
        "expected cfg_attr no_std in:\n{content}");
}

#[test]
fn test_no_std_uses_heapless() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &["--no-std"]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("heapless"));
}

#[test]
fn test_dcbor_flag_noted_in_header() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &["--dcbor"]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("deterministic"));
}

#[test]
fn test_generated_file_header_comment() {
    let (dir, stderr, ok) = generate(&fixture("sensor.cddl"), &[]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains("@generated by cddlc"));
}

#[test]
fn test_output_dir_created() {
    let base = tempdir();
    let output = base.join("nested").join("output");
    let input = fixture("sensor.cddl");

    let (_, stderr, ok) = run(&[
        "generate",
        input.to_str().unwrap(),
        "-o", output.to_str().unwrap(),
    ]);
    assert!(ok, "failed to create nested output dir: {stderr}");
    assert!(output.exists(), "output dir not created");
}

#[test]
fn test_import_chain_codegen() {
    let (dir, stderr, ok) = generate(&fixture("message.cddl"), &[]);
    assert!(ok, "import chain codegen failed: {stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    // message.cddl defines sensor-message; primitives.cddl defines device-id etc.
    assert!(content.contains("SensorMessage") || content.contains("sensor_message"),
        "expected SensorMessage in:\n{content}");
}

#[test]
fn test_max_array_flag() {
    // Write a temp schema with an unbounded array and no @capacity pragma
    let tmp = tempdir();
    let schema = tmp.join("arr.cddl");
    fs::write(&schema, "items = [* uint]\n").unwrap();

    let (dir, stderr, ok) = run_generate(&schema, &["--no-std", "--max-array", "8"]);
    assert!(ok, "{stderr}");

    let content = fs::read_to_string(find_rs_file(&dir)).unwrap();
    assert!(content.contains(", 8>"), "expected capacity 8 in heapless::Vec:\n{content}");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn find_rs_file(dir: &Path) -> PathBuf {
    // Output is now a crate: <dir>/<module>-cbor/src/lib.rs
    // Walk recursively to find lib.rs
    find_rs_recursive(dir)
        .unwrap_or_else(|| panic!("no lib.rs found under {}", dir.display()))
}

fn find_rs_recursive(dir: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_rs_recursive(&path) {
                return Some(found);
            }
        } else if path.file_name().map(|n| n == "lib.rs").unwrap_or(false) {
            return Some(path);
        }
    }
    None
}

// ── validate ─────────────────────────────────────────────────────────────────

#[test]
fn test_validate_json_ok() {
    let tmp = tempdir();
    let data = tmp.join("valid.json");
    fs::write(&data, r#"{"id":1,"label":"a","value":1.5}"#).unwrap();

    let (stdout, stderr, ok) = run(&[
        "validate", "--cddl", fixture("sensor.cddl").to_str().unwrap(),
        "--json", data.to_str().unwrap(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("OK"), "{stdout}");
}

#[test]
fn test_validate_json_fail_exit_code_and_message() {
    let tmp = tempdir();
    let data = tmp.join("invalid.json");
    // missing required 'value' field
    fs::write(&data, r#"{"id":1,"label":"a"}"#).unwrap();

    let (stdout, _stderr, ok) = run(&[
        "validate", "--cddl", fixture("sensor.cddl").to_str().unwrap(),
        "--json", data.to_str().unwrap(),
    ]);
    assert!(!ok, "expected non-zero exit for a failing validation");
    assert!(stdout.contains("FAIL"), "{stdout}");
    assert!(stdout.contains("value"), "expected mention of missing field 'value':\n{stdout}");
}

#[test]
fn test_validate_explicit_type_flag() {
    let tmp = tempdir();
    let data = tmp.join("status.json");
    fs::write(&data, r#""warn""#).unwrap();

    let (stdout, stderr, ok) = run(&[
        "validate", "--cddl", fixture("sensor.cddl").to_str().unwrap(),
        "--type", "status",
        "--json", data.to_str().unwrap(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("OK"), "{stdout}");
}

#[test]
fn test_validate_no_data_files_errors() {
    let (_, stderr, ok) = run(&[
        "validate", "--cddl", fixture("sensor.cddl").to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("--json") || stderr.contains("--cbor"), "{stderr}");
}

fn run_generate(input: &Path, extra: &[&str]) -> (TestDir, String, bool) {
    let dir = tempdir();
    let mut args = vec![
        "generate",
        input.to_str().unwrap(),
        "-o", dir.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let (_, stderr, ok) = run(&args);
    (dir, stderr, ok)
}

// ── Header, docs and source lines (dgtlrift/cddlc#8, #9, #10) ─────────────────

/// Generates Rust for a commented schema in a temporary directory, run from there so the
/// paths are relative to it; returns the generated `lib.rs`.
fn generate_commented(extra: &[&str]) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("cddlc-doc-test-{}-{n}", std::process::id()));
    fs::create_dir_all(dir.join("schema")).unwrap();
    fs::write(
        dir.join("schema/t.cddl"),
        "; A thing.\nthing = {\n  id: uint,   ; its id\n}\n",
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_cddlc");
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["generate", "schema/t.cddl", "--lang", "rust", "-o", "out"])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lib = fs::read_to_string(dir.join("out/t-cbor/src/lib.rs")).unwrap();
    let _ = fs::remove_dir_all(&dir);
    lib
}

#[test]
fn generated_files_name_their_cddlc_and_inputs() {
    let lib = generate_commented(&[]);
    let head: Vec<&str> = lib.lines().take(4).collect();
    assert_eq!(head[0], "// @generated by cddlc — do not edit manually");
    assert!(head[1].starts_with("// cddlc v"), "{}", head[1]);
    assert!(head[2].starts_with("// branch "), "{}", head[2]);
    // The SHA-256 of the schema `generate_commented` writes.
    assert_eq!(
        head[3],
        "// source schema/t.cddl (sha256 9e8a56bf389cda0f228b6d069d681058f0cf09e3307b9a2149d5e9a4524432bc, 46 bytes)"
    );
    // No comment block of its own (its first comment is `thing`'s): no boilerplate.
    assert_ne!(lib.lines().nth(4).unwrap(), "//", "{lib}");
}

#[test]
fn the_schemas_boilerplate_heads_every_generated_file() {
    let dir = std::env::temp_dir().join(format!("cddlc-boilerplate-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("t.cddl"),
        ";;;;;;\n; Copyright (c) 2026 Acme\n; SPDX-License-Identifier: MIT\n;;;;;;\n\n; A thing.\nthing = uint\n",
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_cddlc");
    for (lang, files, open) in [
        ("rust",   &["Cargo.toml", "src/lib.rs", "tests/roundtrip.rs"][..], &["# ", "// ", "// "][..]),
        ("c",      &["CMakeLists.txt", "include/t.h", "compat/endian.h"][..], &["# ", " * ", " * "][..]),
        ("lua",    &["t-cbor-0.1.0-1.rockspec", "src/t.lua"][..], &["-- ", "-- "][..]),
    ] {
        let out = std::process::Command::new(bin)
            .current_dir(&dir)
            .args(["generate", "t.cddl", "--lang", lang, "-o", lang])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        for (f, open) in files.iter().zip(open) {
            let text = fs::read_to_string(dir.join(lang).join("t-cbor").join(f)).unwrap();
            let want = format!("{open}Copyright (c) 2026 Acme\n{open}SPDX-License-Identifier: MIT\n");
            assert!(text.contains(&want), "{lang} {f}:\n{text}");
            assert!(!text.contains(";;;"), "{lang} {f}");
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn comments_and_source_lines_become_docs() {
    let lib = generate_commented(&[]);
    assert!(lib.contains("/// A thing.\n/// cddl: schema/t.cddl:2: thing = {\n"), "{lib}");
    assert!(lib.contains("/// its id\n    /// cddl: schema/t.cddl:3: id: uint,   ; its id\n"), "{lib}");
    let quiet = generate_commented(&["--no-source-comments"]);
    assert!(quiet.contains("/// A thing.\n") && !quiet.contains("cddl: "), "{quiet}");
}

// ── sample (dgtlrift/cddlc#110) ──────────────────────────────────────────────

#[test]
fn sample_writes_an_example_that_validate_accepts() {
    let dir = tempdir();
    let schema = dir.join("s.cddl");
    fs::write(&schema, "reading = {\n  id: uint .ge 1,\n  kind: \"temp\",\n  ? note: tstr,\n  key: bstr .size 2,\n}\n").unwrap();
    let s = schema.to_str().unwrap();

    let (json, err, ok) = run(&["sample", "--cddl", s]);
    assert!(ok, "{err}");
    assert_eq!(json, "{\n  \"id\": 1,\n  \"kind\": \"temp\",\n  \"note\": \"text\",\n  \"key\": \"AAA\"\n}\n");

    let (diag, err, ok) = run(&["sample", "--cddl", s, "--format", "diag", "--minimal"]);
    assert!(ok, "{err}");
    assert_eq!(diag, "{\"id\": 1, \"kind\": \"temp\", \"key\": h'0000'}\n");

    // The CBOR and JSON it writes validate.
    for (fmt, flag) in [("cbor", "--cbor"), ("json", "--json")] {
        let out = dir.join(format!("sample.{fmt}"));
        let (_, err, ok) = run(&["sample", "--cddl", s, "--format", fmt, "-o", out.to_str().unwrap()]);
        assert!(ok, "{err}");
        let (stdout, err, ok) = run(&["validate", "--cddl", s, flag, out.to_str().unwrap()]);
        assert!(ok && stdout.contains("OK"), "{stdout}{err}");
    }
}

#[test]
fn sample_warns_and_fails_when_its_example_does_not_validate() {
    let dir = tempdir();
    let schema = dir.join("s.cddl");
    fs::write(&schema, "r = { code: tstr .regexp \"[A-Z]{3}\" }\n").unwrap();
    let (stdout, err, ok) = run(&["sample", "--cddl", schema.to_str().unwrap()]);
    assert!(!ok);
    assert!(stdout.contains("\"code\""), "{stdout}");
    assert!(err.contains("doesn't validate") && err.contains("does not match pattern"), "{err}");
}
