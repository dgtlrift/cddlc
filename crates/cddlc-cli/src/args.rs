use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// cddlc — CDDL compiler and code generator
///
/// Parses RFC 8610 CDDL schema files and either generates serializers/
/// deserializers for a target language, or validates JSON/CBOR data against
/// the schema.
#[derive(Parser, Debug)]
#[command(name = "cddlc", version = crate::build_info::VERSION, long_version = crate::build_info::LONG_VERSION, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate serialization/deserialization code for a target language
    Generate(GenerateArgs),
    /// Validate JSON/CBOR documents against a CDDL schema
    Validate(ValidateArgs),
}

#[derive(Args, Debug)]
pub struct GenerateArgs {
    /// One or more .cddl source files (entry points)
    #[arg(required = true, value_name = "INPUT")]
    pub inputs: Vec<PathBuf>,

    /// Target language
    #[arg(short, long, value_enum, default_value = "rust")]
    pub lang: Lang,

    /// Output directory for generated files
    #[arg(short, long, default_value = "./generated")]
    pub output: PathBuf,

    /// Serialization format
    #[arg(long, value_enum, default_value = "cbor")]
    pub format: Fmt,

    /// CBOR runtime library
    ///   rust: minicbor | ciborium | cbor4ii
    ///   c:    tinycbor | nanocbor | zcbor
    #[arg(short, long, default_value = "minicbor")]
    pub runtime: String,

    /// Emit deterministic encoding (dCBOR, RFC 8949 §4.2)
    #[arg(long)]
    pub dcbor: bool,

    /// Allocation strategy for generated code
    #[arg(long, value_enum, default_value = "arena")]
    pub alloc: Alloc,

    /// Rust: emit #![no_std] + heapless collections (implies arena/stack)
    #[arg(long)]
    pub no_std: bool,

    /// Maximum nesting depth for decoder
    #[arg(long, default_value = "16")]
    pub depth_limit: usize,

    /// Default capacity for unbounded sequences (e.g. `[* T]`)
    #[arg(long, default_value = "16", value_name = "N")]
    pub max_array: usize,

    /// Default capacity for unbounded strings
    #[arg(long, default_value = "64", value_name = "N")]
    pub max_str: usize,

    /// Namespace / module prefix for emitted symbols
    #[arg(long, value_name = "NS")]
    pub namespace: Option<String>,

    /// Additional search paths for imported .cddl files (repeatable)
    #[arg(long, value_name = "DIR")]
    pub include_dir: Vec<PathBuf>,

    /// Print rich parse diagnostics on failure (also enabled by CDDLC_TRACE=1)
    #[arg(long)]
    pub debug_parse: bool,

    /// Generate cross-language interoperability test vectors and harnesses
    #[arg(long)]
    pub interop: bool,

    /// Comma-separated list of languages for interop harnesses (default: all)
    /// e.g. --interop-langs rust,python,nodejs
    #[arg(long, default_value = "rust,c,cpp,csharp,nodejs,python")]
    pub interop_langs: String,

    /// Parse and analyze only — do not write output files
    #[arg(long)]
    pub dry_run: bool,

    /// Leave out each type's and field's CDDL source line (`cddl: <file>:<line>: <text>`)
    /// from its documentation comment
    #[arg(long)]
    pub no_source_comments: bool,

    /// Verbose output (print resolved types, warnings, file paths)
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// CDDL schema entry point
    #[arg(long, value_name = "FILE")]
    pub cddl: PathBuf,

    /// Type name to validate against (defaults to the schema's root rule —
    /// the first rule declared in the file)
    #[arg(long = "type", value_name = "NAME")]
    pub type_name: Option<String>,

    /// JSON document(s) to validate (repeatable)
    #[arg(long = "json", value_name = "FILE")]
    pub json: Vec<PathBuf>,

    /// CBOR document(s) to validate (repeatable)
    #[arg(long = "cbor", value_name = "FILE")]
    pub cbor: Vec<PathBuf>,

    /// Additional search paths for imported .cddl files (repeatable)
    #[arg(long, value_name = "DIR")]
    pub include_dir: Vec<PathBuf>,

    /// Print rich parse diagnostics on failure (also enabled by CDDLC_TRACE=1)
    #[arg(long)]
    pub debug_parse: bool,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    C,
    Cpp,
    Csharp,
    Nodejs,
    Python,
    Dart,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    Cbor,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alloc {
    Stack,
    Arena,
    Heap,
}

impl From<Lang> for cddlc_codegen::Language {
    fn from(l: Lang) -> Self {
        match l {
            Lang::Rust   => cddlc_codegen::Language::Rust,
            Lang::C      => cddlc_codegen::Language::C,
            Lang::Cpp    => cddlc_codegen::Language::Cpp,
            Lang::Csharp => cddlc_codegen::Language::CSharp,
            Lang::Nodejs => cddlc_codegen::Language::Python, // reuses Python slot
            Lang::Python => cddlc_codegen::Language::Python,
            Lang::Dart   => cddlc_codegen::Language::Dart,
        }
    }
}

impl From<Alloc> for cddlc_codegen::AllocStrategy {
    fn from(a: Alloc) -> Self {
        match a {
            Alloc::Stack => cddlc_codegen::AllocStrategy::Stack,
            Alloc::Arena => cddlc_codegen::AllocStrategy::Arena,
            Alloc::Heap  => cddlc_codegen::AllocStrategy::Heap,
        }
    }
}
