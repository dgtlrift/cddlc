/// Import resolution for cddlc-cli.
///
/// Follows `@import "path"` pragmas recursively, detecting cycles, and
/// merges all rules into a single `CddlModule` for IR lowering.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cddlc_parser::{parse_cddl, parse_cddl_debug, format_parse_diagnostic, CddlModule, ParseWarning, PragmaValue, Rule, Spanned};

/// Error from import resolution.
#[derive(Debug)]
pub enum LoadError {
    Io { path: PathBuf, source: std::io::Error },
    Parse(cddlc_parser::ParseError),
    Cycle { cycle: Vec<String> },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, source } =>
                write!(f, "I/O error reading '{}': {}", path.display(), source),
            LoadError::Parse(e) => write!(f, "{e}"),
            LoadError::Cycle { cycle } =>
                write!(f, "import cycle: {}", cycle.join(" → ")),
        }
    }
}

impl From<cddlc_parser::ParseError> for LoadError {
    fn from(e: cddlc_parser::ParseError) -> Self { LoadError::Parse(e) }
}

/// Result of loading an entry-point file and all its transitive imports.
pub struct LoadedModule {
    /// Merged module — all rules from all files, entry-point pragmas preserved.
    pub module:   CddlModule,
    /// All warnings collected during parsing.
    pub warnings: Vec<ParseWarning>,
}

/// Load `entry_path` and all transitive `@import` dependencies.
///
/// `include_dirs` are additional search paths tried when an import path
/// can't be resolved relative to the importing file.
pub fn load(
    entry_path:   &Path,
    include_dirs: &[PathBuf],
    verbose:      bool,
    debug_parse:  bool,
) -> Result<LoadedModule, LoadError> {
    let mut loader = Loader {
        include_dirs,
        seen:        HashSet::new(),
        stack:       Vec::new(),
        warnings:    Vec::new(),
        verbose,
        debug_parse,
    };

    let canonical = entry_path.canonicalize()
        .map_err(|e| LoadError::Io { path: entry_path.to_owned(), source: e })?;

    loader.load_file(&canonical)
}

// ── Internal loader ───────────────────────────────────────────────────────────

struct Loader<'a> {
    include_dirs: &'a [PathBuf],
    seen:        HashSet<PathBuf>,
    stack:       Vec<PathBuf>,
    warnings:    Vec<ParseWarning>,
    verbose:     bool,
    debug_parse: bool,
}

impl<'a> Loader<'a> {
    fn load_file(&mut self, canonical: &Path) -> Result<LoadedModule, LoadError> {
        // Cycle detection
        if self.stack.contains(&canonical.to_path_buf()) {
            let mut cycle: Vec<String> = self.stack
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(canonical.display().to_string());
            return Err(LoadError::Cycle { cycle });
        }

        // Already loaded — return empty module (rules de-duped at merge)
        if self.seen.contains(canonical) {
            return Ok(LoadedModule {
                module:   empty_module(canonical),
                warnings: vec![],
            });
        }

        if self.verbose {
            eprintln!("  loading: {}", canonical.display());
        }

        // Read and parse
        let src = std::fs::read_to_string(canonical)
            .map_err(|e| LoadError::Io { path: canonical.to_owned(), source: e })?;

        let result = if self.debug_parse {
            parse_cddl_debug(&src, canonical.to_owned())
        } else {
            parse_cddl(&src, canonical.to_owned())
        }.map_err(|e| {
            // On debug_parse the diagnostic was already printed; on normal
            // mode print a compact version here
            if !self.debug_parse {
                eprintln!("{}", format_parse_diagnostic(&src, &canonical.to_owned(), &e));
            }
            LoadError::Parse(e)
        })?;
        self.warnings.extend(result.warnings);
        let module = result.value;

        self.stack.push(canonical.to_path_buf());

        // Resolve and load imports
        let mut all_rules: Vec<Spanned<Rule>> = Vec::new();
        let file_pragmas = module.file_pragmas.clone();

        for pragma in &module.file_pragmas {
            if let PragmaValue::Import(import_path) = &pragma.value {
                let resolved = self.resolve_import(import_path, canonical)?;
                let imported = self.load_file(&resolved)?;
                // Prepend imported rules (so dependencies appear first)
                all_rules.extend(imported.module.rules);
            }
        }

        // Then this file's own rules
        all_rules.extend(module.rules);

        self.stack.pop();
        self.seen.insert(canonical.to_path_buf());

        Ok(LoadedModule {
            module:   CddlModule {
                source_path: canonical.to_owned(),
                file_pragmas,
                rules: all_rules,
            },
            warnings: std::mem::take(&mut self.warnings),
        })
    }

    fn resolve_import(
        &self,
        import_path: &str,
        from_file:   &Path,
    ) -> Result<PathBuf, LoadError> {
        // Try relative to the importing file's directory first
        if let Some(parent) = from_file.parent() {
            let candidate = parent.join(import_path);
            if candidate.exists() {
                return candidate.canonicalize()
                    .map_err(|e| LoadError::Io { path: candidate, source: e });
            }
        }

        // Try each --include-dir in order
        for dir in self.include_dirs {
            let candidate = dir.join(import_path);
            if candidate.exists() {
                return candidate.canonicalize()
                    .map_err(|e| LoadError::Io { path: candidate, source: e });
            }
        }

        Err(LoadError::Io {
            path: PathBuf::from(import_path),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("cannot find imported file '{import_path}'"),
            ),
        })
    }
}

fn empty_module(path: &Path) -> CddlModule {
    CddlModule {
        source_path: path.to_owned(),
        file_pragmas: vec![],
        rules:        vec![],
    }
}
