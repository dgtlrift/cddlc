/// Build validation harness for cddlc backends.
///
/// Generates code from reference CDDL schemas using each backend,
/// writes it to a temporary directory, then invokes the language's
/// native toolchain to verify the generated code actually compiles.
///
/// Tests skip gracefully when a required toolchain is not installed.

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
}

impl BuildOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, BuildOutcome::Skipped { .. } | BuildOutcome::Built { .. })
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
    if let Err(reason) = require_tools(&["cargo"]) {
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

// ── C build ───────────────────────────────────────────────────────────────────

pub fn build_c(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = require_tools(&["cmake"]) {
        return BuildOutcome::Skipped { reason };
    }

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
    // C++ uses the same CMake flow as C
    build_c(work_dir, files, schema_name)
}

// ── C# build ──────────────────────────────────────────────────────────────────

pub fn build_csharp(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    if let Err(reason) = require_tools(&["dotnet"]) {
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
    if let Err(reason) = require_tools(&["npm"]) {
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

// ── Python build (syntax check) ───────────────────────────────────────────────

pub fn build_python(
    work_dir:    &Path,
    files:       &[GeneratedFile],
    schema_name: &str,
) -> BuildOutcome {
    // Try python3 then python
    let python = match find_tool("python3").or_else(|_| find_tool("python")) {
        Ok(p) => p,
        Err(_) => return BuildOutcome::Skipped { reason: "python3/python not found in PATH".into() },
    };

    write_files(work_dir, files).expect("write files");

    // Find all .py source files
    let py_files: Vec<PathBuf> = files.iter()
        .filter(|f| f.path.ends_with(".py") && f.path.contains("/src/"))
        .map(|f| work_dir.join(&f.path))
        .collect();

    if py_files.is_empty() {
        return BuildOutcome::Skipped { reason: "no .py source files generated".into() };
    }

    let t0 = std::time::Instant::now();

    // Step 1: syntax check with py_compile
    for py in &py_files {
        let out = run_cmd(
            Command::new(&python)
                .args(["-m", "py_compile"])
                .arg(py)
        );
        if !out.status.success() {
            let (stdout, stderr) = format_output(&out);
            return BuildOutcome::Failed {
                schema:    schema_name.into(),
                stdout:    format!("[py_compile {}]\n{stdout}", py.display()),
                stderr,
                exit_code: out.status.code(),
            };
        }
    }

    // Step 2: try pip install -e . if cbor2 available, otherwise just type-check
    let pkg_dir = files.iter()
        .find(|f| f.path.ends_with("pyproject.toml"))
        .map(|f| work_dir.join(Path::new(&f.path).parent().unwrap_or(Path::new("."))))
        .unwrap_or_else(|| work_dir.to_path_buf());

    // Check if cbor2 is already installed
    let cbor2_check = Command::new(&python)
        .args(["-c", "import cbor2"])
        .output();

    if cbor2_check.map(|o| o.status.success()).unwrap_or(false) {
        // cbor2 available — do a full import check
        let src_dir = pkg_dir.join("src");
        for py in &py_files {
            let rel = py.strip_prefix(&src_dir).unwrap_or(py);
            let module = rel.to_string_lossy()
                .trim_end_matches(".py")
                .replace(['/', '\\'], ".")
                .to_string();
            let out = run_cmd(
                Command::new(&python)
                    .args(["-c", &format!("import sys; sys.path.insert(0, 'src'); import {module}")])
                    .current_dir(&pkg_dir)
            );
            if !out.status.success() {
                let (stdout, stderr) = format_output(&out);
                let _ms = t0.elapsed().as_millis() as u64;
                return BuildOutcome::Failed {
                    schema:    schema_name.into(),
                    stdout:    format!("[import {module}]\n{stdout}"),
                    stderr,
                    exit_code: out.status.code(),
                };
            }
        }
    }

    // Step 3: run pytest if available
    let pytest_available = Command::new(&python)
        .args(["-m", "pytest", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if pytest_available {
        let src_dir = pkg_dir.join("src");
        let test = run_cmd(
            Command::new(&python)
                .args(["-m", "pytest", "tests/", "-q"])
                .current_dir(&pkg_dir)
                .env("PYTHONPATH", &src_dir)
        );
        if !test.status.success() {
            let (stdout, stderr) = format_output(&test);
            return BuildOutcome::Failed {
                schema:    schema_name.into(),
                stdout:    format!("[pytest]\n{stdout}"),
                stderr,
                exit_code: test.status.code(),
            };
        }
    }

    let ms = t0.elapsed().as_millis() as u64;
    BuildOutcome::Built { schema: schema_name.into(), files: files.len(), build_ms: ms }
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
        let outcome = build_fn(&schema_work_dir, &files, schema_name);

        match &outcome {
            BuildOutcome::Built { build_ms, .. } =>
                eprintln!("OK ({build_ms}ms)"),
            BuildOutcome::Skipped { reason } =>
                eprintln!("SKIP ({reason})"),
            BuildOutcome::Failed { exit_code, stderr, .. } =>
                eprintln!("FAIL (exit={exit_code:?})\n    stderr: {}", stderr.lines().next().unwrap_or("")),
            BuildOutcome::CodegenError { error, .. } =>
                eprintln!("CODEGEN ERROR: {error}"),
        }

        outcomes.push(outcome);
    }

    outcomes
}

/// Assert all outcomes are OK (built or skipped), printing failures.
pub fn assert_all_ok(outcomes: &[BuildOutcome]) {
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
