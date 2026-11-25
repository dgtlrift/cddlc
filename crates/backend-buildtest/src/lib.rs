/// Build validation harness for cddlc backends.
///
/// Generates code from reference CDDL schemas using each backend,
/// writes it to a temporary directory, then invokes the language's
/// native toolchain to verify the generated code actually compiles.
///
/// Tests skip gracefully when a required toolchain is not installed, and say how
/// to install it (see [`toolchain`]).

pub mod toolchain;

use toolchain::Lang;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cddlc_codegen::{Backend, CodegenOptions, GeneratedFile};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

// ── Public schema list ────────────────────────────────────────────────────────

/// All reference schemas bundled with this crate.
pub const SCHEMAS: &[(&str, &str)] = &[
    ("primitives",  include_str!("../schemas/primitives.cddl")),
    ("structs",     include_str!("../schemas/structs.cddl")),
    ("enums",       include_str!("../schemas/enums.cddl")),
    ("arrays",      include_str!("../schemas/arrays.cddl")),
    ("aliases",     include_str!("../schemas/aliases.cddl")),
    ("tagged",      include_str!("../schemas/tagged.cddl")),
    ("iot-sensor",  include_str!("../schemas/iot_sensor.cddl")),
    ("simple-values", include_str!("../schemas/simple_values.cddl")),
    ("choices",     include_str!("../schemas/choices.cddl")),
    ("prelude",     include_str!("../schemas/prelude.cddl")),
    ("records",     include_str!("../schemas/records.cddl")),
    ("maps",        include_str!("../schemas/maps.cddl")),
    ("groups",      include_str!("../schemas/groups.cddl")),
    ("inline-enum", include_str!("../schemas/inline-enum-in-group-after-comment.cddl")),
    ("names",       include_str!("../schemas/names.cddl")),
    ("bits",        include_str!("../schemas/bits.cddl")),
    ("repeated-groups", include_str!("../schemas/repeated_groups.cddl")),
];

// ── Result types ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BuildOutcome {
    /// Toolchain not found — test skipped.
    Skipped { reason: String },
    /// Code generated and build succeeded.
    Built { schema: String, files: usize, build_ms: u64 },
    /// Code generated but build failed.
    Failed { schema: String, stdout: String, stderr: String, exit_code: Option<i32> },
    /// Code generation itself failed.
    CodegenError { schema: String, error: String },
    /// The build couldn't fetch its dependencies: a package registry was
    /// unavailable (rate-limited, down, unreachable), after retries. Not a
    /// failure of the generated code, so it doesn't fail the test.
    Unavailable { schema: String, reason: String },
}

impl BuildOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, BuildOutcome::Skipped { .. } | BuildOutcome::Built { .. } | BuildOutcome::Unavailable { .. })
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self, BuildOutcome::Unavailable { .. })
    }

    pub fn is_skipped(&self) -> bool {
        matches!(self, BuildOutcome::Skipped { .. })
    }
}

// ── Toolchain probing ─────────────────────────────────────────────────────────

/// Returns `Ok(path)` if `tool` exists in PATH, `Err(reason)` otherwise.
pub fn find_tool(tool: &str) -> Result<String, String> {
    let status = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(tool)
        .output();

    match status {
        Ok(out) if out.status.success() => {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => Err(format!("'{tool}' not found in PATH")),
    }
}

/// Check multiple tools; return first error.
pub fn require_tools(tools: &[&str]) -> Result<(), String> {
    for tool in tools {
        find_tool(tool)?;
    }
    Ok(())
}

// ── Code generation ───────────────────────────────────────────────────────────

/// Generate files for one schema using the given backend.
pub fn generate_schema(
    schema_name: &str,
    schema_src:  &str,
    backend:     &dyn Backend,
    opts:        &CodegenOptions,
) -> Result<Vec<GeneratedFile>, String> {
    let path   = PathBuf::from(format!("{schema_name}.cddl"));
    let parsed = parse_cddl(schema_src, path)
        .map_err(|e| format!("parse error: {e}"))?;
    let lowered = lower(&parsed.value, opts.max_array, opts.max_str)
        .map_err(|e| format!("lower error: {e}"))?;
    backend.generate(&lowered.module, opts)
        .map(|out| out.files)
        .map_err(|e| format!("codegen error: {e}"))
}

/// Write generated files into `base_dir`, creating subdirectories as needed.
pub fn write_files(base_dir: &Path, files: &[GeneratedFile]) -> std::io::Result<()> {
    for f in files {
        let dest = base_dir.join(&f.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &f.content)?;
    }
    Ok(())
}

// ── Build runners ─────────────────────────────────────────────────────────────

fn run_cmd(cmd: &mut Command) -> Output {
    cmd.output().unwrap_or_else(|e| {
        panic!("failed to spawn command: {e}");
    })
}

fn format_output(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Build result from a process output.
fn build_result(schema: &str, out: Output, file_count: usize, ms: u64) -> BuildOutcome {
    if out.status.success() {
        BuildOutcome::Built {
            schema:   schema.into(),
            files:    file_count,
            build_ms: ms,
        }
    } else {
        let (stdout, stderr) = format_output(&out);
        BuildOutcome::Failed {
            schema:    schema.into(),
            stdout,
            stderr,
            exit_code: out.status.code(),
        }
    }
}

// ── Rust build ────────────────────────────────────────────────────────────────

/// Build a generated Rust crate.
///
/// Writes a workspace `Cargo.toml` wrapper so we don't need a registry
/// dependency on minicbor — it resolves from the parent workspace.
pub fn build_rust(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Rust) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    // The generated crate is in a subdirectory e.g. work_dir/primitives-cbor/
    // Find it by locating the Cargo.toml
    let crate_dir = files.iter()
        .find(|f| f.path.ends_with("Cargo.toml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0  = std::time::Instant::now();
    let out = run_cmd(
        Command::new("cargo")
            .arg("test")
            .current_dir(&crate_dir)
            .env("CARGO_NET_OFFLINE", "false")
    );
    let ms = t0.elapsed().as_millis() as u64;

    build_result(schema_name, out, files.len(), ms)
}

/// Build a `--no-std` crate as `#![no_std]` (`--no-default-features --features
/// no-std`), then run its tests with the default features.
pub fn build_rust_no_std(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Rust) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    let crate_dir = files.iter()
        .find(|f| f.path.ends_with("Cargo.toml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0 = std::time::Instant::now();
    let build = run_cmd(
        Command::new("cargo")
            .args(["build", "--no-default-features", "--features", "no-std"])
            .current_dir(&crate_dir)
    );
    if !build.status.success() {
        let (stdout, stderr) = format_output(&build);
        return BuildOutcome::Failed {
            schema:    schema_name.into(),
            stdout:    format!("[cargo build --features no-std]\n{stdout}"),
            stderr,
            exit_code: build.status.code(),
        };
    }
    let out = run_cmd(Command::new("cargo").arg("test").current_dir(&crate_dir));
    let ms = t0.elapsed().as_millis() as u64;

    build_result(schema_name, out, files.len(), ms)
}

// ── C build ───────────────────────────────────────────────────────────────────

pub fn build_c(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::C) {
        return BuildOutcome::Skipped { reason };
    }
    build_cmake(work_dir, files, schema_name)
}

/// Configure, build and test a generated CMake project (C or C++).
fn build_cmake(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    write_files(work_dir, files).expect("write files");

    // Find the CMakeLists.txt directory
    let cmake_dir = files.iter()
        .find(|f| f.path.ends_with("CMakeLists.txt"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let build_dir = cmake_dir.join("_build");
    std::fs::create_dir_all(&build_dir).expect("create build dir");

    let t0 = std::time::Instant::now();

    // Configure
    let cfg = run_cmd(
        Command::new("cmake")
            .arg("..")
            .arg("-DCMAKE_BUILD_TYPE=Debug")
            .current_dir(&build_dir)
    );
    if !cfg.status.success() {
        let (stdout, stderr) = format_output(&cfg);
        return BuildOutcome::Failed {
            schema: schema_name.into(),
            stdout: format!("[cmake configure]\n{stdout}"),
            stderr,
            exit_code: cfg.status.code(),
        };
    }

    // Build
    let build = run_cmd(
        Command::new("cmake")
            .args(["--build", ".", "--parallel"])
            .current_dir(&build_dir)
    );
    if !build.status.success() {
        let (stdout, stderr) = format_output(&build);
        return BuildOutcome::Failed {
            schema:    schema_name.into(),
            stdout:    format!("[cmake build]\n{stdout}"),
            stderr,
            exit_code: build.status.code(),
        };
    }

    // Run tests via CTest
    let test = run_cmd(
        Command::new("ctest")
            .arg("--output-on-failure")
            .current_dir(&build_dir)
    );
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, test, files.len(), ms)
}

// ── C++ build ─────────────────────────────────────────────────────────────────

pub fn build_cpp(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Cpp) {
        return BuildOutcome::Skipped { reason };
    }
    // C++ uses the same CMake flow as C
    build_cmake(work_dir, files, schema_name)
}

// ── C# build ──────────────────────────────────────────────────────────────────

pub fn build_csharp(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::CSharp) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    let csproj_dir = files.iter()
        .find(|f| f.path.ends_with(".csproj"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0  = std::time::Instant::now();
    let out = run_cmd(
        Command::new("dotnet")
            .arg("test")
            .arg("--nologo")
            .arg("-v").arg("quiet")
            .current_dir(&csproj_dir)
    );
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, out, files.len(), ms)
}

// ── Node.js / TypeScript build ────────────────────────────────────────────────

pub fn build_nodejs(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::NodeJs) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    let pkg_dir = files.iter()
        .find(|f| f.path.ends_with("package.json"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0 = std::time::Instant::now();

    // npm install
    let install = run_cmd(
        Command::new("npm")
            .args(["install", "--prefer-offline"])
            .current_dir(&pkg_dir)
    );
    if !install.status.success() {
        let (stdout, stderr) = format_output(&install);
        return BuildOutcome::Failed {
            schema:    schema_name.into(),
            stdout:    format!("[npm install]\n{stdout}"),
            stderr,
            exit_code: install.status.code(),
        };
    }

    // npm run build (tsc)
    let build = run_cmd(
        Command::new("npm")
            .args(["run", "build"])
            .current_dir(&pkg_dir)
    );
    if !build.status.success() {
        let (stdout, stderr) = format_output(&build);
        return BuildOutcome::Failed {
            schema:    schema_name.into(),
            stdout:    format!("[npm run build]\n{stdout}"),
            stderr,
            exit_code: build.status.code(),
        };
    }

    // npm test (Jest + ts-jest)
    let test = run_cmd(
        Command::new("npm")
            .arg("test")
            .current_dir(&pkg_dir)
    );
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, test, files.len(), ms)
}

// ── Python build (compile + pytest) ──────────────────────────────────────────

pub fn build_python(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    // `CDDLC_PYTHON` picks the interpreter (e.g. a venv's python); otherwise
    // python3, then python. It needs cbor2 >= 6.0 (`semantic_decoders`) and
    // pytest; without them nothing meaningful can run, so it's a skip.
    if let Err(reason) = toolchain::check(Lang::Python) {
        return BuildOutcome::Skipped { reason };
    }
    let Some(python) = toolchain::python() else {
        return BuildOutcome::Skipped { reason: "no Python interpreter".into() };
    };

    write_files(work_dir, files).expect("write files");

    let pkg_dir = files.iter()
        .find(|f| f.path.ends_with("pyproject.toml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());
    let src_dir = pkg_dir.join("src");

    let t0 = std::time::Instant::now();

    // Compile every generated file, then run the generated tests.
    let steps: [(&str, Vec<&str>); 2] = [
        ("compileall", vec!["-m", "compileall", "-q", "src", "tests"]),
        ("pytest", vec!["-m", "pytest", "tests/", "-q"]),
    ];
    for (label, args) in steps {
        let out = run_cmd(
            Command::new(&python)
                .args(&args)
                .current_dir(&pkg_dir)
                .env("PYTHONPATH", &src_dir)
        );
        if !out.status.success() {
            let (stdout, stderr) = format_output(&out);
            return BuildOutcome::Failed {
                schema:    schema_name.into(),
                stdout:    format!("[{label}]\n{stdout}"),
                stderr,
                exit_code: out.status.code(),
            };
        }
    }

    let ms = t0.elapsed().as_millis() as u64;
    BuildOutcome::Built { schema: schema_name.into(), files: files.len(), build_ms: ms }
}

// ── Go build (go mod tidy + vet + test) ───────────────────────────────────────

/// Build and test a generated Go module: `go mod tidy` (fetches fxamacker/cbor
/// and writes go.sum), `go vet`, then `go test`.
pub fn build_go(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Go) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    let mod_dir = files.iter()
        .find(|f| f.path.ends_with("go.mod"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0 = std::time::Instant::now();
    for (label, args) in [("go mod tidy", vec!["mod", "tidy"]), ("go vet", vec!["vet", "./..."])] {
        let out = run_cmd(Command::new("go").args(&args).current_dir(&mod_dir));
        if !out.status.success() {
            let (stdout, stderr) = format_output(&out);
            return BuildOutcome::Failed {
                schema:    schema_name.into(),
                stdout:    format!("[{label}]\n{stdout}"),
                stderr,
                exit_code: out.status.code(),
            };
        }
    }
    let out = run_cmd(Command::new("go").args(["test", "./..."]).current_dir(&mod_dir));
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, out, files.len(), ms)
}

// ── Java build (mvn test) ─────────────────────────────────────────────────────

/// Build and test a generated Maven project with `mvn test` (compiles the
/// sources and runs the JUnit tests). The first run downloads dependencies.
pub fn build_java(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Java) {
        return BuildOutcome::Skipped { reason };
    }

    write_files(work_dir, files).expect("write files");

    let project_dir = files.iter()
        .find(|f| f.path.ends_with("pom.xml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0 = std::time::Instant::now();
    let out = run_cmd(Command::new("mvn").args(["-B", "-ntp", "-q", "test"]).current_dir(&project_dir));
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, out, files.len(), ms)
}

// ── Lua build (busted) ────────────────────────────────────────────────────────


/// Check and test a generated Dart package: `dart pub get`, then `dart analyze`
/// (errors fail it, warnings don't), then `dart test` when the package has tests;
/// the backend writes them for structs and enums only, and `dart test` fails on none.
pub fn build_dart(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Dart) {
        return BuildOutcome::Skipped { reason };
    }
    let Some(dart) = toolchain::find("dart") else {
        return BuildOutcome::Skipped { reason: "dart not found".into() };
    };

    write_files(work_dir, files).expect("write files");

    let pkg_dir = files.iter()
        .find(|f| f.path.ends_with("pubspec.yaml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());
    let has_tests = files.iter()
        .any(|f| f.path.ends_with("_test.dart") && f.content.contains("test('"));

    let t0 = std::time::Instant::now();
    let get = run_cmd(Command::new(&dart).args(["pub", "get"]).current_dir(&pkg_dir));
    if !get.status.success() {
        return build_result(schema_name, get, files.len(), t0.elapsed().as_millis() as u64);
    }
    let analyze = run_cmd(Command::new(&dart).args(["analyze", "--no-fatal-warnings"]).current_dir(&pkg_dir));
    if !analyze.status.success() || !has_tests {
        return build_result(schema_name, analyze, files.len(), t0.elapsed().as_millis() as u64);
    }
    let out = run_cmd(Command::new(&dart).arg("test").current_dir(&pkg_dir));
    build_result(schema_name, out, files.len(), t0.elapsed().as_millis() as u64)
}

/// Test a generated Lua package with busted (which loads `src/` and runs `spec/`).
pub fn build_lua(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = toolchain::check(Lang::Lua) {
        return BuildOutcome::Skipped { reason };
    }
    let Some(busted) = toolchain::busted() else {
        return BuildOutcome::Skipped { reason: "busted not found".into() };
    };

    write_files(work_dir, files).expect("write files");

    let pkg_dir = files.iter()
        .find(|f| f.path.ends_with(".rockspec"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    let t0 = std::time::Instant::now();
    let out = run_cmd(Command::new(&busted).arg("-o").arg("TAP").current_dir(&pkg_dir));
    let ms = t0.elapsed().as_millis() as u64;
    build_result(schema_name, out, files.len(), ms)
}

// ── Multi-schema runner ───────────────────────────────────────────────────────

/// Run a build test for all reference schemas using one backend + build fn.
///
/// Returns a vec of outcomes, one per schema.
/// Prints progress to stderr as tests run.
pub fn run_build_tests<B, F>(
    backend:   &B,
    build_fn:  F,
    opts:      &CodegenOptions,
    work_root: &Path,
) -> Vec<BuildOutcome>
where
    B: Backend,
    F: Fn(&Path, &Vec<GeneratedFile>, &str) -> BuildOutcome,
{
    let mut outcomes = Vec::new();

    for (schema_name, schema_src) in SCHEMAS {
        eprint!("  [{schema_name}] generating ... ");

        let files = match generate_schema(schema_name, schema_src, backend, opts) {
            Ok(f) => {
                eprintln!("{} files", f.len());
                f
            }
            Err(e) => {
                eprintln!("CODEGEN ERROR");
                outcomes.push(BuildOutcome::CodegenError {
                    schema: schema_name.to_string(),
                    error:  e,
                });
                continue;
            }
        };

        let schema_work_dir = work_root.join(schema_name);
        std::fs::create_dir_all(&schema_work_dir).expect("create work dir");

        eprint!("  [{schema_name}] building  ... ");
        let outcome = build_with_retries(&schema_work_dir, &files, schema_name, &build_fn, &outcomes);

        match &outcome {
            BuildOutcome::Built { build_ms, .. } =>
                eprintln!("OK ({build_ms}ms)"),
            BuildOutcome::Skipped { reason } =>
                eprintln!("SKIP ({reason})"),
            BuildOutcome::Failed { exit_code, stderr, .. } =>
                eprintln!("FAIL (exit={exit_code:?})\n    stderr: {}", stderr.lines().next().unwrap_or("")),
            BuildOutcome::CodegenError { error, .. } =>
                eprintln!("CODEGEN ERROR: {error}"),
            BuildOutcome::Unavailable { reason, .. } =>
                eprintln!("UNAVAILABLE ({reason})"),
        }

        outcomes.push(outcome);
    }

    outcomes
}

/// Build one schema; when it fails because a package registry was unavailable
/// (see [`toolchain::registry_unavailable`]), wait and retry, up to
/// `CDDLC_NETWORK_RETRIES` times (default 2), then report it unavailable.
/// Once a schema in this run was unavailable, later ones get no retries, so a
/// registry that stays down doesn't add minutes per schema.
fn build_with_retries<F>(
    dir:      &Path,
    files:    &Vec<GeneratedFile>,
    schema:   &str,
    build_fn: &F,
    earlier:  &[BuildOutcome],
) -> BuildOutcome
where
    F: Fn(&Path, &Vec<GeneratedFile>, &str) -> BuildOutcome,
{
    let retries: u64 = if earlier.iter().any(BuildOutcome::is_unavailable) {
        0
    } else {
        std::env::var("CDDLC_NETWORK_RETRIES").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
    };
    retry_build(dir, files, schema, build_fn, retries, 20)
}

/// [`build_with_retries`], with the retry count and the wait unit given.
fn retry_build<F>(
    dir:      &Path,
    files:    &Vec<GeneratedFile>,
    schema:   &str,
    build_fn: &F,
    retries:  u64,
    wait_s:   u64,
) -> BuildOutcome
where
    F: Fn(&Path, &Vec<GeneratedFile>, &str) -> BuildOutcome,
{
    let mut attempt = 0;
    loop {
        let outcome = build_fn(dir, files, schema);
        let BuildOutcome::Failed { stdout, stderr, .. } = &outcome else { return outcome };
        let Some(reason) = toolchain::registry_unavailable(&format!("{stdout}\n{stderr}")) else {
            return outcome;
        };
        if attempt >= retries {
            return BuildOutcome::Unavailable { schema: schema.into(), reason };
        }
        attempt += 1;
        let wait = wait_s * attempt;
        eprint!("registry unavailable ({reason}); retrying in {wait}s ... ");
        std::thread::sleep(std::time::Duration::from_secs(wait));
    }
}

/// Assert all outcomes are OK (built or skipped), printing failures.
///
/// With `CDDLC_REQUIRE_TOOLS` set (as CI sets it), a skipped schema fails
/// too, so a missing toolchain can't pass for a tested one.
pub fn assert_all_ok(outcomes: &[BuildOutcome]) {
    // A registry outage isn't the code's fault: warn (as a GitHub Actions
    // annotation in CI) rather than fail, so it can't block a pull request.
    let unavailable: Vec<(&String, &String)> = outcomes.iter()
        .filter_map(|o| match o {
            BuildOutcome::Unavailable { schema, reason } => Some((schema, reason)),
            _ => None,
        })
        .collect();
    if let Some((_, reason)) = unavailable.first() {
        let names: Vec<&str> = unavailable.iter().map(|(s, _)| s.as_str()).collect();
        let msg = format!(
            "{} schema(s) not built because a package registry was unavailable ({}): {reason}",
            unavailable.len(), names.join(", ")
        );
        eprintln!("\nWARNING: {msg}\n");
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            println!("::warning title=Package registry unavailable::{}", msg.replace('\n', " "));
        }
    }
    if std::env::var_os("CDDLC_REQUIRE_TOOLS").is_some_and(|v| !v.is_empty()) {
        let skipped: Vec<String> = outcomes.iter()
            .filter_map(|o| match o {
                BuildOutcome::Skipped { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect();
        if !skipped.is_empty() {
            panic!(
                "\n{} schema(s) skipped with CDDLC_REQUIRE_TOOLS set: {}\n",
                skipped.len(),
                skipped[0]
            );
        }
    }

    let failures: Vec<&BuildOutcome> = outcomes.iter()
        .filter(|o| !o.is_ok())
        .collect();

    if !failures.is_empty() {
        let mut msg = format!("\n{} build(s) failed:\n", failures.len());
        for f in &failures {
            match f {
                BuildOutcome::Failed { schema, stdout, stderr, exit_code } => {
                    msg.push_str(&format!(
                        "\n  Schema: {schema} (exit={exit_code:?})\n  stdout:\n{stdout}\n  stderr:\n{stderr}\n"
                    ));
                }
                BuildOutcome::CodegenError { schema, error } => {
                    msg.push_str(&format!("\n  Schema: {schema} — codegen error: {error}\n"));
                }
                _ => {}
            }
        }
        panic!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn failed(stderr: &str) -> BuildOutcome {
        BuildOutcome::Failed { schema: "s".into(), stdout: String::new(), stderr: stderr.into(), exit_code: Some(1) }
    }

    const RATE_LIMITED: &str = "[ERROR] Could not transfer artifact a:b:pom:1 from/to central: status code: 429, reason phrase: Too Many Requests (429)";

    #[test]
    fn a_registry_failure_is_retried_then_unavailable() {
        let calls = Cell::new(0);
        let build = |_: &Path, _: &Vec<GeneratedFile>, _: &str| { calls.set(calls.get() + 1); failed(RATE_LIMITED) };
        let out = retry_build(Path::new("."), &vec![], "s", &build, 2, 0);
        assert!(matches!(out, BuildOutcome::Unavailable { .. }), "{out:?}");
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn a_registry_failure_that_clears_builds() {
        let calls = Cell::new(0);
        let build = |_: &Path, _: &Vec<GeneratedFile>, _: &str| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 { failed(RATE_LIMITED) } else { BuildOutcome::Built { schema: "s".into(), files: 1, build_ms: 0 } }
        };
        assert!(matches!(retry_build(Path::new("."), &vec![], "s", &build, 2, 0), BuildOutcome::Built { .. }));
    }

    #[test]
    fn a_code_failure_is_not_retried() {
        let calls = Cell::new(0);
        let build = |_: &Path, _: &Vec<GeneratedFile>, _: &str| { calls.set(calls.get() + 1); failed("error[E0308]: mismatched types") };
        assert!(matches!(retry_build(Path::new("."), &vec![], "s", &build, 2, 0), BuildOutcome::Failed { .. }));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn unavailable_schemas_do_not_fail_the_test() {
        assert_all_ok(&[
            BuildOutcome::Built { schema: "a".into(), files: 1, build_ms: 0 },
            BuildOutcome::Unavailable { schema: "b".into(), reason: "429".into() },
        ]);
    }

    #[test]
    #[should_panic(expected = "build(s) failed")]
    fn a_failed_schema_fails_the_test() {
        assert_all_ok(&[failed("error[E0308]: mismatched types")]);
    }
}
