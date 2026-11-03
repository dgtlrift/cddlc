mod args;
mod loader;

use std::path::PathBuf;
use std::process;

use clap::Parser;

use args::{Cli, Command, GenerateArgs, ValidateArgs};
mod build_info;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::{lower, IrModule};
use loader::load;

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Command::Generate(args) => run_generate(args),
        Command::Validate(args) => run_validate(args),
    };

    match result {
        Ok(true) => {}
        Ok(false) => process::exit(1), // validation ran but found failures
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}

fn parse_interop_langs(s: &str) -> backend_interop::InteropLangs {
    let mut langs = backend_interop::InteropLangs::default();
    for part in s.split(',') {
        match part.trim() {
            "rust"   => langs.rust   = true,
            "c"      => langs.c      = true,
            "cpp"    => langs.cpp    = true,
            "csharp" => langs.csharp = true,
            "nodejs" => langs.nodejs = true,
            "python" => langs.python = true,
            _        => {}
        }
    }
    langs
}

/// Load one or more `.cddl` entry points (each with its own transitive
/// `@import`s), merge them into a single module, and lower to IR. Shared by
/// both `generate` and `validate`.
fn load_and_lower(
    inputs:               &[PathBuf],
    include_dirs:         &[PathBuf],
    verbose:              bool,
    debug_parse:          bool,
    default_capacity:     usize,
    default_str_capacity: usize,
) -> Result<cddlc_ir::LowerResult, Box<dyn std::error::Error>> {
    if verbose {
        eprintln!("cddlc: loading {} input file(s)...", inputs.len());
    }

    let mut merged_rules   = Vec::new();
    let mut merged_pragmas = Vec::new();
    let mut all_warnings   = Vec::new();
    let mut primary_path   = PathBuf::from("module.cddl");

    for (i, input) in inputs.iter().enumerate() {
        let loaded = load(input, include_dirs, verbose, debug_parse)
            .map_err(|e| e.to_string())?;

        all_warnings.extend(loaded.warnings);

        if i == 0 {
            primary_path   = loaded.module.source_path.clone();
            merged_pragmas = loaded.module.file_pragmas;
        } else {
            merged_pragmas.extend(
                loaded.module.file_pragmas.into_iter()
                    .filter(|p| p.keyword == "import")
            );
        }

        merged_rules.extend(loaded.module.rules);
    }

    for w in &all_warnings {
        eprintln!("warning: {w}");
    }

    let merged_module = cddlc_parser::CddlModule {
        source_path:  primary_path,
        file_pragmas: merged_pragmas,
        rules:        merged_rules,
    };

    if verbose {
        eprintln!("cddlc: running semantic analysis...");
    }

    let lower_result = lower(&merged_module, default_capacity, default_str_capacity)
        .map_err(|e| e.to_string())?;

    for w in &lower_result.warnings {
        eprintln!("warning: {w}");
    }

    if verbose {
        eprintln!("cddlc: {} type(s) resolved:", lower_result.module.types.len());
        for (name, def) in &lower_result.module.types {
            eprintln!("  {} {:?}", name, std::mem::discriminant(def));
        }
    }

    Ok(lower_result)
}

// ── generate ─────────────────────────────────────────────────────────────────

fn run_generate(args: &GenerateArgs) -> Result<bool, Box<dyn std::error::Error>> {
    if args.no_std && args.lang != args::Lang::Rust {
        return Err("--no-std is only valid with --lang rust".into());
    }

    let lower_result = load_and_lower(
        &args.inputs, &args.include_dir, args.verbose, args.debug_parse,
        args.max_array, args.max_str,
    )?;
    let ir = &lower_result.module;

    if args.dry_run {
        eprintln!("cddlc: dry-run complete ({} types)", ir.types.len());
        return Ok(true);
    }

    let opts = CodegenOptions {
        lang:        args.lang.into(),
        format:      match args.format {
            args::Fmt::Cbor => cddlc_codegen::Format::Cbor,
            args::Fmt::Json => cddlc_codegen::Format::Json,
        },
        runtime:     args.runtime.clone(),
        alloc:       args.alloc.into(),
        dcbor:       args.dcbor,
        no_std:      args.no_std,
        depth_limit: args.depth_limit,
        namespace:   args.namespace.clone(),
        max_array:   args.max_array,
        max_str:     args.max_str,
        provenance:  build_info::provenance(&args.inputs),
    };

    // Each type's and field's CDDL source, in its documentation (dgtlrift/cddlc#9),
    // the paths relative to where cddlc runs.
    let annotated;
    let ir = if args.no_source_comments {
        ir
    } else {
        let base = std::env::current_dir().ok().and_then(|d| d.canonicalize().ok());
        annotated = cddlc_codegen::with_source_docs(ir, base.as_deref());
        &annotated
    };

    let output = match args.lang {
        args::Lang::Rust => {
            let backend = backend_rust::RustBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::C => {
            let backend = backend_c::CBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::Cpp => {
            let backend = backend_cpp::CppBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::Csharp => {
            let backend = backend_csharp::CSharpBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::Nodejs => {
            let backend = backend_nodejs::NodeJsBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::Python => {
            let backend = backend_python::PythonBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
        args::Lang::Dart => {
            let backend = backend_dart::DartBackend;
            backend.generate(ir, &opts).map_err(|e| e.to_string())?
        }
    };

    std::fs::create_dir_all(&args.output)?;

    for file in &output.files {
        let dest = args.output.join(&file.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.content)?;
        eprintln!("cddlc: wrote {}", dest.display());
    }

    if args.interop {
        let langs = parse_interop_langs(&args.interop_langs);
        let interop_files = backend_interop::generate(ir, &langs, &opts);
        for file in &interop_files {
            let dest = args.output.join(&file.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &file.content)?;
            eprintln!("cddlc: wrote {}", dest.display());
        }
    }

    Ok(true)
}

// ── validate ─────────────────────────────────────────────────────────────────

/// Returns `Ok(true)` if every input document validated successfully,
/// `Ok(false)` if at least one failed (or errored while decoding) — the
/// caller maps this to a process exit code.
fn run_validate(args: &ValidateArgs) -> Result<bool, Box<dyn std::error::Error>> {
    let lower_result = load_and_lower(
        std::slice::from_ref(&args.cddl), &args.include_dir, args.verbose, args.debug_parse,
        16, 64,
    )?;
    let ir: &IrModule = &lower_result.module;

    let type_name = args.type_name.as_deref().unwrap_or(ir.root.as_str());
    if type_name.is_empty() {
        return Err("schema defines no rules to validate against".into());
    }
    if args.json.is_empty() && args.cbor.is_empty() {
        return Err("no data files given — pass --json FILE and/or --cbor FILE".into());
    }

    let mut all_ok = true;

    for path in &args.json {
        let ok = validate_one(path, type_name, ir, |bytes| {
            let json: serde_json::Value = serde_json::from_slice(bytes)?;
            Ok(cddlc_validate::Value::from(json))
        });
        all_ok &= ok;
    }

    for path in &args.cbor {
        let ok = validate_one(path, type_name, ir, |bytes| {
            let cbor: ciborium::Value = ciborium::from_reader(bytes)?;
            Ok(cddlc_validate::Value::from(cbor))
        });
        all_ok &= ok;
    }

    Ok(all_ok)
}

fn validate_one(
    path:      &PathBuf,
    type_name: &str,
    ir:        &IrModule,
    decode:    impl FnOnce(&[u8]) -> Result<cddlc_validate::Value, Box<dyn std::error::Error>>,
) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: I/O error: {e}", path.display());
            return false;
        }
    };

    let value = match decode(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: decode error: {e}", path.display());
            return false;
        }
    };

    match cddlc_validate::validate(ir, Some(type_name), &value) {
        Ok(()) => {
            println!("{}: OK ({type_name})", path.display());
            true
        }
        Err(errors) => {
            println!("{}: FAIL ({type_name}, {} error(s))", path.display(), errors.len());
            for e in &errors {
                println!("  {e}");
            }
            false
        }
    }
}
