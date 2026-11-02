/// Shared code-generation utilities used by all backends.

// ── Indentation ───────────────────────────────────────────────────────────────

/// A simple indented string builder.
pub struct IndentWriter {
    buf:    String,
    indent: usize,
    width:  usize,
}

impl IndentWriter {
    pub fn new(indent_width: usize) -> Self {
        Self { buf: String::new(), indent: 0, width: indent_width }
    }

    pub fn indent(&mut self)   { self.indent += 1; }
    pub fn dedent(&mut self)   { self.indent = self.indent.saturating_sub(1); }

    pub fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.buf.push('\n');
        } else {
            for _ in 0..self.indent * self.width {
                self.buf.push(' ');
            }
            self.buf.push_str(s);
            self.buf.push('\n');
        }
    }

    pub fn blank(&mut self) { self.buf.push('\n'); }

    pub fn finish(self) -> String { self.buf }
}

// ── Name mangling ─────────────────────────────────────────────────────────────

/// Convert a CDDL identifier to snake_case (Rust fields, C variables).
pub fn to_snake_case(s: &str) -> String {
    s.replace('-', "_")
}

/// Convert a CDDL identifier to PascalCase (Rust types, C# types).
pub fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None    => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Convert a CDDL identifier to SCREAMING_SNAKE_CASE (C enum constants).
pub fn to_screaming_snake(s: &str) -> String {
    s.replace('-', "_").to_uppercase()
}

// ── Primitive type mapping ────────────────────────────────────────────────────

use cddlc_ir::Primitive;

/// Map a primitive to its Rust type name.
pub fn primitive_to_rust(p: &Primitive, no_std: bool) -> &'static str {
    match p {
        Primitive::Bool    => "bool",
        Primitive::Null    => "()",
        Primitive::Undefined => "()",
        Primitive::Uint    => "u64",
        Primitive::Int     => "i64",
        Primitive::Float16 => "f32",   // f16 not stable; use f32
        Primitive::Float32 => "f32",
        Primitive::Float64 => "f64",
        Primitive::Float   => "f64",
        Primitive::Tstr    => if no_std { "&'b str" } else { "String" },
        Primitive::Bstr    => if no_std { "&'b [u8]" } else { "Vec<u8>" },
        Primitive::Any     => if no_std { "&'b [u8]" } else { "Vec<u8>" },
    }
}

/// Map a primitive to its C type name.
pub fn primitive_to_c(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool    => "bool",
        Primitive::Null    => "void*",
        Primitive::Undefined => "void*",
        Primitive::Uint    => "uint64_t",
        Primitive::Int     => "int64_t",
        Primitive::Float16 => "float",
        Primitive::Float32 => "float",
        Primitive::Float64 => "double",
        Primitive::Float   => "double",
        Primitive::Bstr    => "cbor_slice_t",
        Primitive::Tstr    => "cbor_str_t",
        Primitive::Any     => "cbor_any_t",
    }
}

// ── Constraint helpers ────────────────────────────────────────────────────────

use cddlc_ir::Constraint;

/// Emit a Rust boolean expression that checks `val` against a constraint.
/// Returns `None` for constraints that don't produce inline checks.
pub fn constraint_to_rust_check(c: &Constraint, val_expr: &str) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => {
            Some(format!("{val_expr}.len() == {n}"))
        }
        Constraint::SizeRange { min, max } => {
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{val_expr}.len() >= {lo} && {val_expr}.len() <= {hi}")),
                (Some(lo), None)     => Some(format!("{val_expr}.len() >= {lo}")),
                (None, Some(hi))     => Some(format!("{val_expr}.len() <= {hi}")),
                (None, None)         => None,
            }
        }
        Constraint::ValueRangeInt { min, max, inclusive } => {
            let ge = if *inclusive { ">=" } else { ">" };
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{val_expr} {ge} {lo} && {val_expr} {le} {hi}")),
                (Some(lo), None)     => Some(format!("{val_expr} {ge} {lo}")),
                (None, Some(hi))     => Some(format!("{val_expr} {le} {hi}")),
                (None, None)         => None,
            }
        }
        Constraint::ValueRangeUint { min, max, inclusive } => {
            let ge = if *inclusive { ">=" } else { ">" };
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{val_expr} {ge} {lo} && {val_expr} {le} {hi}")),
                (Some(lo), None)     => Some(format!("{val_expr} {ge} {lo}")),
                (None, Some(hi))     => Some(format!("{val_expr} {le} {hi}")),
                (None, None)         => None,
            }
        }
        Constraint::ValueRangeF64 { min, max, inclusive } => {
            let ge = if *inclusive { ">=" } else { ">" };
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{val_expr} {ge} {lo:.?} && {val_expr} {le} {hi:.?}")),
                (Some(lo), None)     => Some(format!("{val_expr} {ge} {lo:.?}")),
                (None, Some(hi))     => Some(format!("{val_expr} {le} {hi:.?}")),
                (None, None)         => None,
            }
        }
        Constraint::Eq(v) => Some(format!("{val_expr} == {}", literal_to_rust(v))),
        Constraint::Ne(v) => Some(format!("{val_expr} != {}", literal_to_rust(v))),
        Constraint::Regexp { hook: Some(h), .. } => {
            Some(format!("{h}({val_expr})"))
        }
        Constraint::Default(_)
        | Constraint::Regexp { hook: None, .. }
        | Constraint::CborEmbedded(_)
        | Constraint::CborSeq(_) => None,
    }
}

/// Emit a C boolean expression checking `val_expr` against a constraint.
pub fn constraint_to_c_check(c: &Constraint, val_expr: &str) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => Some(format!("{val_expr}.len == {n}")),
        Constraint::SizeRange { min, max } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!("{val_expr}.len >= {lo} && {val_expr}.len <= {hi}")),
            (Some(lo), None)     => Some(format!("{val_expr}.len >= {lo}")),
            (None, Some(hi))     => Some(format!("{val_expr}.len <= {hi}")),
            (None, None)         => None,
        },
        Constraint::ValueRangeInt { min, max, .. } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!("{val_expr} >= {lo} && {val_expr} <= {hi}")),
            (Some(lo), None)     => Some(format!("{val_expr} >= {lo}")),
            (None, Some(hi))     => Some(format!("{val_expr} <= {hi}")),
            (None, None)         => None,
        },
        Constraint::ValueRangeUint { min, max, .. } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!("{val_expr} >= {lo}U && {val_expr} <= {hi}U")),
            (Some(lo), None)     => Some(format!("{val_expr} >= {lo}U")),
            (None, Some(hi))     => Some(format!("{val_expr} <= {hi}U")),
            (None, None)         => None,
        },
        Constraint::Eq(v) => Some(format!("{val_expr} == {}", literal_to_c(v))),
        Constraint::Ne(v) => Some(format!("{val_expr} != {}", literal_to_c(v))),
        Constraint::Regexp { hook: Some(h), .. } => Some(format!("{h}({val_expr})")),
        _ => None,
    }
}

/// Like `constraint_to_c_check` but uses `.size()` for C++ `std::string_view`.
pub fn constraint_to_cpp_check(c: &Constraint, val_expr: &str) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => Some(format!("{val_expr}.size() == {n}")),
        Constraint::SizeRange { min, max } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!("{val_expr}.size() >= {lo} && {val_expr}.size() <= {hi}")),
            (Some(lo), None)     => Some(format!("{val_expr}.size() >= {lo}")),
            (None, Some(hi))     => Some(format!("{val_expr}.size() <= {hi}")),
            (None, None)         => None,
        },
        _ => constraint_to_c_check(c, val_expr),
    }
}

// ── Literal formatting ────────────────────────────────────────────────────────

use cddlc_ir::LiteralValue;

pub fn literal_to_rust(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Bool(b)  => b.to_string(),
        LiteralValue::Null     => "()".into(),
        LiteralValue::Uint(n)  => format!("{n}u64"),
        LiteralValue::Int(n)   => format!("{n}i64"),
        LiteralValue::Float(f) => format!("{f:.?}_f64"),
        LiteralValue::Text(s)  => format!("{s:?}"),
        LiteralValue::Bytes(b) => format!("&[{}]", b.iter().map(|x| format!("0x{x:02x}")).collect::<Vec<_>>().join(",")),
    }
}

pub fn literal_to_c(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Bool(b)  => if *b { "1".into() } else { "0".into() },
        LiteralValue::Null     => "NULL".into(),
        LiteralValue::Uint(n)  => format!("{n}ULL"),
        LiteralValue::Int(n)   => format!("{n}LL"),
        LiteralValue::Float(f) => format!("{f:.?}"),
        LiteralValue::Text(s)  => format!("{s:?}"),
        LiteralValue::Bytes(_) => "/* bytes */".into(),
    }
}

// ── Capacity helpers ──────────────────────────────────────────────────────────

use cddlc_ir::Capacity;

pub fn capacity_value(cap: &Capacity, default: usize) -> usize {
    match cap {
        Capacity::Fixed(n)   => *n,
        Capacity::Bounded(n) => *n,
        Capacity::Dynamic    => default,
    }
}

// ── Documentation comments (dgtlrift/cddlc#8) ─────────────────────────────────

/// How a language writes a documentation comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocStyle {
    /// One prefix per line: `///` (Rust, C++, Dart), `#` (Python).
    Line(&'static str),
    /// A block: `/** … */` (TypeScript), `/* … */` (C), one ` * ` line per doc line.
    Block { open: &'static str },
    /// C#'s XML documentation: `/// <summary>` … `/// </summary>`, text escaped.
    CSharpXml,
}

/// Writes `doc`, if any, as a documentation comment in `style`: every line of a
/// multi-line doc gets the comment's prefix, and text that would end the comment or
/// break its markup is escaped.
pub fn emit_doc_lines(w: &mut IndentWriter, doc: Option<&str>, style: DocStyle) {
    let Some(doc) = doc else { return };
    match style {
        DocStyle::Line(prefix) => {
            for line in doc.lines() {
                if line.is_empty() {
                    w.line(prefix);
                } else {
                    w.line(&format!("{prefix} {line}"));
                }
            }
        }
        DocStyle::Block { open } => {
            let lines: Vec<String> = doc.lines().map(|l| l.replace("*/", "* /")).collect();
            if let [only] = lines.as_slice() {
                w.line(&format!("{open} {only} */"));
            } else {
                w.line(open);
                for line in &lines {
                    if line.is_empty() {
                        w.line(" *");
                    } else {
                        w.line(&format!(" * {line}"));
                    }
                }
                w.line(" */");
            }
        }
        DocStyle::CSharpXml => {
            w.line("/// <summary>");
            for line in doc.lines() {
                let text = line.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                w.line(&format!("/// {text}"));
            }
            w.line("/// </summary>");
        }
    }
}

// ── Source comments (dgtlrift/cddlc#9) ────────────────────────────────────────

/// `module` with each type's and field's CDDL source appended to its documentation, as
/// `cddl: <path>:<line>: <text>` ([`cddlc_ir::SourceRef::describe`]), the path relative
/// to `base`. Every backend writes the docs, so every backend shows the sources.
pub fn with_source_docs(module: &cddlc_ir::IrModule, base: Option<&std::path::Path>) -> cddlc_ir::IrModule {
    use cddlc_ir::TypeDef;
    let mut m = module.clone();
    let append = |doc: &mut Option<String>, line: String| match doc {
        Some(d) => { d.push('\n'); d.push_str(&line); }
        None => *doc = Some(line),
    };
    for (name, def) in &mut m.types {
        if let Some(src) = module.sources.types.get(name) {
            let line = src.describe(base);
            match def {
                TypeDef::Struct(s) => append(&mut s.doc, line),
                TypeDef::Enum(e) => append(&mut e.doc, line),
                TypeDef::Alias(a) => append(&mut a.doc, line),
                TypeDef::Array(a) => append(&mut a.doc, line),
                TypeDef::Map(mp) => append(&mut mp.doc, line),
            }
        }
        if let TypeDef::Struct(s) = def {
            for f in &mut s.fields {
                if let Some(src) = module.sources.fields.get(&(name.clone(), f.name.clone())) {
                    append(&mut f.doc, src.describe(base));
                }
            }
        }
    }
    m
}

