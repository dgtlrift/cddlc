/// Build validation tests.
///
/// These tests generate code from reference schemas and attempt to build
/// the output using the real language toolchain.
///
/// They are marked `#[ignore]` so they don't run in CI by default.
/// Run them explicitly:
///
///   cargo test -p backend-buildtest --test build_validation -- --ignored --nocapture
///
/// Individual language:
///   cargo test -p backend-buildtest --test build_validation test_build_rust -- --ignored --nocapture

use backend_buildtest::{
    assert_all_ok, build_c, build_cpp, build_csharp, build_nodejs,
    build_python, build_rust, run_build_tests, BuildOutcome, SCHEMAS,
};
use backend_rust::RustBackend;
use backend_c::CBackend;
use backend_cpp::CppBackend;
use backend_csharp::CSharpBackend;
use backend_nodejs::NodeJsBackend;
use backend_python::PythonBackend;
use cddlc_codegen::CodegenOptions;

fn work_dir(lang: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("cddlc-buildtest-{lang}-"))
        .tempdir()
        .expect("create temp dir")
}

// ── Schema generation smoke tests (always run, no toolchain needed) ───────────

#[test]
fn test_all_schemas_generate_rust() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &RustBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 3, "schema '{name}': expected 3 files");
    }
}

#[test]
fn test_all_schemas_generate_c() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 5, "schema '{name}': expected 5 files");
    }
}

#[test]
fn test_all_schemas_generate_cpp() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CppBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 5, "schema '{name}': expected 5 files");
    }
}

#[test]
fn test_all_schemas_generate_csharp() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CSharpBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 3, "schema '{name}': expected 3 files");
    }
}

#[test]
fn test_all_schemas_generate_nodejs() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &NodeJsBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 4, "schema '{name}': expected 4 files");
    }
}

#[test]
fn test_all_schemas_generate_python() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &PythonBackend, &opts)
            .unwrap_or_else(|e| panic!("schema '{name}' failed: {e}"));
        assert_eq!(files.len(), 3, "schema '{name}': expected 3 files");
    }
}

// ── Content spot checks (always run) ─────────────────────────────────────────

#[test]
fn test_iot_sensor_rust_types() {
    let src  = SCHEMAS.iter().find(|(n, _)| *n == "iot-sensor").unwrap().1;
    let files = backend_buildtest::generate_schema("iot-sensor", src, &RustBackend, &CodegenOptions::default()).unwrap();
    let lib_rs = files.iter().find(|f| f.path.ends_with("lib.rs")).unwrap();
    assert!(lib_rs.content.contains("pub struct SensorMessage"), "missing SensorMessage");
    assert!(lib_rs.content.contains("pub struct SensorReading"),  "missing SensorReading");
    assert!(lib_rs.content.contains("pub enum SensorType"),       "missing SensorType");
}

#[test]
fn test_iot_sensor_python_types() {
    let src  = SCHEMAS.iter().find(|(n, _)| *n == "iot-sensor").unwrap().1;
    let files = backend_buildtest::generate_schema("iot-sensor", src, &PythonBackend, &CodegenOptions::default()).unwrap();
    let py = files.iter().find(|f| f.path.ends_with(".py")).unwrap();
    assert!(py.content.contains("class SensorMessage:"), "missing SensorMessage");
    assert!(py.content.contains("class SensorReading:"),  "missing SensorReading");
    assert!(py.content.contains("class SensorType:"),     "missing SensorType");
}

#[test]
fn test_iot_sensor_csharp_types() {
    let src  = SCHEMAS.iter().find(|(n, _)| *n == "iot-sensor").unwrap().1;
    let files = backend_buildtest::generate_schema("iot-sensor", src, &CSharpBackend, &CodegenOptions::default()).unwrap();
    let cs = files.iter().find(|f| f.path.ends_with(".cs")).unwrap();
    assert!(cs.content.contains("class SensorMessage"), "missing SensorMessage");
    assert!(cs.content.contains("class SensorReading"),  "missing SensorReading");
    assert!(cs.content.contains("class SensorType"),     "missing SensorType");
}

#[test]
fn test_iot_sensor_nodejs_types() {
    let src  = SCHEMAS.iter().find(|(n, _)| *n == "iot-sensor").unwrap().1;
    let files = backend_buildtest::generate_schema("iot-sensor", src, &NodeJsBackend, &CodegenOptions::default()).unwrap();
    let ts = files.iter().find(|f| f.path.ends_with(".ts") && f.path.contains("/src/")).unwrap();
    assert!(ts.content.contains("SensorMessage"), "missing SensorMessage");
    assert!(ts.content.contains("SensorReading"),  "missing SensorReading");
    assert!(ts.content.contains("SensorType"),     "missing SensorType");
}

#[test]
fn test_dcbor_generates_for_all_schemas() {
    let mut opts = CodegenOptions::default();
    opts.dcbor = true;
    for (name, src) in SCHEMAS {
        let _ = backend_buildtest::generate_schema(name, src, &RustBackend, &opts)
            .unwrap_or_else(|e| panic!("dcbor schema '{name}' failed: {e}"));
    }
}

// ── CMake / C++ generated content checks (always run, no toolchain needed) ───

/// Generated CMakeLists.txt must use FetchContent_MakeAvailable (not the
/// deprecated FetchContent_Populate) to fetch and build nanocbor.
#[test]
fn test_cmake_uses_fetchcontent_make_available() {
    let opts = CodegenOptions::default();
    for backend in [
        ("C",   backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CBackend, &opts).unwrap()),
        ("C++", backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CppBackend, &opts).unwrap()),
    ] {
        let (lang, files) = backend;
        let cmake = files.iter().find(|f| f.path.ends_with("CMakeLists.txt"))
            .unwrap_or_else(|| panic!("{lang}: no CMakeLists.txt generated"));
        assert!(cmake.content.contains("FetchContent_MakeAvailable"),
            "{lang}: CMakeLists.txt must use FetchContent_MakeAvailable to fetch nanocbor");
        assert!(!cmake.content.contains("FetchContent_Populate"),
            "{lang}: CMakeLists.txt must not use the deprecated FetchContent_Populate");
    }
}

/// Generated CMakeLists.txt must use file(GLOB) to compile nanocbor sources
/// directly (the nanocbor CMake target is not reliably built in all environments).
#[test]
fn test_cmake_globs_nanocbor_sources() {
    let opts = CodegenOptions::default();
    for backend in [
        ("C",   backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CBackend, &opts).unwrap()),
        ("C++", backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CppBackend, &opts).unwrap()),
    ] {
        let (lang, files) = backend;
        let cmake = files.iter().find(|f| f.path.ends_with("CMakeLists.txt"))
            .unwrap_or_else(|| panic!("{lang}: no CMakeLists.txt generated"));
        assert!(cmake.content.contains("file(GLOB"),
            "{lang}: CMakeLists.txt must use file(GLOB) to collect nanocbor sources");
        assert!(!cmake.content.contains("nanocbor.c"),
            "{lang}: CMakeLists.txt must not reference nanocbor.c by name");
    }
}

/// Generated CMakeLists.txt must explicitly add nanocbor's include dir so
/// `#include <nanocbor/nanocbor.h>` resolves during compilation of our sources.
#[test]
fn test_cmake_includes_nanocbor_headers() {
    let opts = CodegenOptions::default();
    for backend in [
        ("C",   backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CBackend, &opts).unwrap()),
        ("C++", backend_buildtest::generate_schema("structs", SCHEMAS.iter().find(|(n,_)| *n == "structs").unwrap().1, &CppBackend, &opts).unwrap()),
    ] {
        let (lang, files) = backend;
        let cmake = files.iter().find(|f| f.path.ends_with("CMakeLists.txt"))
            .unwrap_or_else(|| panic!("{lang}: no CMakeLists.txt generated"));
        assert!(cmake.content.contains("nanocbor_SOURCE_DIR}/include"),
            "{lang}: CMakeLists.txt must add ${{nanocbor_SOURCE_DIR}}/include to target_include_directories");
    }
}

/// Generated C++ header must not use `nanocbor_put_tstr_len` (renamed to
/// `nanocbor_put_tstrn` in current NanoCBOR).
#[test]
fn test_cpp_uses_nanocbor_put_tstrn() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CppBackend, &opts).unwrap();
        let cpp = files.iter().find(|f| f.path.ends_with(".cpp") && f.path.contains("/src/")).unwrap();
        assert!(!cpp.content.contains("nanocbor_put_tstr_len"),
            "schema '{name}': .cpp must not use removed nanocbor_put_tstr_len (use nanocbor_put_tstrn)");
    }
}

/// Generated C source must not use `nanocbor_put_tstr_len` (renamed to
/// `nanocbor_put_tstrn` in current NanoCBOR).
#[test]
fn test_c_uses_nanocbor_put_tstrn() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CBackend, &opts).unwrap();
        let c_src = files.iter().find(|f| f.path.ends_with(".c") && f.path.contains("/src/")).unwrap();
        assert!(!c_src.content.contains("nanocbor_put_tstr_len"),
            "schema '{name}': .c must not use removed nanocbor_put_tstr_len (use nanocbor_put_tstrn)");
    }
}

/// Generated C++ text enums record their variant in `kind` (dgtlrift/cddlc#49):
/// a decoded literal maps to its no-argument constructor, and encoding writes the
/// variant's own literal rather than a stored string.
#[test]
fn test_cpp_enum_tstr_decode_sets_kind() {
    let opts = CodegenOptions::default();
    let src = SCHEMAS.iter().find(|(n, _)| *n == "enums").unwrap().1;
    let files = backend_buildtest::generate_schema("enums", src, &CppBackend, &opts).unwrap();
    let cpp = files.iter().find(|f| f.path.ends_with(".cpp") && f.path.contains("/src/")).unwrap();
    assert!(cpp.content.contains("if (sv == std::string_view(\"ok\")) { out = Status::make_Ok(); return NANOCBOR_OK; }"),
        "enums .cpp: \"ok\" must decode to Status::make_Ok()");
    assert!(cpp.content.contains("case Status::Kind::Warn:\n        nanocbor_put_tstrn(enc, \"warn\", 4);"),
        "enums .cpp: Status::Kind::Warn must encode as \"warn\"");
}

/// Generated C++ source for structs with optional fields must compute the map
/// length at runtime (has_value()) and skip key+value when the field is absent.
#[test]
fn test_cpp_optional_fields_dynamic_map_length() {
    let opts = CodegenOptions::default();
    let src = SCHEMAS.iter().find(|(n, _)| *n == "structs").unwrap().1;
    let files = backend_buildtest::generate_schema("structs", src, &CppBackend, &opts).unwrap();
    let cpp = files.iter().find(|f| f.path.ends_with(".cpp") && f.path.contains("/src/")).unwrap();
    assert!(cpp.content.contains("has_value()"),
        "structs .cpp: optional fields must use has_value() for runtime map length and conditional emit");
}

/// Generated C++ array operator== must not use `!=` on elements (only `==` is
/// defined on generated structs).
#[test]
fn test_cpp_array_operator_eq_no_neq() {
    let opts = CodegenOptions::default();
    for (name, src) in SCHEMAS {
        let files = backend_buildtest::generate_schema(name, src, &CppBackend, &opts).unwrap();
        let hpp = files.iter().find(|f| f.path.ends_with(".hpp")).unwrap();
        assert!(!hpp.content.contains("items[i] != o.items[i]"),
            "schema '{name}': .hpp array operator== must not use != on elements (only operator== is defined)");
    }
}

/// Generated C++ size-constrained alias struct must default-initialize its
/// `value` field to a string of the required length so default-constructed
/// instances are valid for encode/decode roundtrip.
#[test]
fn test_cpp_size_constrained_alias_has_valid_default() {
    let opts = CodegenOptions::default();
    let src = SCHEMAS.iter().find(|(n, _)| *n == "aliases").unwrap().1;
    let files = backend_buildtest::generate_schema("aliases", src, &CppBackend, &opts).unwrap();
    let hpp = files.iter().find(|f| f.path.ends_with(".hpp")).unwrap();
    // device-name = tstr .size 32 — default value should be a 32-char string
    assert!(hpp.content.contains("\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"") ||
            hpp.content.contains("32)"),
        "aliases .hpp: DeviceName default value must be a 32-char string to satisfy .size 32 constraint");
}

/// A default-constructed C++ enum is its first variant (dgtlrift/cddlc#49).
#[test]
fn test_cpp_enum_tstr_has_valid_default() {
    let opts = CodegenOptions::default();
    let src = SCHEMAS.iter().find(|(n, _)| *n == "enums").unwrap().1;
    let files = backend_buildtest::generate_schema("enums", src, &CppBackend, &opts).unwrap();
    let hpp = files.iter().find(|f| f.path.ends_with(".hpp")).unwrap();
    // status = "ok" / "warn" / "error"
    assert!(hpp.content.contains("enum class Kind { Ok, Warn, Error };"), "{}", hpp.content);
    assert!(hpp.content.contains("Kind kind{Kind::Ok};"), "enums .hpp: Status must default to Kind::Ok");
}

/// Generated C++ decode functions must use the container cursor (&map / &arr)
/// when reading field values inside map and array loops — not the outer `dec`.
/// A reliable signal: inside a struct decode, field reads go through `&map`.
#[test]
fn test_cpp_decode_uses_container_cursor() {
    let opts = CodegenOptions::default();
    let src = SCHEMAS.iter().find(|(n, _)| *n == "structs").unwrap().1;
    let files = backend_buildtest::generate_schema("structs", src, &CppBackend, &opts).unwrap();
    let cpp = files.iter().find(|f| f.path.ends_with(".cpp") && f.path.contains("/src/")).unwrap();
    // decode(dec, ...) inside a map would read from the outer cursor — wrong.
    // All field decodes inside a map must pass &map.
    assert!(cpp.content.contains("decode(&map,"),
        "structs .cpp: named-type field decode inside a map must pass &map, not dec");
}

// ── Actual build tests (ignored by default, require installed toolchain) ──────

/// Build and test all schemas using the Rust backend (runs `cargo test`).
/// Requires: cargo in PATH
/// Run: cargo test -p backend-buildtest --test build_validation test_build_rust -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_rust() {
    let tmp  = work_dir("rust");
    let opts = CodegenOptions::default();
    eprintln!("\n=== Rust build test ===");
    let outcomes = run_build_tests(&RustBackend, |d, f, n| build_rust(d, f, n), &opts, tmp.path());
    print_summary("Rust", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the C backend (cmake build + ctest).
/// Requires: cmake, cc in PATH
/// Run: cargo test -p backend-buildtest --test build_validation test_build_c -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_c() {
    let tmp  = work_dir("c");
    let opts = CodegenOptions::default();
    eprintln!("\n=== C build test ===");
    let outcomes = run_build_tests(&CBackend, |d, f, n| build_c(d, f, n), &opts, tmp.path());
    print_summary("C", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the C++ backend (cmake build + ctest).
/// Requires: cmake, c++ in PATH
/// Run: cargo test -p backend-buildtest --test build_validation test_build_cpp -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_cpp() {
    let tmp  = work_dir("cpp");
    let opts = CodegenOptions::default();
    eprintln!("\n=== C++ build test ===");
    let outcomes = run_build_tests(&CppBackend, |d, f, n| build_cpp(d, f, n), &opts, tmp.path());
    print_summary("C++", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the C# backend (runs `dotnet test`).
/// Requires: dotnet in PATH
/// Run: cargo test -p backend-buildtest --test build_validation test_build_csharp -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_csharp() {
    let tmp  = work_dir("csharp");
    let opts = CodegenOptions::default();
    eprintln!("\n=== C# build test ===");
    let outcomes = run_build_tests(&CSharpBackend, |d, f, n| build_csharp(d, f, n), &opts, tmp.path());
    print_summary("C#", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the Node.js backend (npm install + tsc + jest).
/// Requires: npm in PATH
/// Run: cargo test -p backend-buildtest --test build_validation test_build_nodejs -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_nodejs() {
    let tmp  = work_dir("nodejs");
    let opts = CodegenOptions::default();
    eprintln!("\n=== Node.js build test ===");
    let outcomes = run_build_tests(&NodeJsBackend, |d, f, n| build_nodejs(d, f, n), &opts, tmp.path());
    print_summary("Node.js", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the Python backend (compileall + pytest).
/// Requires: a Python with cbor2>=6.0 and pytest — python3 from PATH, or the
/// interpreter named by CDDLC_PYTHON (e.g. a venv's bin/python). Skips otherwise.
/// Run: cargo test -p backend-buildtest --test build_validation test_build_python -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_python() {
    let tmp  = work_dir("python");
    let opts = CodegenOptions::default();
    eprintln!("\n=== Python build test ===");
    let outcomes = run_build_tests(&PythonBackend, |d, f, n| build_python(d, f, n), &opts, tmp.path());
    print_summary("Python", &outcomes);
    assert_all_ok(&outcomes);
}

/// Build and test all schemas using the Python backend's JSON output.
/// Requires: as test_build_python.
/// Run: cargo test -p backend-buildtest --test build_validation test_build_python_json -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_python_json() {
    let tmp  = work_dir("python-json");
    let opts = CodegenOptions { format: cddlc_codegen::Format::Json, ..CodegenOptions::default() };
    eprintln!("\n=== Python JSON build test ===");
    let outcomes = run_build_tests(&PythonBackend, |d, f, n| build_python(d, f, n), &opts, tmp.path());
    print_summary("Python JSON", &outcomes);
    assert_all_ok(&outcomes);
}

/// Run all language build tests in sequence with a final summary table.
/// Run: cargo test -p backend-buildtest --test build_validation test_build_all -- --ignored --nocapture
#[test]
#[ignore]
fn test_build_all() {
    let opts = CodegenOptions::default();

    eprintln!("\n╔══════════════════════════════════════╗");
    eprintln!("║   cddlc backend build validation    ║");
    eprintln!("╚══════════════════════════════════════╝");

    let mut all_outcomes: Vec<(&str, Vec<BuildOutcome>)> = Vec::new();

    macro_rules! run_lang {
        ($label:expr, $backend:expr, $build_fn:expr, $suffix:expr) => {{
            eprintln!("\n─── {} ───────────────────────────", $label);
            let tmp = work_dir($suffix);
            let outcomes = run_build_tests(&$backend, $build_fn, &opts, tmp.path());
            print_summary($label, &outcomes);
            all_outcomes.push(($label, outcomes));
        }};
    }

    run_lang!("Rust",    RustBackend,   |d, f, n| build_rust(d, f, n),    "rust");
    run_lang!("C",       CBackend,      |d, f, n| build_c(d, f, n),       "c");
    run_lang!("C++",     CppBackend,    |d, f, n| build_cpp(d, f, n),     "cpp");
    run_lang!("C#",      CSharpBackend, |d, f, n| build_csharp(d, f, n),  "csharp");
    run_lang!("Node.js", NodeJsBackend, |d, f, n| build_nodejs(d, f, n),  "nodejs");
    run_lang!("Python",  PythonBackend, |d, f, n| build_python(d, f, n),  "python");

    eprintln!("\n╔════════════╦═══════╦═══════╦═══════╗");
    eprintln!("║ Language   ║  OK   ║ SKIP  ║ FAIL  ║");
    eprintln!("╠════════════╬═══════╬═══════╬═══════╣");
    for (lang, outcomes) in &all_outcomes {
        let ok   = outcomes.iter().filter(|o| matches!(o, BuildOutcome::Built { .. })).count();
        let skip = outcomes.iter().filter(|o| o.is_skipped()).count();
        let fail = outcomes.iter().filter(|o| !o.is_ok()).count();
        eprintln!("║ {lang:<10} ║  {ok:<4} ║  {skip:<4} ║  {fail:<4} ║");
    }
    eprintln!("╚════════════╩═══════╩═══════╩═══════╝");

    for (_lang, outcomes) in &all_outcomes {
        assert_all_ok(outcomes);
    }
}

fn print_summary(lang: &str, outcomes: &[BuildOutcome]) {
    let ok   = outcomes.iter().filter(|o| matches!(o, BuildOutcome::Built { .. })).count();
    let skip = outcomes.iter().filter(|o| o.is_skipped()).count();
    let fail = outcomes.iter().filter(|o| !o.is_ok()).count();
    eprintln!("  {lang}: {ok} built, {skip} skipped, {fail} failed / {} schemas", outcomes.len());
    if let Some(BuildOutcome::Skipped { reason }) = outcomes.first() {
        eprintln!("  (skip reason: {reason})");
    }
}
