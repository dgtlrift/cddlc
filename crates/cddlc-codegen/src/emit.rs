/// Shared code-generation utilities used by all backends.

// ── Indentation ───────────────────────────────────────────────────────────────

/// A simple indented string builder.
pub struct IndentWriter {
    buf: String,
    indent: usize,
    width: usize,
}

impl IndentWriter {
    pub fn new(indent_width: usize) -> Self {
        Self {
            buf: String::new(),
            indent: 0,
            width: indent_width,
        }
    }

    pub fn indent(&mut self) {
        self.indent += 1;
    }
    pub fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

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

    pub fn blank(&mut self) {
        self.buf.push('\n');
    }

    pub fn finish(self) -> String {
        self.buf
    }
}

// ── Name mangling ─────────────────────────────────────────────────────────────

// A CDDL name (RFC 8610 `id`) can hold `$` and `@` (socket names: `$val`,
// `$$ext`), `.` (`session.Status`) and `-`, none of which most targets allow in an
// identifier (dgtlrift/cddlc#123). `.` and `-` separate words; `$` and `@` are dropped.

fn is_word_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.')
}

fn is_dropped(c: char) -> bool {
    matches!(c, '$' | '@')
}

/// Convert a CDDL identifier to snake_case (Rust fields, C variables).
pub fn to_snake_case(s: &str) -> String {
    s.chars()
        .filter(|&c| !is_dropped(c))
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect()
}

/// Convert a CDDL identifier to PascalCase (Rust types, C# types).
pub fn to_pascal_case(s: &str) -> String {
    let s: String = s.chars().filter(|&c| !is_dropped(c)).collect();
    s.split(is_word_separator)
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Convert a CDDL identifier to SCREAMING_SNAKE_CASE (C enum constants).
pub fn to_screaming_snake(s: &str) -> String {
    to_snake_case(s).to_uppercase()
}

// ── Primitive type mapping ────────────────────────────────────────────────────

use cddlc_ir::Primitive;

/// Map a primitive to its Rust type name.
pub fn primitive_to_rust(p: &Primitive, no_std: bool) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::Null => "()",
        Primitive::Undefined => "()",
        Primitive::Uint => "u64",
        Primitive::Int => "i64",
        Primitive::Float16 => "f32", // f16 not stable; use f32
        Primitive::Float32 => "f32",
        Primitive::Float64 => "f64",
        Primitive::Float => "f64",
        Primitive::Tstr => {
            if no_std {
                "&'b str"
            } else {
                "String"
            }
        }
        Primitive::Bstr => {
            if no_std {
                "&'b [u8]"
            } else {
                "Vec<u8>"
            }
        }
        Primitive::Any => {
            if no_std {
                "&'b [u8]"
            } else {
                "Vec<u8>"
            }
        }
    }
}

/// Map a primitive to its C type name.
pub fn primitive_to_c(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::Null => "void*",
        Primitive::Undefined => "void*",
        Primitive::Uint => "uint64_t",
        Primitive::Int => "int64_t",
        Primitive::Float16 => "float",
        Primitive::Float32 => "float",
        Primitive::Float64 => "double",
        Primitive::Float => "double",
        Primitive::Bstr => "cbor_slice_t",
        Primitive::Tstr => "cbor_str_t",
        Primitive::Any => "cbor_any_t",
    }
}

// ── Constraint helpers ────────────────────────────────────────────────────────

use cddlc_ir::Constraint;

/// Emit a Rust boolean expression that checks `val` against a constraint.
/// Returns `None` for constraints that don't produce inline checks.
pub fn constraint_to_rust_check(c: &Constraint, val_expr: &str) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => Some(format!("{val_expr}.len() == {n}")),
        Constraint::SizeRange { min, max } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!(
                "{val_expr}.len() >= {lo} && {val_expr}.len() <= {hi}"
            )),
            (Some(lo), None) => Some(format!("{val_expr}.len() >= {lo}")),
            (None, Some(hi)) => Some(format!("{val_expr}.len() <= {hi}")),
            (None, None) => None,
        },
        Constraint::ValueRangeInt {
            min,
            max,
            inclusive,
        } => {
            let ge = ">="; // CDDL ranges always include the lower bound
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => {
                    Some(format!("{val_expr} {ge} {lo} && {val_expr} {le} {hi}"))
                }
                (Some(lo), None) => Some(format!("{val_expr} {ge} {lo}")),
                (None, Some(hi)) => Some(format!("{val_expr} {le} {hi}")),
                (None, None) => None,
            }
        }
        Constraint::ValueRangeUint {
            min,
            max,
            inclusive,
        } => {
            let ge = ">="; // CDDL ranges always include the lower bound
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => {
                    Some(format!("{val_expr} {ge} {lo} && {val_expr} {le} {hi}"))
                }
                (Some(lo), None) => Some(format!("{val_expr} {ge} {lo}")),
                (None, Some(hi)) => Some(format!("{val_expr} {le} {hi}")),
                (None, None) => None,
            }
        }
        Constraint::ValueRangeF64 {
            min,
            max,
            inclusive,
        } => {
            let ge = ">="; // CDDL ranges always include the lower bound
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!(
                    "{val_expr} {ge} {lo:.?} && {val_expr} {le} {hi:.?}"
                )),
                (Some(lo), None) => Some(format!("{val_expr} {ge} {lo:.?}")),
                (None, Some(hi)) => Some(format!("{val_expr} {le} {hi:.?}")),
                (None, None) => None,
            }
        }
        Constraint::Eq(v) => Some(format!("{val_expr} == {}", literal_to_rust(v))),
        Constraint::Ne(v) => Some(format!("{val_expr} != {}", literal_to_rust(v))),
        Constraint::Regexp { hook: Some(h), .. } => Some(format!("{h}({val_expr})")),
        // The generated crate's `BitsCheck`, for a `u64` or bytes.
        Constraint::Bits(bits) => Some(format!(
            "({val_expr}).bits_ok(&[{}])",
            bits_mask_bytes(bits)
                .iter()
                .map(|b| format!("{b:#04x}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Constraint::Default(_)
        | Constraint::Regexp { hook: None, .. }
        | Constraint::CborEmbedded(_)
        | Constraint::CborSeq(_)
        | Constraint::TextEncoded { .. } => None,
    }
}

/// Emit a C boolean expression checking `val_expr` against a constraint.
pub fn constraint_to_c_check(c: &Constraint, val_expr: &str) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => Some(format!("{val_expr}.len == {n}")),
        Constraint::SizeRange { min, max } => match (min, max) {
            (Some(lo), Some(hi)) => {
                Some(format!("{val_expr}.len >= {lo} && {val_expr}.len <= {hi}"))
            }
            (Some(lo), None) => Some(format!("{val_expr}.len >= {lo}")),
            (None, Some(hi)) => Some(format!("{val_expr}.len <= {hi}")),
            (None, None) => None,
        },
        // The lower bound is always inclusive; `inclusive` is the upper bound's
        // (`a..b` vs `a...b`).
        Constraint::ValueRangeInt {
            min,
            max,
            inclusive,
        } => {
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{val_expr} >= {lo} && {val_expr} {le} {hi}")),
                (Some(lo), None) => Some(format!("{val_expr} >= {lo}")),
                (None, Some(hi)) => Some(format!("{val_expr} {le} {hi}")),
                (None, None) => None,
            }
        }
        Constraint::ValueRangeUint {
            min,
            max,
            inclusive,
        } => {
            let le = if *inclusive { "<=" } else { "<" };
            match (min, max) {
                (Some(lo), Some(hi)) => {
                    Some(format!("{val_expr} >= {lo}U && {val_expr} {le} {hi}U"))
                }
                (Some(lo), None) => Some(format!("{val_expr} >= {lo}U")),
                (None, Some(hi)) => Some(format!("{val_expr} {le} {hi}U")),
                (None, None) => None,
            }
        }
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
            (Some(lo), Some(hi)) => Some(format!(
                "{val_expr}.size() >= {lo} && {val_expr}.size() <= {hi}"
            )),
            (Some(lo), None) => Some(format!("{val_expr}.size() >= {lo}")),
            (None, Some(hi)) => Some(format!("{val_expr}.size() <= {hi}")),
            (None, None) => None,
        },
        _ => constraint_to_c_check(c, val_expr),
    }
}

// ── Literal formatting ────────────────────────────────────────────────────────

use cddlc_ir::LiteralValue;

pub fn literal_to_rust(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Null => "()".into(),
        LiteralValue::Uint(n) => format!("{n}u64"),
        LiteralValue::Int(n) => format!("{n}i64"),
        LiteralValue::Float(f) => format!("{f:.?}_f64"),
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(b) => format!(
            "&[{}]",
            b.iter()
                .map(|x| format!("0x{x:02x}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

pub fn literal_to_c(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Bool(b) => {
            if *b {
                "1".into()
            } else {
                "0".into()
            }
        }
        LiteralValue::Null => "NULL".into(),
        LiteralValue::Uint(n) => format!("{n}ULL"),
        LiteralValue::Int(n) => format!("{n}LL"),
        LiteralValue::Float(f) => format!("{f:.?}"),
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(_) => "/* bytes */".into(),
    }
}

// ── Capacity helpers ──────────────────────────────────────────────────────────

/// The CDDL string of a text (or `null`) enum variant: the literal it was
/// written with (`"free-space"` in `"free-space" / "log-distance"`).
///
/// A variant without a text literal falls back to its lowercased name. Never
/// rebuild the string from the name when a literal exists: the name is
/// PascalCase (`FreeSpace`), so that loses separators and case
/// (dgtlrift/cddlc#36).
pub fn enum_variant_text(v: &cddlc_ir::EnumVariant) -> String {
    let literal = v.constraints.iter().find_map(|c| match c {
        cddlc_ir::Constraint::Eq(cddlc_ir::LiteralValue::Text(t)) => Some(t.as_str()),
        _ => None,
    });
    match (&v.ty, literal) {
        (cddlc_ir::TypeRef::Primitive(cddlc_ir::Primitive::Tstr), Some(t)) => t.to_owned(),
        _ => v.name.to_lowercase().replace('_', "-"),
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// A valid Rust identifier for a CDDL field name: snake_case, with keywords made
/// raw identifiers (`type` → `r#type`), except those that cannot be raw (`self`,
/// `Self`, `super`, `crate`), which get a trailing underscore. Shared by the Rust
/// backend and the Rust interop harness so both name fields the same way.
pub fn rust_field_ident(name: &str) -> String {
    let snake = to_snake_case(name);
    if !RUST_KEYWORDS.contains(&snake.as_str()) {
        snake
    } else if matches!(snake.as_str(), "self" | "Self" | "super" | "crate") {
        format!("{snake}_")
    } else {
        format!("r#{snake}")
    }
}

use cddlc_ir::Capacity;

pub fn capacity_value(cap: &Capacity, default: usize) -> usize {
    match cap {
        Capacity::Fixed(n) => *n,
        Capacity::Bounded(n) => *n,
        Capacity::Dynamic => default,
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
                let text = line
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
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
pub fn with_source_docs(
    module: &cddlc_ir::IrModule,
    base: Option<&std::path::Path>,
) -> cddlc_ir::IrModule {
    use cddlc_ir::TypeDef;
    let mut m = module.clone();
    let append = |doc: &mut Option<String>, line: String| match doc {
        Some(d) => {
            d.push('\n');
            d.push_str(&line);
        }
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

// ── `.bits` (dgtlrift/cddlc#107) ─────────────────────────────────────────────

/// The bytes of a `.bits` mask: bit `n` may be set if bit `n % 8` of byte
/// `n / 8` is (RFC 8610 §3.8.2; bit 0 is the least significant bit of the
/// first byte, as `cddlc validate` numbers them).
pub fn bits_mask_bytes(bits: &[u64]) -> Vec<u8> {
    let len = bits.iter().map(|b| b / 8 + 1).max().unwrap_or(0) as usize;
    let mut m = vec![0u8; len.min(4096)];
    for &b in bits {
        if let Some(byte) = m.get_mut((b / 8) as usize) {
            *byte |= 1 << (b % 8);
        }
    }
    m
}

/// The mask of the bits below 64 an unsigned integer may have set.
pub fn bits_mask_u64(bits: &[u64]) -> u64 {
    bits.iter().filter(|&&b| b < 64).fold(0, |m, b| m | 1 << b)
}

/// Whether any type of the module has a `.bits` constraint, so that the
/// backend emits its helper.
pub fn module_uses_bits(m: &cddlc_ir::IrModule) -> bool {
    use cddlc_ir::TypeDef;
    let has = |cs: &[Constraint]| cs.iter().any(|c| matches!(c, Constraint::Bits(_)));
    m.types.values().any(|t| match t {
        TypeDef::Struct(s) => s.fields.iter().any(|f| has(&f.constraints)),
        TypeDef::Enum(e) => e.variants.iter().any(|v| has(&v.constraints)),
        TypeDef::Alias(a) => has(&a.constraints),
        TypeDef::Array(_) | TypeDef::Map(_) => false,
    })
}

/// Whether any type in the module holds `any` (dgtlrift/cddlc#178), so a
/// backend emits its runtime type for the encoded data item.
pub fn module_uses_any(m: &cddlc_ir::IrModule) -> bool {
    use cddlc_ir::{Primitive, TypeDef, TypeRef};
    fn has(t: &TypeRef) -> bool {
        match t {
            TypeRef::Primitive(p) => *p == Primitive::Any,
            TypeRef::Tagged(_, inner) => has(inner),
            TypeRef::Choice(cs) => cs.iter().any(has),
            TypeRef::Named(_) => false,
        }
    }
    m.types.values().any(|t| match t {
        TypeDef::Struct(s) => {
            s.fields.iter().any(|f| has(&f.ty))
                || s.rest
                    .as_ref()
                    .is_some_and(|r| has(&r.key_ty) || has(&r.val_ty))
        }
        TypeDef::Enum(e) => e.variants.iter().any(|v| has(&v.ty)),
        TypeDef::Alias(a) => has(&a.ty),
        TypeDef::Array(a) => has(&a.element),
        TypeDef::Map(mp) => has(&mp.key_ty) || has(&mp.val_ty),
    })
}

/// For a record whose optional members aren't all at the end (COSE's
/// `Sig_structure`, RFC 9052): how many optional members come before each
/// member. An array of `n` items holds the first `n - required` optional
/// members, in order (dgtlrift/cddlc#134), so with `k = n - required`, the
/// optional member with `j` optional members before it is present when
/// `k > j`, and member `i` is item `i - max(0, before[i] - k)`. `None` for a
/// record whose optional members, if any, all come last, where reading in
/// order until the array ends does the same.
pub fn record_optionals_before(s: &cddlc_ir::StructDef) -> Option<Vec<usize>> {
    if s.encoding != cddlc_ir::StructEncoding::Array {
        return None;
    }
    let middle = s
        .fields
        .windows(2)
        .any(|w| !w[0].occurrence.is_required() && w[1].occurrence.is_required());
    if !middle {
        return None;
    }
    let mut before = 0;
    Some(
        s.fields
            .iter()
            .map(|f| {
                let b = before;
                if !f.occurrence.is_required() {
                    before += 1;
                }
                b
            })
            .collect(),
    )
}

/// A record's repeated member (`[primary, * extension, payload]`), if it has
/// one: its index. Every other member is required, so an array of `n` items
/// holds `n - (members - 1)` of its items, inline, where it stands
/// (dgtlrift/cddlc#134).
pub fn record_repeated(s: &cddlc_ir::StructDef) -> Option<usize> {
    if s.encoding != cddlc_ir::StructEncoding::Array {
        return None;
    }
    s.fields.iter().position(|f| f.occurrence.is_repeated())
}

/// How many items a repeated member may have: at least `min`, and at most
/// `max` when it's bounded (`*`: 0.., `+`: 1.., `n*m`: n..=m).
pub fn repeat_bounds(occ: &cddlc_ir::Occurrence) -> (u64, Option<u64>) {
    match occ {
        cddlc_ir::Occurrence::OneOrMore { .. } => (1, None),
        cddlc_ir::Occurrence::Bounded { min, max, .. } => (u64::from(*min), max.map(u64::from)),
        _ => (0, None),
    }
}

/// Whether a struct's default value (every list empty) encodes: not when a
/// repeated member needs items (`+ step: tstr`), as the encoder refuses it.
pub fn default_is_encodable(s: &cddlc_ir::StructDef) -> bool {
    s.fields.iter().all(|f| repeat_bounds(&f.occurrence).0 == 0)
}

/// A map member's key in JSON: its text, or an integer key as decimal text
/// (`1 => tstr` is `"1"`), as JSON objects have only text keys
/// (dgtlrift/cddlc#176).
pub fn field_key_json(key: &cddlc_ir::FieldKey) -> String {
    match key {
        cddlc_ir::FieldKey::Text(t) => t.to_string(),
        cddlc_ir::FieldKey::Int(n) => n.to_string(),
        cddlc_ir::FieldKey::Uint(n) => n.to_string(),
    }
}

/// Whether a map has a member whose key isn't text (`1 => tstr`), so its
/// decoder must read integer keys too (dgtlrift/cddlc#176).
pub fn has_int_keys(s: &cddlc_ir::StructDef) -> bool {
    s.encoding == cddlc_ir::StructEncoding::Map
        && s.fields
            .iter()
            .any(|f| !matches!(f.key, cddlc_ir::FieldKey::Text(_)))
}

/// A map key's canonical CBOR encoding (RFC 8949 §4.2.1).
fn key_bytes(key: &cddlc_ir::FieldKey) -> Vec<u8> {
    let head = |major: u8, n: u64| -> Vec<u8> {
        let m = major << 5;
        match n {
            0..=23 => vec![m | n as u8],
            24..=0xff => vec![m | 24, n as u8],
            0x100..=0xffff => {
                let mut v = vec![m | 25];
                v.extend((n as u16).to_be_bytes());
                v
            }
            0x1_0000..=0xffff_ffff => {
                let mut v = vec![m | 26];
                v.extend((n as u32).to_be_bytes());
                v
            }
            _ => {
                let mut v = vec![m | 27];
                v.extend(n.to_be_bytes());
                v
            }
        }
    };
    match key {
        cddlc_ir::FieldKey::Uint(n) => head(0, *n),
        cddlc_ir::FieldKey::Int(n) if *n >= 0 => head(0, *n as u64),
        cddlc_ir::FieldKey::Int(n) => head(1, (-1 - *n) as u64),
        cddlc_ir::FieldKey::Text(t) => {
            let mut v = head(3, t.len() as u64);
            v.extend(t.as_bytes());
            v
        }
    }
}

/// The order of a map's members in deterministic CBOR (dCBOR): by their keys'
/// encoded bytes, so integer keys come first and `"b"` before `"aa"`
/// (dgtlrift/cddlc#176).
pub fn dcbor_key_order(a: &cddlc_ir::FieldDef, b: &cddlc_ir::FieldDef) -> std::cmp::Ordering {
    key_bytes(&a.key).cmp(&key_bytes(&b.key))
}

/// Whether a map's other entries (`* tstr => V`) have text keys, through
/// aliases. Their keys may also be integers or a choice of them (COSE's
/// `* label => values`, dgtlrift/cddlc#136).
pub fn rest_keys_are_text(r: &cddlc_ir::MapRest, module: &cddlc_ir::IrModule) -> bool {
    let mut ty = &r.key_ty;
    for _ in 0..16 {
        match ty {
            cddlc_ir::TypeRef::Primitive(cddlc_ir::Primitive::Tstr) => return true,
            cddlc_ir::TypeRef::Named(n) => match module.get(n) {
                Some(cddlc_ir::TypeDef::Alias(a)) => ty = &a.ty,
                _ => return false,
            },
            _ => return false,
        }
    }
    false
}

/// A map, in the module, whose other entries have keys that aren't text: the
/// first one's name. JSON objects have only text keys, so JSON output can't
/// hold them (dgtlrift/cddlc#136).
pub fn map_with_non_text_rest(module: &cddlc_ir::IrModule) -> Option<&str> {
    module.types.values().find_map(|d| match d {
        cddlc_ir::TypeDef::Struct(s) => s
            .rest
            .as_ref()
            .filter(|r| !rest_keys_are_text(r, module))
            .map(|_| s.name.as_str()),
        _ => None,
    })
}

/// Refuses a map whose other entries have keys that aren't text where they
/// can't be held: in JSON output (JSON objects have only text keys), and in
/// CBOR output for a backend that doesn't hold them yet (`cbor_ok` false,
/// dgtlrift/cddlc#181).
pub fn check_rest_keys(
    module: &cddlc_ir::IrModule,
    opts: &crate::CodegenOptions,
    lang: &str,
    cbor_ok: bool,
) -> Result<(), crate::CodegenError> {
    let Some(name) = map_with_non_text_rest(module) else {
        return Ok(());
    };
    if opts.is_json() {
        return Err(crate::CodegenError::new(format!(
            "'{name}': JSON objects have text keys, so a map's other entries can't have integer keys in JSON output (dgtlrift/cddlc#136)"
        )));
    }
    if !cbor_ok {
        return Err(crate::CodegenError::new(format!(
            "'{name}': the {lang} backend can't yet hold a map's other entries with integer keys (dgtlrift/cddlc#181)"
        )));
    }
    Ok(())
}

/// Whether two choice alternatives are maps with the same members (keys,
/// types, occurrences and constraints): nothing on the wire tells them apart
/// (dgtlrift/cddlc#157).
pub fn same_map_shape(
    a: &cddlc_ir::TypeRef,
    b: &cddlc_ir::TypeRef,
    module: &cddlc_ir::IrModule,
) -> bool {
    use cddlc_ir::{StructEncoding, TypeDef, TypeRef};
    let (TypeRef::Named(x), TypeRef::Named(y)) = (a, b) else {
        return false;
    };
    let (Some(TypeDef::Struct(s)), Some(TypeDef::Struct(t))) = (module.get(x), module.get(y))
    else {
        return false;
    };
    s.encoding == StructEncoding::Map
        && t.encoding == StructEncoding::Map
        && s.rest.is_none()
        && t.rest.is_none()
        && s.fields.len() == t.fields.len()
        && s.fields.iter().zip(&t.fields).all(|(f, g)| {
            f.key == g.key
                && f.ty == g.ty
                && format!("{:?}", f.occurrence) == format!("{:?}", g.occurrence)
                && format!("{:?}", f.constraints) == format!("{:?}", g.constraints)
        })
}

/// The module with types renamed (`from` → `to`), at their definitions and
/// everywhere they're used: for a backend whose name for a rule would clash
/// with one its target already has (C's `mode_t`, dgtlrift/cddlc#154).
pub fn rename_types(
    module: &cddlc_ir::IrModule,
    renames: &std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr>,
) -> cddlc_ir::IrModule {
    use cddlc_ir::{TypeDef, TypeRef};
    fn fix(
        ty: &mut TypeRef,
        renames: &std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr>,
    ) {
        match ty {
            TypeRef::Named(n) => {
                if let Some(to) = renames.get(n) {
                    *n = to.clone()
                }
            }
            TypeRef::Tagged(_, inner) => fix(inner, renames),
            TypeRef::Choice(cs) => cs.iter_mut().for_each(|c| fix(c, renames)),
            TypeRef::Primitive(_) => {}
        }
    }
    if renames.is_empty() {
        return module.clone();
    }
    let mut out = module.clone();
    out.types.clear();
    for (name, def) in &module.types {
        let mut def = def.clone();
        let new = renames.get(name).cloned().unwrap_or_else(|| name.clone());
        match &mut def {
            TypeDef::Struct(s) => {
                s.name = new.clone();
                s.fields.iter_mut().for_each(|f| fix(&mut f.ty, renames));
                if let Some(r) = s.rest.as_mut() {
                    fix(&mut r.key_ty, renames);
                    fix(&mut r.val_ty, renames);
                }
            }
            TypeDef::Enum(e) => {
                e.name = new.clone();
                e.variants.iter_mut().for_each(|v| fix(&mut v.ty, renames));
            }
            TypeDef::Array(a) => {
                a.name = new.clone();
                fix(&mut a.element, renames);
            }
            TypeDef::Map(m) => {
                m.name = new.clone();
                fix(&mut m.key_ty, renames);
                fix(&mut m.val_ty, renames);
            }
            TypeDef::Alias(a) => {
                a.name = new.clone();
                fix(&mut a.ty, renames);
            }
        }
        out.types.insert(new.clone(), def);
        if let Some(src) = module.sources.types.get(name) {
            out.sources.types.insert(new, src.clone());
        }
    }
    out.sources.fields = module
        .sources
        .fields
        .iter()
        .map(|((t, f), src)| {
            (
                (
                    renames.get(t).cloned().unwrap_or_else(|| t.clone()),
                    f.clone(),
                ),
                src.clone(),
            )
        })
        .collect();
    if let Some(to) = renames.get(&module.root) {
        out.root = to.clone();
    }
    out
}

/// The C library's and the runtime's `<name>_t` types, which a rule's type
/// can't be named after: POSIX (`<sys/types.h>`, `<time.h>`, `<signal.h>`,
/// `<pthread.h>`), ISO C (`<stddef.h>`, `<stdint.h>`, `<stdio.h>`, `<wchar.h>`)
/// and nanocbor / the generated runtime.
#[rustfmt::skip]
const RESERVED_C_TYPES: &[&str] = &[
    "blkcnt",
    "blksize",
    "cbor_any",
    "cbor_bytes",
    "cbor_slice",
    "cbor_str",
    "clock",
    "clockid",
    "dev",
    "fpos",
    "fsblkcnt",
    "fsfilcnt",
    "gid",
    "id",
    "ino",
    "int16",
    "int32",
    "int64",
    "int8",
    "int_fast16",
    "int_fast32",
    "int_fast64",
    "int_fast8",
    "int_least16",
    "int_least32",
    "int_least64",
    "int_least8",
    "intmax",
    "intptr",
    "json_bytes",
    "key",
    "locale",
    "max_align",
    "mbstate",
    "mode",
    "nanocbor_encoder",
    "nanocbor_value",
    "nlink",
    "off",
    "pid",
    "pthread",
    "pthread_attr",
    "pthread_cond",
    "pthread_key",
    "pthread_mutex",
    "pthread_once",
    "ptrdiff",
    "sig_atomic",
    "sigset",
    "size",
    "ssize",
    "suseconds",
    "time",
    "timer",
    "uid",
    "uint16",
    "uint32",
    "uint64",
    "uint8",
    "uint_fast16",
    "uint_fast32",
    "uint_fast64",
    "uint_fast8",
    "uint_least16",
    "uint_least32",
    "uint_least64",
    "uint_least8",
    "uintmax",
    "uintptr",
    "useconds",
    "wchar",
    "wctype",
    "wint",
];

/// The rules to rename so their C types don't clash with one in
/// [`RESERVED_C_TYPES`]: each to `<module>-<rule>`.
pub fn c_reserved_renames(
    module: &cddlc_ir::IrModule,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    reserved_renames(module, RESERVED_C_TYPES, crate::to_snake_case)
}

/// The rules whose name, as a backend writes it (`case`), is one of
/// `reserved`: each renamed `<module>-<rule>`, to use with [`rename_types`].
pub fn reserved_renames(
    module: &cddlc_ir::IrModule,
    reserved: &[&str],
    case: fn(&str) -> String,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    let mut out = std::collections::HashMap::new();
    // A socket (`$eid`) beside a rule of its name (`eid`): both would be
    // `Eid`, so the socket is `<name>-socket` (RFC 9171's `eid` and `$eid`).
    for name in module.types.keys() {
        let bare = name.trim_start_matches('$');
        if bare.len() < name.len()
            && module
                .types
                .keys()
                .any(|k| k.as_str() != name.as_str() && case(k) == case(bare))
        {
            let mut to: smol_str::SmolStr = format!("{bare}-socket").into();
            while module.types.contains_key(&to) {
                to = format!("{to}-type").into();
            }
            out.insert(name.clone(), to);
        }
    }
    for name in module.types.keys() {
        if reserved.contains(&case(name).as_str()) {
            let mut to: smol_str::SmolStr = format!("{}-{name}", module.name).into();
            while module.types.contains_key(&to) {
                to = format!("{to}-type").into();
            }
            out.insert(name.clone(), to);
        }
    }
    out
}

/// Names a Rust rule's type can't take: the prelude's types, traits and
/// variants, and what the generated crate uses unqualified (minicbor's,
/// serde's). The runtime's names (`ErrorCode`) a rule may take (#149).
/// PascalCase, as the type is named.
#[rustfmt::skip]
const RESERVED_RUST_TYPES: &[&str] = &[
    "AsMut",
    "AsRef",
    "Box",
    "Clone",
    "Copy",
    "Debug",
    "Decode",
    "DecodeError",
    "Decoder",
    "Default",
    "Deserialize",
    "Deserializer",
    "Drop",
    "Encode",
    "EncodeError",
    "Encoder",
    "Engine",
    "Eq",
    "Err",
    "Extend",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "Hash",
    "Into",
    "IntoIterator",
    "Iterator",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Result",
    "Self",
    "Send",
    "Serialize",
    "Serializer",
    "Sized",
    "Some",
    "String",
    "Sync",
    "ToOwned",
    "ToString",
    "TryFrom",
    "TryInto",
    "Vec",
    "Write",
    // The runtime's type for `any` (dgtlrift/cddlc#178).
    "AnyItem",
];

/// The rules to rename so their Rust types don't clash with one in
/// [`RESERVED_RUST_TYPES`] (a rule named `result` hid `Result`,
/// dgtlrift/cddlc#191): each to `<module>-<rule>`.
pub fn rust_reserved_renames(
    module: &cddlc_ir::IrModule,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    reserved_renames(module, RESERVED_RUST_TYPES, crate::to_pascal_case)
}

/// Names a C# rule's class can't take: the members of the `JsonConverter`
/// it gets, inside which the class is named (`Write`, dgtlrift/cddlc#191),
/// and the members every generated class has, as a class can't have a member
/// of its own name (`items = [* any]` and its `Items`).
#[rustfmt::skip]
const RESERVED_CSHARP_TYPES: &[&str] = &[
    "CanConvert",
    "HandleNull",
    "Read",
    "Type",
    "Write",
    // The generated classes' members
    "Decode",
    "Default",
    "Encode",
    "Entries",
    "FromJson",
    "Items",
    "ToJson",
    "Value",
];

/// Names a C++ rule's type can't take: the runtime's, in the same namespace
/// (WebDriver BiDi's `ErrorCode`).
#[rustfmt::skip]
const RESERVED_CPP_TYPES: &[&str] = &[
    "Box",
    "Callbacks",
    "ErrorCode",
    "FieldError",
];

/// The rules to rename so their C++ types don't clash with one in
/// [`RESERVED_CPP_TYPES`]: each to `<module>-<rule>`.
pub fn cpp_reserved_renames(
    module: &cddlc_ir::IrModule,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    reserved_renames(module, RESERVED_CPP_TYPES, crate::to_pascal_case)
}

/// The rules to rename so their C# classes don't clash with one in
/// [`RESERVED_CSHARP_TYPES`]: each to `<module>-<rule>`.
pub fn csharp_reserved_renames(
    module: &cddlc_ir::IrModule,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    reserved_renames(module, RESERVED_CSHARP_TYPES, crate::to_pascal_case)
}

/// Names a Dart rule's class can't take (dgtlrift/cddlc#222). A class
/// declared in the library hides an imported one of the same name, so these
/// are the names the generated library and tests use from elsewhere:
/// `dart:core`'s types (a class `List` breaks `List<int>`), `dart:typed_data`'s
/// and `package:cbor`'s, and `package:test`'s, which the tests import beside
/// the library (two imports of one name are ambiguous).
#[rustfmt::skip]
const RESERVED_DART_TYPES: &[&str] = &[
    // dart:core
    "ArgumentError",
    "AssertionError",
    "BigInt",
    "Comparable",
    "DateTime",
    "Deprecated",
    "Duration",
    "Enum",
    "Error",
    "Exception",
    "Expando",
    "FormatException",
    "Function",
    "Future",
    "Invocation",
    "Iterable",
    "Iterator",
    "List",
    "Map",
    "MapEntry",
    "Never",
    "Null",
    "Object",
    "Pattern",
    "RangeError",
    "Record",
    "RegExp",
    "Set",
    "Sink",
    "StackTrace",
    "StateError",
    "Stopwatch",
    "Stream",
    "String",
    "StringBuffer",
    "Symbol",
    "Type",
    "TypeError",
    "UnimplementedError",
    "UnsupportedError",
    "Uri",
    "WeakReference",
    // dart:typed_data
    "ByteBuffer",
    "ByteData",
    "Endian",
    "Float32List",
    "Float64List",
    "Int16List",
    "Int32List",
    "Int64List",
    "Int8List",
    "TypedData",
    "Uint16List",
    "Uint32List",
    "Uint64List",
    "Uint8List",
    // package:cbor
    "Cbor",
    "CborBigInt",
    "CborBool",
    "CborBytes",
    "CborDecodeException",
    "CborFloat",
    "CborInt",
    "CborList",
    "CborMap",
    "CborNull",
    "CborSimpleValue",
    "CborSmallInt",
    "CborString",
    "CborUndefined",
    "CborValue",
    // package:test
    "Matcher",
    "OnPlatform",
    "Retry",
    "Skip",
    "Tags",
    "TestOn",
    "Timeout",
];

/// The rules to rename so their Dart classes don't clash with one in
/// [`RESERVED_DART_TYPES`]: each to `<module>-<rule>`.
pub fn dart_reserved_renames(
    module: &cddlc_ir::IrModule,
) -> std::collections::HashMap<smol_str::SmolStr, smol_str::SmolStr> {
    reserved_renames(module, RESERVED_DART_TYPES, crate::to_pascal_case)
}

// ── Recursion in languages that hold values inline (C, C++) ──────────────────

/// The named types `ty` holds inline: itself, under any tag, and each named
/// alternative of a choice.
fn inline_refs(ty: &cddlc_ir::TypeRef) -> Vec<&str> {
    use cddlc_ir::TypeRef;
    match ty {
        TypeRef::Named(n) => vec![n.as_str()],
        TypeRef::Tagged(_, inner) => inline_refs(inner),
        TypeRef::Choice(cs) => cs.iter().flat_map(inline_refs).collect(),
        TypeRef::Primitive(_) => vec![],
    }
}

/// The types `name` holds inline in C: every member's (a repeated member is
/// an array of them in place), its other entries' keys and values, each
/// variant's, an array's items, a table's keys and values, an alias's type.
fn inline_edges<'m>(name: &str, module: &'m cddlc_ir::IrModule) -> Vec<&'m str> {
    use cddlc_ir::TypeDef;
    match module.get(name) {
        Some(TypeDef::Struct(s)) => s
            .fields
            .iter()
            .flat_map(|f| inline_refs(&f.ty))
            .chain(s.rest.iter().flat_map(|r| {
                inline_refs(&r.key_ty)
                    .into_iter()
                    .chain(inline_refs(&r.val_ty))
            }))
            .collect(),
        Some(TypeDef::Enum(e)) => e.variants.iter().flat_map(|v| inline_refs(&v.ty)).collect(),
        Some(TypeDef::Array(a)) => inline_refs(&a.element),
        Some(TypeDef::Map(m)) => inline_refs(&m.key_ty)
            .into_iter()
            .chain(inline_refs(&m.val_ty))
            .collect(),
        Some(TypeDef::Alias(a)) => inline_refs(&a.ty),
        None => vec![],
    }
}

/// Whether a member or variant of `from` holding `ty` must be held through a
/// pointer in a language that holds values inline (C, C++): `ty` holds `from`
/// again, inline, so the type would contain itself (dgtlrift/cddlc#156).
/// Every cycle passes through a struct or an enum, so pointers at their
/// members and variants are enough.
pub fn is_indirect(from: &str, ty: &cddlc_ir::TypeRef, module: &cddlc_ir::IrModule) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<&str> = inline_refs(ty);
    while let Some(n) = stack.pop() {
        if n == from {
            return true;
        }
        if seen.insert(n) {
            stack.extend(inline_edges(n, module));
        }
    }
    false
}

/// The module's types in an order where each comes after every type it holds
/// inline, except through a pointer ([`is_indirect`]): the order C and C++
/// declare them in.
pub fn inline_decl_order(module: &cddlc_ir::IrModule) -> Vec<smol_str::SmolStr> {
    use cddlc_ir::TypeDef;
    fn visit(
        n: &str,
        module: &cddlc_ir::IrModule,
        done: &mut std::collections::HashSet<smol_str::SmolStr>,
        busy: &mut std::collections::HashSet<smol_str::SmolStr>,
        out: &mut Vec<smol_str::SmolStr>,
    ) {
        if done.contains(n) || busy.contains(n) || module.get(n).is_none() {
            return;
        }
        busy.insert(n.into());
        // The types `n` holds by value, not through a pointer.
        let direct: Vec<&str> = match module.get(n) {
            Some(TypeDef::Struct(s)) => s
                .fields
                .iter()
                .filter(|f| !is_indirect(n, &f.ty, module))
                .flat_map(|f| inline_refs(&f.ty))
                .chain(s.rest.iter().flat_map(|r| {
                    inline_refs(&r.key_ty)
                        .into_iter()
                        .chain(inline_refs(&r.val_ty))
                }))
                .collect(),
            Some(TypeDef::Enum(e)) => e
                .variants
                .iter()
                .filter(|v| !is_indirect(n, &v.ty, module))
                .flat_map(|v| inline_refs(&v.ty))
                .collect(),
            _ => inline_edges(n, module),
        };
        for d in direct {
            visit(d, module, done, busy, out);
        }
        busy.remove(n);
        done.insert(n.into());
        out.push(n.into());
    }
    let mut done = std::collections::HashSet::new();
    let mut busy = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in module.types.keys() {
        visit(n, module, &mut done, &mut busy, &mut out);
    }
    out
}

/// C++'s keywords and alternative tokens, which a member or function can't be
/// named.
const CPP_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "char8_t",
    "char16_t",
    "char32_t",
    "class",
    "compl",
    "concept",
    "const",
    "consteval",
    "constexpr",
    "constinit",
    "const_cast",
    "continue",
    "co_await",
    "co_return",
    "co_yield",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "dynamic_cast",
    "else",
    "enum",
    "explicit",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "requires",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "while",
    "xor",
    "xor_eq",
];

/// A C++ identifier for a CDDL name: snake_case, with a trailing `_` when
/// that's a keyword (`protected` → `protected_`, `this` → `this_`).
pub fn cpp_ident(name: &str) -> String {
    let s = to_snake_case(name);
    if CPP_KEYWORDS.contains(&s.as_str()) {
        format!("{s}_")
    } else {
        s
    }
}

/// A choice variant's class name: its PascalCase name, with a `_` when an
/// enum's own member has it (`action: "default" / "cancel"`, WebDriver BiDi,
/// would be a class `Default` beside the static `Default`).
pub fn csharp_variant_name(name: &str) -> String {
    let p = crate::to_pascal_case(name);
    if matches!(
        p.as_str(),
        "Default" | "Encode" | "Decode" | "ToJson" | "FromJson" | "Instance" | "Value"
    ) {
        format!("{p}_")
    } else {
        p
    }
}
