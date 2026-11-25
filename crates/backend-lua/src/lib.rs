/// Lua backend for cddlc: Lua 5.3, 5.4 and 5.5, with no runtime dependencies.
///
/// Emits a LuaRocks package:
///   <module>-cbor/
///     <module>-cbor-0.1.0-1.rockspec
///     src/<module>.lua           ← types + an embedded CBOR (or JSON) codec
///     spec/<module>_spec.lua     ← busted roundtrip specs
///
/// Every type is a table on the module with `validate`, `to_raw` / `from_raw`
/// (to and from a typed item model, which keeps text and bytes, and integers
/// and floats, apart) and `encode` / `decode`. Structs and arrays are tables
/// with that metatable (`v:encode()`); enums are `{ kind = ..., value = ... }`
/// tables made by constructors (`M.Level.Uint(5)`); aliases are plain values
/// (`M.DeviceId.encode("...")`).

mod runtime;

use cddlc_codegen::{
    to_pascal_case, to_snake_case, Backend, CodegenError, CodegenOptions, CodegenOutput,
    GeneratedFile, IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, FieldDef, IrModule, LiteralValue,
    MapDef, Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

pub struct LuaBackend;

impl Backend for LuaBackend {
    fn language_name(&self) -> &str { "Lua" }
    fn file_extension(&self) -> &str { "lua" }

    fn generate(&self, module: &IrModule, opts: &CodegenOptions) -> Result<CodegenOutput, CodegenError> {
        let name = module.name.as_str();
        let dir = format!("{name}-{}", if opts.is_json() { "json" } else { "cbor" });
        let modname = lua_module_name(name, opts.namespace.as_deref());
        let file = modname.replace('.', "/");
        let spec = modname.replace('.', "_");
        let cx = Cx { module, opts, modname: &modname };
        Ok(CodegenOutput {
            files: vec![
                GeneratedFile { path: format!("{dir}/{}-0.1.0-1.rockspec", dir.to_lowercase()), content: emit_rockspec(&cx, &dir, &file) },
                GeneratedFile { path: format!("{dir}/src/{file}.lua"), content: emit_module(&cx) },
                GeneratedFile { path: format!("{dir}/spec/{spec}_spec.lua"), content: emit_spec(&cx) },
            ],
        })
    }
}

struct Cx<'a> {
    module:  &'a IrModule,
    opts:    &'a CodegenOptions,
    modname: &'a str,
}

impl Cx<'_> {
    fn json(&self) -> bool { self.opts.is_json() }
    fn format(&self) -> &'static str { if self.json() { "JSON" } else { "CBOR" } }
}

// ── Names ─────────────────────────────────────────────────────────────────────

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "global", "goto",
    "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Member names that would hide a type's functions on an instance.
const RESERVED_MEMBERS: &[&str] = &["new", "validate", "encode", "decode", "to_raw", "from_raw"];

/// The Lua module name: `--namespace` when given, else the module name, each
/// dotted part a lowercase identifier (`simple-values` → `simple_values`).
pub fn lua_module_name(module: &str, namespace: Option<&str>) -> String {
    namespace.unwrap_or(module)
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut s: String = p.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' })
                .collect();
            if s.starts_with(|c: char| c.is_ascii_digit()) { s.insert(0, '_'); }
            if LUA_KEYWORDS.contains(&s.as_str()) { s.push('_'); }
            s
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The type table of a CDDL rule on the module (`M.SensorReading`).
pub fn lua_type_name(name: &str) -> String {
    let n = to_pascal_case(name);
    if n.is_empty() { "Type".into() } else { n }
}

/// The Lua name of a struct member: snake_case, renamed when it is a keyword or
/// would hide one of the type's functions.
pub fn lua_member_name(name: &str) -> String {
    let mut s = to_snake_case(name);
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) { s.insert(0, '_'); }
    if LUA_KEYWORDS.contains(&s.as_str()) || RESERVED_MEMBERS.contains(&s.as_str()) { s.push('_'); }
    s
}

/// A Lua string literal; bytes outside printable ASCII are `\ddd` escapes.
pub fn lua_str(s: &str) -> String {
    lua_bytes(s.as_bytes())
}

/// A Lua string literal holding these bytes.
pub fn lua_bytes(b: &[u8]) -> String {
    let mut out = String::from("\"");
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(c as char),
            _ => {
                // \ddd takes up to three digits: pad so a following digit isn't read in.
                let next_digit = b.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if next_digit { out.push_str(&format!("\\{c:03}")) } else { out.push_str(&format!("\\{c}")) }
            }
        }
    }
    out.push('"');
    out
}

/// A Lua integer literal; one past i64::MAX is written in hex, which Lua reads
/// as its 64-bit pattern.
fn lua_int(n: i128) -> String {
    if n > i64::MAX as i128 { format!("0x{:x}", n as u64) }
    else if n == i64::MIN as i128 { "math.mininteger".into() }
    else { n.to_string() }
}

fn lua_float(f: f64) -> String {
    let s = format!("{f:?}");
    if s.contains(['.', 'e', 'E', 'n', 'i']) { s } else { format!("{s}.0") }
}

fn header(w: &mut IndentWriter, cx: &Cx) {
    for l in cddlc_codegen::header_comment(cx.opts, cddlc_codegen::CommentStyle::DashDash) {
        w.line(&l);
    }
    w.line(&format!("-- Format: {}; Lua 5.3+", if cx.json() { "JSON (RFC 8259)" } else { "CBOR (RFC 8949)" }));
    if cx.opts.dcbor { w.line("-- Encoding: deterministic (dCBOR)"); }
}

fn doc(w: &mut IndentWriter, d: Option<&str>, fallback: &str) {
    match d {
        Some(text) if !text.trim().is_empty() => {
            for (i, l) in text.lines().enumerate() {
                let prefix = if i == 0 { "---" } else { "--" };
                if l.trim().is_empty() { w.line(prefix); } else { w.line(&format!("{prefix} {l}")); }
            }
        }
        _ => w.line(&format!("--- {fallback}")),
    }
}

// ── rockspec ──────────────────────────────────────────────────────────────────

fn emit_rockspec(cx: &Cx, dir: &str, file: &str) -> String {
    let mut s = String::new();
    s.push_str(&cddlc_codegen::header_text(cx.opts, cddlc_codegen::CommentStyle::DashDash));
    s.push_str(&format!(r#"rockspec_format = "3.0"
package = "{pkg}"
version = "0.1.0-1"
source = {{ url = "." }}
description = {{
  summary = "{name} types for {fmt}, @generated by cddlc",
}}
dependencies = {{
  "lua >= 5.3",
}}
test_dependencies = {{
  "busted",
}}
test = {{
  type = "busted",
}}
build = {{
  type = "builtin",
  modules = {{
    ["{modname}"] = "src/{file}.lua",
  }},
}}
"#, pkg = dir.to_lowercase(), name = cx.module.name, fmt = cx.format(), modname = cx.modname));
    s
}

// ── Module ────────────────────────────────────────────────────────────────────

fn emit_module(cx: &Cx) -> String {
    let mut w = IndentWriter::new(2);
    header(&mut w, cx);
    w.blank();
    w.line(&format!("--- The types of the {} CDDL schema, encoded and decoded as {}.", cx.module.name, cx.format()));
    w.line(&format!("-- @module {}", cx.modname));
    w.line("local M = {}");
    w.blank();
    let mut s = w.finish();
    s.push_str(runtime::COMMON);
    s.push_str(if cx.json() { runtime::JSON } else { runtime::CBOR });
    let mut w = IndentWriter::new(2);
    w.blank();
    w.line("-- ── Types ─────────────────────────────────────────────────────────────────────");
    for (_, def) in &cx.module.types {
        w.blank();
        match def {
            TypeDef::Struct(st) => emit_struct(&mut w, cx, st),
            TypeDef::Enum(e) => emit_enum(&mut w, cx, e),
            TypeDef::Array(a) => emit_array(&mut w, cx, a),
            TypeDef::Alias(a) => emit_alias(&mut w, cx, a),
            TypeDef::Map(m) => emit_table(&mut w, cx, m),
        }
    }
    w.blank();
    w.line("return M");
    s.push_str(&w.finish());
    s
}

/// The type table, with its metatable fields for instances.
fn open_type(w: &mut IndentWriter, t: &str, instances: bool) {
    w.line(&format!("M.{t} = {{}}"));
    if instances {
        w.line(&format!("M.{t}.__index = M.{t}"));
        w.line(&format!("M.{t}.__name = {}", lua_str(t)));
    }
    w.blank();
}

/// `encode` / `decode`, shared by every type.
fn emit_encode_decode(w: &mut IndentWriter, cx: &Cx, t: &str) {
    let f = cx.format();
    w.blank();
    w.line(&format!("--- Encodes a {t} as {f}; raises an error if it breaks a constraint."));
    w.line(&format!("function M.{t}.encode(v) return R.encode(M.{t}.to_raw(v)) end"));
    w.blank();
    w.line(&format!("--- Decodes a {t} from {f}; raises an error on invalid input."));
    w.line(&format!("function M.{t}.decode(data) return M.{t}.from_raw(R.decode(data)) end"));
}

// ── Values ────────────────────────────────────────────────────────────────────

/// The primitive under tags, if `ty` is one.
fn base_primitive(ty: &TypeRef) -> Option<&Primitive> {
    match ty {
        TypeRef::Primitive(p) => Some(p),
        TypeRef::Tagged(_, inner) => base_primitive(inner),
        _ => None,
    }
}

fn is_float(p: &Primitive) -> bool {
    matches!(p, Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float)
}

fn is_repeated(occ: &Occurrence) -> bool {
    matches!(occ, Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. } | Occurrence::Bounded { .. })
}

/// A Lua expression for `x`'s item.
fn to_raw(ty: &TypeRef, x: &str) -> String {
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => format!("R.uint({x})"),
            Primitive::Int => format!("R.int({x})"),
            Primitive::Float16 | Primitive::Float32 => format!("R.f32({x})"),
            Primitive::Float64 | Primitive::Float => format!("R.f64({x})"),
            Primitive::Bool => format!("R.bool({x})"),
            Primitive::Tstr => format!("R.text({x})"),
            Primitive::Bstr | Primitive::Any => format!("R.bytes({x})"),
            Primitive::Null | Primitive::Undefined => "R.null()".into(),
        },
        TypeRef::Named(n) => format!("M.{}.to_raw({x})", lua_type_name(n)),
        TypeRef::Tagged(tag, inner) => format!("R.tag({}, {})", lua_int(tag.0 as i128), to_raw(inner, x)),
        TypeRef::Choice(_) => format!("R.from_lua({x})"),
    }
}

/// A Lua expression reading the item `item` as a value of `ty`, its kind checked.
fn from_raw(ty: &TypeRef, item: &str, at: &str) -> String {
    let at_lit = lua_str(at);
    match ty {
        TypeRef::Primitive(p) => {
            let f = match p {
                Primitive::Uint => "as_uint",
                Primitive::Int => "as_int",
                Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float => "as_float",
                Primitive::Bool => "as_bool",
                Primitive::Tstr => "as_text",
                Primitive::Bstr | Primitive::Any => "as_bytes",
                Primitive::Null | Primitive::Undefined => "as_null",
            };
            format!("R.{f}({item}, {at_lit})")
        }
        TypeRef::Named(n) => format!("M.{}.from_raw({item})", lua_type_name(n)),
        TypeRef::Tagged(tag, inner) => from_raw(inner, &format!("R.untag({item}, {}, {at_lit})", lua_int(tag.0 as i128)), at),
        TypeRef::Choice(_) => format!("R.to_lua({item})"),
    }
}

/// A statement checking the Lua value `x` has `ty`'s Lua type (and, for a named
/// type, its constraints).
fn want(ty: &TypeRef, x: &str, at: &str) -> String {
    let at_lit = lua_str(at);
    match ty {
        TypeRef::Primitive(p) => {
            let f = match p {
                Primitive::Uint | Primitive::Int => "want_integer",
                Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float => "want_number",
                Primitive::Bool => "want_boolean",
                Primitive::Tstr => "want_text",
                Primitive::Bstr | Primitive::Any => "want_string",
                Primitive::Null | Primitive::Undefined => "want_null",
            };
            format!("R.{f}({x}, {at_lit})")
        }
        TypeRef::Named(n) => format!("M.{}.validate({x})", lua_type_name(n)),
        TypeRef::Tagged(_, inner) => want(inner, x, at),
        TypeRef::Choice(_) => format!("R.from_lua({x})"),
    }
}

// ── Constraints ───────────────────────────────────────────────────────────────

fn literal_fits(v: &LiteralValue, p: &Primitive) -> bool {
    match v {
        LiteralValue::Bool(_) => matches!(p, Primitive::Bool),
        LiteralValue::Text(_) => matches!(p, Primitive::Tstr),
        LiteralValue::Uint(_) | LiteralValue::Int(_) => matches!(p, Primitive::Uint | Primitive::Int) || is_float(p),
        LiteralValue::Float(_) => is_float(p),
        LiteralValue::Bytes(_) => matches!(p, Primitive::Bstr | Primitive::Any),
        LiteralValue::Null => false,
    }
}

fn lua_literal(v: &LiteralValue) -> Option<String> {
    Some(match v {
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Text(s) => lua_str(s),
        LiteralValue::Uint(n) => lua_int(*n as i128),
        LiteralValue::Int(n) => lua_int(*n as i128),
        LiteralValue::Float(f) => lua_float(*f),
        LiteralValue::Bytes(b) => lua_bytes(b),
        LiteralValue::Null => return None,
    })
}

/// A Lua boolean expression that holds when `x` (a value of primitive `p`)
/// meets the constraint, or `None` when it does not apply.
fn lua_check(c: &Constraint, p: &Primitive, x: &str) -> Option<String> {
    // `#` counts bytes: CDDL's .size on text is its UTF-8 length.
    let len_ok = matches!(p, Primitive::Tstr | Primitive::Bstr | Primitive::Any);
    let is_uint = matches!(p, Primitive::Uint);
    let numeric = matches!(p, Primitive::Uint | Primitive::Int) || is_float(p);
    // A uint above 2^63 is a negative Lua integer: compare unsigned.
    let ge = |lo: i128| if is_uint { format!("R.ule({}, {x})", lua_int(lo)) } else { format!("{x} >= {}", lua_int(lo)) };
    let le = |hi: i128, inclusive: bool| match (is_uint, inclusive) {
        (true, true) => format!("R.ule({x}, {})", lua_int(hi)),
        (true, false) => format!("math.ult({x}, {})", lua_int(hi)),
        (false, true) => format!("{x} <= {}", lua_int(hi)),
        (false, false) => format!("{x} < {}", lua_int(hi)),
    };
    let join = |parts: Vec<String>| (!parts.is_empty()).then(|| parts.join(" and "));
    match c {
        Constraint::SizeExact(n) if len_ok => Some(format!("#{x} == {n}")),
        Constraint::SizeRange { min, max } if len_ok => {
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("#{x} >= {lo}")); }
            if let Some(hi) = max { parts.push(format!("#{x} <= {hi}")); }
            join(parts)
        }
        Constraint::ValueRangeInt { min, max, inclusive } if numeric => {
            let mut parts = Vec::new();
            if let Some(lo) = min.filter(|lo| !(is_uint && *lo <= 0)) { parts.push(ge(lo as i128)); }
            match max {
                // No uint is at or below a negative bound.
                Some(hi) if is_uint && *hi < 0 => return Some("false".into()),
                Some(hi) => parts.push(le(*hi as i128, *inclusive)),
                None => {}
            }
            join(parts)
        }
        Constraint::ValueRangeUint { min, max, inclusive } if numeric => {
            // No int reaches a lower bound past i64::MAX; every int is below such an upper one.
            if min.is_some_and(|lo| matches!(p, Primitive::Int) && lo > i64::MAX as u64) { return Some("false".into()); }
            let mut parts = Vec::new();
            if let Some(lo) = min.filter(|lo| *lo > 0) { parts.push(ge(lo as i128)); }
            if let Some(hi) = max.filter(|hi| is_uint || is_float(p) || *hi <= i64::MAX as u64) { parts.push(le(hi as i128, *inclusive)); }
            join(parts)
        }
        Constraint::ValueRangeF64 { min, max, inclusive } if is_float(p) => {
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("{x} >= {}", lua_float(*lo))); }
            if let Some(hi) = max { parts.push(format!("{x} {} {}", if *inclusive { "<=" } else { "<" }, lua_float(*hi))); }
            join(parts)
        }
        Constraint::Eq(v) if literal_fits(v, p) => lua_literal(v).map(|l| format!("{x} == {l}")),
        Constraint::Ne(v) if literal_fits(v, p) => lua_literal(v).map(|l| format!("{x} ~= {l}")),
        // `.bits` (dgtlrift/cddlc#107): no bit outside the mask is set.
        Constraint::Bits(bits) if is_uint => Some(format!(
            "({x} & ~{}) == 0", lua_int(cddlc_codegen::bits_mask_u64(bits) as i128)
        )),
        Constraint::Bits(bits) if matches!(p, Primitive::Bstr) => {
            let m = cddlc_codegen::bits_mask_bytes(bits).iter().map(|b| b.to_string()).collect::<Vec<_>>().join(", ");
            Some(format!(
                "(function(s) local m = {{ {m} }} for i = 1, #s do if (s:byte(i) & ~(m[i] or 0)) ~= 0 then return false end end return true end)({x})"
            ))
        }
        _ => None,
    }
}

fn bounds_description<T: std::fmt::Display>(what: &str, min: Option<T>, max: Option<T>, inclusive: bool) -> String {
    let le = if inclusive { "<=" } else { "<" };
    match (min, max) {
        (Some(lo), Some(hi)) => format!("{what} must be >= {lo} and {le} {hi}"),
        (Some(lo), None)     => format!("{what} must be >= {lo}"),
        (None, Some(hi))     => format!("{what} must be {le} {hi}"),
        (None, None)         => format!("{what} out of range"),
    }
}

fn describe(c: &Constraint) -> String {
    match c {
        Constraint::SizeExact(n) => format!("length must be {n}"),
        Constraint::SizeRange { min, max } => bounds_description("length", *min, *max, true),
        Constraint::ValueRangeInt { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::ValueRangeUint { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::ValueRangeF64 { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::Eq(v) => format!("must equal {}", literal_text(v)),
        Constraint::Ne(v) => format!("must not equal {}", literal_text(v)),
        _ => "constraint violated".into(),
    }
}

fn literal_text(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(b) => format!("h'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        LiteralValue::Null => "nil".into(),
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Uint(n) => n.to_string(),
        LiteralValue::Int(n) => n.to_string(),
        LiteralValue::Float(f) => f.to_string(),
    }
}

fn emit_checks(w: &mut IndentWriter, ty: &TypeRef, constraints: &[Constraint], x: &str, at: &str) {
    let Some(p) = base_primitive(ty) else { return };
    for c in constraints {
        if let Some(check) = lua_check(c, p, x) {
            w.line(&format!("if not ({check}) then R.fail({}, {}) end", lua_str(at), lua_str(&describe(c))));
        }
    }
}

// ── Struct ────────────────────────────────────────────────────────────────────

fn struct_fields<'a>(s: &'a StructDef, cx: &Cx) -> Vec<&'a FieldDef> {
    let mut fs: Vec<_> = s.fields.iter().collect();
    if cx.opts.dcbor {
        fs.sort_by(|a, b| a.name.cmp(&b.name));
    }
    fs
}

fn emit_struct(w: &mut IndentWriter, cx: &Cx, s: &StructDef) {
    let t = lua_type_name(&s.name);
    doc(w, s.doc.as_deref(), &format!("The {} map.", s.name));
    if s.deprecated { w.line("-- @deprecated"); }
    let members: Vec<String> = s.fields.iter().map(|f| {
        let opt = if matches!(f.occurrence, Occurrence::Optional) { " (optional)" } else { "" };
        format!("-- @field {} {}{opt}", lua_member_name(&f.name), f.name)
    }).collect();
    for m in members { w.line(&m); }
    if let Some(r) = &s.rest {
        w.line(&format!("-- @field {} the map's other entries: a table from text key to value (optional)", lua_member_name(&r.name)));
    }
    open_type(w, &t, true);

    w.line(&format!("--- Makes a {t} from a table of its members."));
    w.line(&format!("function M.{t}.new(t) return setmetatable(t or {{}}, M.{t}) end"));
    w.blank();

    // validate
    w.line("--- Checks every member's type and CDDL constraints; raises an error if one fails.");
    w.line(&format!("function M.{t}.validate(v)"));
    w.indent();
    w.line(&format!("R.want_table(v, {})", lua_str(&t)));
    for f in &s.fields {
        let m = format!("v.{}", lua_member_name(&f.name));
        let at = format!("{t}.{}", f.name);
        match &f.occurrence {
            Occurrence::Optional => {
                w.line(&format!("if {m} ~= nil then"));
                w.indent();
                w.line(&want(&f.ty, &m, &at));
                emit_checks(w, &f.ty, &f.constraints, &m, &at);
                w.dedent();
                w.line("end");
            }
            o if is_repeated(o) => {
                w.line(&format!("R.want_table({m}, {})", lua_str(&at)));
                w.line(&format!("for _, x in ipairs({m}) do {} end", want(&f.ty, "x", &at)));
            }
            _ => {
                w.line(&format!("if {m} == nil then R.fail({}, \"missing\") end", lua_str(&at)));
                w.line(&want(&f.ty, &m, &at));
                emit_checks(w, &f.ty, &f.constraints, &m, &at);
            }
        }
    }
    // The map's other entries (dgtlrift/cddlc#82): text keys no member has.
    if let Some(r) = &s.rest {
        let m = format!("v.{}", lua_member_name(&r.name));
        let at = format!("{t}.{}", r.name);
        let named: Vec<String> = s.fields.iter().map(|f| format!("[{}] = true", lua_str(&f.name))).collect();
        w.line(&format!("if {m} ~= nil then"));
        w.indent();
        w.line(&format!("R.want_table({m}, {})", lua_str(&at)));
        w.line(&format!("local named = {{ {} }}", named.join(", ")));
        w.line(&format!("for k, x in pairs({m}) do"));
        w.indent();
        w.line(&format!("R.want_text(k, {})", lua_str(&format!("{at} key"))));
        w.line(&format!("if named[k] then R.fail({}, \"other entry \" .. k .. \" is a named member's key\") end", lua_str(&at)));
        w.line(&want(&r.val_ty, "x", &format!("{at} value")));
        w.dedent();
        w.line("end");
        w.dedent();
        w.line("end");
    }
    w.dedent();
    w.line("end");
    w.blank();

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_record_raw(w, cx, s, &t);
        emit_encode_decode(w, cx, &t);
        return;
    }

    // to_raw
    w.line(&format!("--- A {t} as an item, its members in schema order."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    w.line("local m = {}");
    for f in struct_fields(s, cx) {
        let m = format!("v.{}", lua_member_name(&f.name));
        let key = lua_str(&f.name);
        match &f.occurrence {
            Occurrence::Optional => w.line(&format!("if {m} ~= nil then m[#m + 1] = {{ {key}, {} }} end", to_raw(&f.ty, &m))),
            o if is_repeated(o) => {
                w.line("do");
                w.indent();
                w.line("local items = {}");
                w.line(&format!("for i, x in ipairs({m}) do items[i] = {} end", to_raw(&f.ty, "x")));
                w.line(&format!("m[#m + 1] = {{ {key}, R.array(items) }}"));
                w.dedent();
                w.line("end");
            }
            _ => w.line(&format!("m[#m + 1] = {{ {key}, {} }}", to_raw(&f.ty, &m))),
        }
    }
    if let Some(r) = &s.rest {
        let m = format!("v.{}", lua_member_name(&r.name));
        w.line(&format!("if {m} ~= nil then"));
        w.indent();
        w.line("local keys = {}");
        w.line(&format!("for k in pairs({m}) do keys[#keys + 1] = k end"));
        w.line("table.sort(keys)");
        w.line(&format!("for _, k in ipairs(keys) do m[#m + 1] = {{ k, {} }} end", to_raw(&r.val_ty, &format!("{m}[k]"))));
        w.dedent();
        w.line("end");
    }
    match s.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, R.map(m))", lua_int(tag.0 as i128))),
        None => w.line("return R.map(m)"),
    }
    w.dedent();
    w.line("end");
    w.blank();

    // from_raw
    w.line(&format!("--- Reads a {t} from an item, checking each member's type and constraints."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    let raw = match s.tagged { Some(tag) => format!("R.untag(raw, {}, {})", lua_int(tag.0 as i128), lua_str(&t)), None => "raw".into() };
    w.line(&format!("local m = R.as_map({raw}, {})", lua_str(&t)));
    w.line(&format!("local v = setmetatable({{}}, M.{t})"));
    // A map has only the members its schema gives it (RFC 8610 §3.5;
    // dgtlrift/cddlc#102), unless it has other entries.
    if s.rest.is_none() {
        let named: Vec<String> = s.fields.iter().map(|f| format!("[{}] = true", lua_str(&f.name))).collect();
        w.line(&format!(
            "for k in pairs(m) do if not ({{ {} }})[k] then R.fail({}, \"unknown member \" .. k) end end",
            named.join(", "),
            lua_str(&t)
        ));
    }
    for f in &s.fields {
        let m = format!("v.{}", lua_member_name(&f.name));
        let key = lua_str(&f.name);
        let at = format!("{t}.{}", f.name);
        match &f.occurrence {
            Occurrence::Optional => {
                // A present `nil` member is null; for any other type null means absent.
                let is_null = matches!(base_primitive(&f.ty), Some(Primitive::Null | Primitive::Undefined));
                let cond = if is_null { format!("m[{key}] ~= nil") } else { format!("R.present(m, {key})") };
                w.line(&format!("if {cond} then {m} = {} end", from_raw(&f.ty, &format!("m[{key}]"), &at)));
            }
            o if is_repeated(o) => {
                w.line(&format!("{m} = {{}}"));
                w.line(&format!("if R.present(m, {key}) then"));
                w.line(&format!("  for i, x in ipairs(R.as_array(m[{key}], {})) do {m}[i] = {} end", lua_str(&at), from_raw(&f.ty, "x", &at)));
                w.line("end");
            }
            _ => w.line(&format!("{m} = {}", from_raw(&f.ty, &format!("R.required(m, {key}, {})", lua_str(&t)), &at))),
        }
    }
    // Keys no member has are the map's other entries; none leaves the member nil.
    if let Some(r) = &s.rest {
        let named: Vec<String> = s.fields.iter().map(|f| format!("[{}] = true", lua_str(&f.name))).collect();
        let field = format!("v.{}", lua_member_name(&r.name));
        w.line(&format!("local named = {{ {} }}", named.join(", ")));
        w.line("for k, x in pairs(m) do");
        w.indent();
        w.line("if not named[k] then");
        w.indent();
        w.line(&format!("{field} = {field} or {{}}"));
        w.line(&format!("{field}[k] = {}", from_raw(&r.val_ty, "x", &format!("{t} value"))));
        w.dedent();
        w.line("end");
        w.dedent();
        w.line("end");
    }
    w.line(&format!("M.{t}.validate(v)"));
    w.line("return v");
    w.dedent();
    w.line("end");
    emit_encode_decode(w, cx, &t);
}

/// A record's `to_raw` / `from_raw` (RFC 8610 §3.4): an array of the members'
/// values, in order. Optional members come last: those present are written up
/// to the first absent one, and one set after an absent one is an error.
fn emit_record_raw(w: &mut IndentWriter, _cx: &Cx, s: &StructDef, t: &str) {
    w.line(&format!("--- A {t} as an array item, its members in order."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    w.line("local a = {}");
    let mut gap = false;
    for f in &s.fields {
        let m = format!("v.{}", lua_member_name(&f.name));
        if f.occurrence.is_required() {
            w.line(&format!("a[#a + 1] = {}", to_raw(&f.ty, &m)));
        } else {
            if !gap {
                w.line("local gap = false");
                gap = true;
            }
            w.line(&format!("if {m} ~= nil then"));
            w.indent();
            w.line(&format!(
                "if gap then R.fail({}, \"set, but an optional member before it isn't\") end",
                lua_str(&format!("{t}.{}", f.name))
            ));
            w.line(&format!("a[#a + 1] = {}", to_raw(&f.ty, &m)));
            w.dedent();
            w.line("else gap = true end");
        }
    }
    match s.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, R.array(a))", lua_int(tag.0 as i128))),
        None => w.line("return R.array(a)"),
    }
    w.dedent();
    w.line("end");
    w.blank();

    let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();
    w.line(&format!("--- Reads a {t} from an array item, checking each member's type and constraints."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    let raw = match s.tagged { Some(tag) => format!("R.untag(raw, {}, {})", lua_int(tag.0 as i128), lua_str(t)), None => "raw".into() };
    w.line(&format!("local a = R.as_array({raw}, {})", lua_str(t)));
    w.line(&format!("if #a < {required} or #a > {} then", s.fields.len()));
    w.indent();
    let want = if required == s.fields.len() { required.to_string() } else { format!("{required} to {}", s.fields.len()) };
    w.line(&format!("R.fail({}, \"expected {want} items, got \" .. #a)", lua_str(t)));
    w.dedent();
    w.line("end");
    w.line(&format!("local v = setmetatable({{}}, M.{t})"));
    for (i, f) in s.fields.iter().enumerate() {
        let m = format!("v.{}", lua_member_name(&f.name));
        let at = format!("{t}.{}", f.name);
        let item = format!("a[{}]", i + 1);
        if f.occurrence.is_required() {
            w.line(&format!("{m} = {}", from_raw(&f.ty, &item, &at)));
        } else {
            w.line(&format!("if {item} ~= nil then {m} = {} end", from_raw(&f.ty, &item, &at)));
        }
    }
    w.line(&format!("M.{t}.validate(v)"));
    w.line("return v");
    w.dedent();
    w.line("end");
}

// ── Enum ──────────────────────────────────────────────────────────────────────

/// The integer literal of an integer variant (the `2` in `1 / 2 / 3`).
pub fn int_literal(v: &EnumVariant) -> Option<i128> {
    match &v.ty {
        TypeRef::Primitive(Primitive::Uint | Primitive::Int) => v.constraints.iter().find_map(|c| match c {
            Constraint::Eq(LiteralValue::Uint(n)) => Some(*n as i128),
            Constraint::Eq(LiteralValue::Int(n)) => Some(*n as i128),
            _ => None,
        }),
        _ => None,
    }
}

/// Whether a variant carries a value (not a text, nil or integer literal).
pub fn has_payload(v: &EnumVariant) -> bool {
    if int_literal(v).is_some() {
        return false;
    }
    match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) => !v.constraints.iter().any(|c| matches!(c, Constraint::Eq(LiteralValue::Text(_)))),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => false,
        TypeRef::Primitive(_) | TypeRef::Named(_) => true,
        TypeRef::Tagged(..) | TypeRef::Choice(_) => false,
    }
}

/// The `kind` of a variant: its PascalCase name.
pub fn lua_variant_kind(v: &str) -> String {
    to_pascal_case(v)
}

/// Whether a variant is `null` / `nil` (or `undefined`).
fn is_nil(v: &EnumVariant) -> bool {
    matches!(&v.ty, TypeRef::Primitive(Primitive::Null | Primitive::Undefined))
}

fn emit_enum(w: &mut IndentWriter, cx: &Cx, e: &EnumDef) {
    let t = lua_type_name(&e.name);
    let at = lua_str(&t);
    doc(w, e.doc.as_deref(), &format!("One of the {} variants: {{ kind = ..., value = ... }}.", e.name));
    if e.deprecated { w.line("-- @deprecated"); }
    open_type(w, &t, true);

    for v in &e.variants {
        let k = lua_variant_kind(&v.name);
        if has_payload(v) {
            w.line(&format!("--- The {k} variant, holding value."));
            w.line(&format!("function M.{t}.{k}(value) return setmetatable({{ kind = {}, value = value }}, M.{t}) end", lua_str(&k)));
        } else {
            w.line(&format!("--- The {k} variant."));
            w.line(&format!("function M.{t}.{k}() return setmetatable({{ kind = {} }}, M.{t}) end", lua_str(&k)));
        }
    }
    w.blank();

    // validate
    w.line("--- Checks the variant's kind and value; raises an error if they don't fit.");
    w.line(&format!("function M.{t}.validate(v)"));
    w.indent();
    w.line(&format!("R.want_table(v, {at})"));
    for (i, v) in e.variants.iter().enumerate() {
        let k = lua_variant_kind(&v.name);
        w.line(&format!("{} v.kind == {} then", if i == 0 { "if" } else { "elseif" }, lua_str(&k)));
        if has_payload(v) {
            w.line(&format!("  {}", want(&v.ty, "v.value", &format!("{t}.{k}"))));
        }
    }
    w.line("else");
    w.line(&format!("  R.fail({at}, \"unknown kind \" .. tostring(v.kind))"));
    w.line("end");
    w.dedent();
    w.line("end");
    w.blank();

    // to_raw
    w.line(&format!("--- A {t} as an item: the variant's literal or value."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    w.line("local raw");
    for (i, v) in e.variants.iter().enumerate() {
        let k = lua_variant_kind(&v.name);
        let raw = if let Some(n) = int_literal(v) {
            if n < 0 { format!("R.int({})", lua_int(n)) } else { format!("R.uint({})", lua_int(n)) }
        } else if has_payload(v) {
            to_raw(&v.ty, "v.value")
        } else if is_nil(v) {
            // `null` is CBOR / JSON null, not text (dgtlrift/cddlc#147).
            "R.null()".to_owned()
        } else {
            // A text literal is written as its text.
            format!("R.text({})", lua_str(&cddlc_codegen::enum_variant_text(v)))
        };
        w.line(&format!("{} v.kind == {} then raw = {raw}", if i == 0 { "if" } else { "elseif" }, lua_str(&k)));
    }
    if !e.variants.is_empty() { w.line("end"); }
    match e.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, raw)", lua_int(tag.0 as i128))),
        None => w.line("return raw"),
    }
    w.dedent();
    w.line("end");
    w.blank();

    // from_raw: literals first, then the variant whose kind the item has.
    w.line(&format!("--- Reads a {t} from an item: literals first, then by the item's kind."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    match e.tagged {
        Some(tag) => w.line(&format!("local item = R.untag(raw, {}, {at})", lua_int(tag.0 as i128))),
        None => w.line("local item = raw"),
    }
    let payload = |f: &dyn Fn(&Primitive) -> bool| {
        e.variants.iter().find(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(p) if f(p)))
    };
    let ctor = |v: &EnumVariant, arg: Option<&str>| format!("M.{t}.{}({})", lua_variant_kind(&v.name), arg.unwrap_or(""));
    let text_lits: Vec<&EnumVariant> = e.variants.iter().filter(|v| !has_payload(v) && int_literal(v).is_none() && !is_nil(v)).collect();
    if let Some(v) = e.variants.iter().find(|v| is_nil(v)) {
        w.line(&format!("if item.k == \"null\" then return {} end", ctor(v, None)));
    }
    let tstr = payload(&|p| matches!(p, Primitive::Tstr));
    let uint = payload(&|p| matches!(p, Primitive::Uint));
    let int = payload(&|p| matches!(p, Primitive::Int));
    let float = payload(&|p| is_float(p));
    let boolean = payload(&|p| matches!(p, Primitive::Bool));
    let any_bytes = payload(&|p| matches!(p, Primitive::Bstr | Primitive::Any));
    let bytes = any_bytes.filter(|_| !cx.json());
    // JSON writes bytes as base64url text: read them as text, unless a tstr
    // variant takes any text (the two can't be told apart).
    let json_bytes = any_bytes.filter(|_| cx.json() && tstr.is_none());
    if !text_lits.is_empty() || tstr.is_some() || json_bytes.is_some() {
        w.line("if item.k == \"text\" then");
        w.indent();
        for v in &text_lits {
            w.line(&format!("if item.v == {} then return {} end", lua_str(&cddlc_codegen::enum_variant_text(v)), ctor(v, None)));
        }
        if let Some(v) = tstr {
            w.line(&format!("return {}", ctor(v, Some("item.v"))));
        } else if let Some(v) = json_bytes {
            w.line(&format!("local ok, b = pcall(R.as_bytes, item, {at})"));
            w.line(&format!("if ok then return {} end", ctor(v, Some("b"))));
        }
        w.dedent();
        w.line("end");
    }
    let pos_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n >= 0).collect();
    let neg_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n < 0).collect();
    if !pos_lits.is_empty() || uint.is_some() || int.is_some() || float.is_some() {
        w.line("if item.k == \"uint\" then");
        w.indent();
        for (v, n) in &pos_lits {
            w.line(&format!("if item.v == {} then return {} end", lua_int(*n), ctor(v, None)));
        }
        if let Some(v) = uint {
            w.line(&format!("return {}", ctor(v, Some("item.v"))));
        } else if let Some(v) = int {
            w.line(&format!("if item.v >= 0 then return {} end", ctor(v, Some("item.v"))));
        } else if let Some(v) = float {
            w.line(&format!("return {}", ctor(v, Some(&format!("R.as_float(item, {at})")))));
        }
        w.dedent();
        w.line("end");
    }
    if !neg_lits.is_empty() || int.is_some() || float.is_some() {
        w.line("if item.k == \"nint\" then");
        w.indent();
        for (v, n) in &neg_lits {
            w.line(&format!("if item.v == {} then return {} end", lua_int(*n), ctor(v, None)));
        }
        if let Some(v) = int {
            w.line(&format!("return {}", ctor(v, Some("item.v"))));
        } else if let Some(v) = float {
            w.line(&format!("return {}", ctor(v, Some("item.v + 0.0"))));
        }
        w.dedent();
        w.line("end");
    }
    if let Some(v) = float {
        w.line(&format!("if item.k == \"float\" then return {} end", ctor(v, Some("item.v"))));
    }
    if let Some(v) = boolean {
        w.line(&format!("if item.k == \"bool\" then return {} end", ctor(v, Some("item.v"))));
    }
    if let Some(v) = bytes {
        w.line(&format!("if item.k == \"bytes\" then return {} end", ctor(v, Some("item.v"))));
    }
    for v in e.variants.iter().filter(|v| has_payload(v) && matches!(&v.ty, TypeRef::Named(_))) {
        let TypeRef::Named(n) = &v.ty else { continue };
        w.line("do");
        w.line(&format!("  local ok, x = pcall(M.{}.from_raw, item)", lua_type_name(n)));
        w.line(&format!("  if ok then return {} end", ctor(v, Some("x"))));
        w.line("end");
    }
    w.line(&format!("R.fail({at}, \"no variant matches \" .. R.kind_name(item))"));
    w.dedent();
    w.line("end");
    emit_encode_decode(w, cx, &t);
}

// ── Array ─────────────────────────────────────────────────────────────────────

fn emit_array(w: &mut IndentWriter, cx: &Cx, a: &ArrayDef) {
    let t = lua_type_name(&a.name);
    doc(w, a.doc.as_deref(), &format!("The {} array: a sequence of its items.", a.name));
    if a.deprecated { w.line("-- @deprecated"); }
    open_type(w, &t, true);
    w.line(&format!("--- Makes a {t} from a sequence of items."));
    w.line(&format!("function M.{t}.new(items) return setmetatable(items or {{}}, M.{t}) end"));
    w.blank();
    w.line("--- Checks every item's type; raises an error if one doesn't fit.");
    w.line(&format!("function M.{t}.validate(v)"));
    w.line(&format!("  R.want_table(v, {})", lua_str(&t)));
    w.line(&format!("  for _, x in ipairs(v) do {} end", want(&a.element, "x", &format!("{t} item"))));
    w.line("end");
    w.blank();
    w.line(&format!("--- A {t} as an array item."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    w.line("local items = {}");
    // A repeated group: each record's members, inline (dgtlrift/cddlc#90).
    let flat = match (&a.element, a.flat) {
        (TypeRef::Named(n), true) => match cx.module.get(n) {
            Some(TypeDef::Struct(r)) => Some(r.fields.len()),
            _ => None,
        },
        _ => None,
    };
    if flat.is_some() {
        w.line(&format!(
            "for _, x in ipairs(v) do for _, m in ipairs(R.as_array({}, {})) do items[#items + 1] = m end end",
            to_raw(&a.element, "x"), lua_str(&t)
        ));
    } else {
        w.line(&format!("for i, x in ipairs(v) do items[i] = {} end", to_raw(&a.element, "x")));
    }
    match a.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, R.array(items))", lua_int(tag.0 as i128))),
        None => w.line("return R.array(items)"),
    }
    w.dedent();
    w.line("end");
    w.blank();
    w.line(&format!("--- Reads a {t} from an array item, checking each item's kind."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    let raw = match a.tagged { Some(tag) => format!("R.untag(raw, {}, {})", lua_int(tag.0 as i128), lua_str(&t)), None => "raw".into() };
    w.line(&format!("local v = setmetatable({{}}, M.{t})"));
    if let Some(n) = flat {
        w.line(&format!("local all = R.as_array({raw}, {})", lua_str(&t)));
        w.line(&format!("if #all % {n} ~= 0 then R.fail({}, \"a partial group\") end", lua_str(&t)));
        w.line(&format!("for i = 1, #all, {n} do"));
        w.line("  local g = {}");
        w.line(&format!("  for j = 1, {n} do g[j] = all[i + j - 1] end"));
        w.line(&format!("  v[#v + 1] = {}", from_raw(&a.element, "R.array(g)", &format!("{t} item"))));
        w.line("end");
    } else {
        w.line(&format!("for i, x in ipairs(R.as_array({raw}, {})) do v[i] = {} end", lua_str(&t), from_raw(&a.element, "x", &format!("{t} item"))));
    }
    w.line("return v");
    w.dedent();
    w.line("end");
    emit_encode_decode(w, cx, &t);
}

// ── Table ─────────────────────────────────────────────────────────────────────

/// A table (`{ * tstr => uint }`): a Lua table from key to value, written in
/// key order (dgtlrift/cddlc#82). Its keys are text, as the runtime's map
/// items have text keys.
fn emit_table(w: &mut IndentWriter, cx: &Cx, m: &MapDef) {
    let t = lua_type_name(&m.name);
    doc(w, m.doc.as_deref(), &format!("The {} table: a Lua table from key to value.", m.name));
    if m.deprecated { w.line("-- @deprecated"); }
    open_type(w, &t, true);
    w.line(&format!("--- Makes a {t} from a table of entries."));
    w.line(&format!("function M.{t}.new(entries) return setmetatable(entries or {{}}, M.{t}) end"));
    w.blank();
    w.line("--- Checks every entry's types; raises an error if one doesn't fit.");
    w.line(&format!("function M.{t}.validate(v)"));
    w.line(&format!("  R.want_table(v, {})", lua_str(&t)));
    w.line(&format!(
        "  for k, x in pairs(v) do {}; {} end",
        want(&m.key_ty, "k", &format!("{t} key")),
        want(&m.val_ty, "x", &format!("{t} value"))
    ));
    w.line("end");
    w.blank();
    w.line(&format!("--- A {t} as a map item, its entries in key order."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    w.line("local keys = {}");
    w.line("for k in pairs(v) do keys[#keys + 1] = k end");
    w.line("table.sort(keys)");
    w.line("local entries = {}");
    w.line(&format!("for i, k in ipairs(keys) do entries[i] = {{ k, {} }} end", to_raw(&m.val_ty, "v[k]")));
    match m.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, R.map(entries))", lua_int(tag.0 as i128))),
        None => w.line("return R.map(entries)"),
    }
    w.dedent();
    w.line("end");
    w.blank();
    w.line(&format!("--- Reads a {t} from a map item, checking each entry's kinds."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    let raw = match m.tagged { Some(tag) => format!("R.untag(raw, {}, {})", lua_int(tag.0 as i128), lua_str(&t)), None => "raw".into() };
    w.line(&format!("local v = setmetatable({{}}, M.{t})"));
    w.line(&format!(
        "for k, x in pairs(R.as_map({raw}, {})) do v[k] = {} end",
        lua_str(&t),
        from_raw(&m.val_ty, "x", &format!("{t} value"))
    ));
    w.line("return v");
    w.dedent();
    w.line("end");
    emit_encode_decode(w, cx, &t);
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, cx: &Cx, a: &AliasDef) {
    let t = lua_type_name(&a.name);
    doc(w, a.doc.as_deref(), &format!("A {} value (a plain Lua value).", a.name));
    if a.deprecated { w.line("-- @deprecated"); }
    open_type(w, &t, false);
    w.line("--- Checks the value's type and CDDL constraints; raises an error if one fails.");
    w.line(&format!("function M.{t}.validate(v)"));
    w.indent();
    w.line(&format!("if v == nil then R.fail({}, \"missing\") end", lua_str(&t)));
    w.line(&want(&a.ty, "v", &t));
    emit_checks(w, &a.ty, &a.constraints, "v", &t);
    w.dedent();
    w.line("end");
    w.blank();
    w.line(&format!("--- A {t} as an item."));
    w.line(&format!("function M.{t}.to_raw(v)"));
    w.indent();
    w.line(&format!("M.{t}.validate(v)"));
    let raw = to_raw(&a.ty, "v");
    match a.tagged {
        Some(tag) => w.line(&format!("return R.tag({}, {raw})", lua_int(tag.0 as i128))),
        None => w.line(&format!("return {raw}")),
    }
    w.dedent();
    w.line("end");
    w.blank();
    w.line(&format!("--- Reads a {t} from an item, checking its kind and constraints."));
    w.line(&format!("function M.{t}.from_raw(raw)"));
    w.indent();
    let raw = match a.tagged { Some(tag) => format!("R.untag(raw, {}, {})", lua_int(tag.0 as i128), lua_str(&t)), None => "raw".into() };
    w.line(&format!("local v = {}", from_raw(&a.ty, &raw, &t)));
    w.line(&format!("M.{t}.validate(v)"));
    w.line("return v");
    w.dedent();
    w.line("end");
    emit_encode_decode(w, cx, &t);
}

// ── Specs ─────────────────────────────────────────────────────────────────────

fn emit_spec(cx: &Cx) -> String {
    let mut w = IndentWriter::new(2);
    header(&mut w, cx);
    w.line("-- Roundtrip specs: run `busted` in this directory.");
    w.blank();
    w.line(&format!("local M = require({})", lua_str(cx.modname)));
    w.blank();
    for (_, def) in &cx.module.types {
        let tests: Vec<(String, Vec<String>)> = match def {
            TypeDef::Struct(s) => struct_specs(cx, s),
            TypeDef::Enum(e) => enum_specs(cx, e),
            TypeDef::Alias(a) => alias_specs(cx, a),
            TypeDef::Array(a) => array_specs(cx, a),
            TypeDef::Map(m) => table_specs(cx, m),
        };
        if tests.is_empty() { continue; }
        w.line(&format!("describe({}, function()", lua_str(&lua_type_name(def.name()))));
        w.indent();
        for (i, (name, body)) in tests.iter().enumerate() {
            if i > 0 { w.blank(); }
            w.line(&format!("it({}, function()", lua_str(name)));
            w.indent();
            for l in body { w.line(l); }
            w.dedent();
            w.line("end)");
        }
        w.dedent();
        w.line("end)");
        w.blank();
    }
    format!("{}\n", w.finish().trim_end())
}

fn roundtrip(t: &str, value: &str) -> Vec<String> {
    vec![
        format!("local original = {value}"),
        format!("assert.same(original, M.{t}.decode(M.{t}.encode(original)))"),
    ]
}

fn struct_specs(cx: &Cx, s: &StructDef) -> Vec<(String, Vec<String>)> {
    let t = lua_type_name(&s.name);
    let mut out = vec![
        ("roundtrips default values".into(), roundtrip(&t, &lua_struct_value(cx, s, false, 0))),
        ("roundtrips explicit values".into(), roundtrip(&t, &lua_struct_value(cx, s, true, 0))),
    ];
    if s.tagged.is_some() && !cx.json() {
        out.push(("encodes its CBOR tag".into(), vec![
            format!("local data = M.{t}.encode({})", lua_struct_value(cx, s, false, 0)),
            "assert.equal(6, data:byte(1) >> 5) -- CBOR major type 6: tag".into(),
        ]));
    }
    out
}

fn enum_specs(cx: &Cx, e: &EnumDef) -> Vec<(String, Vec<String>)> {
    let t = lua_type_name(&e.name);
    let takes_text = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
    let mut out = Vec::new();
    for v in &e.variants {
        // In JSON a byte string is text, which a tstr variant reads first.
        if cx.json() && takes_text && has_payload(v) && matches!(base_primitive(&v.ty), Some(Primitive::Bstr | Primitive::Any)) { continue; }
        let mut body = roundtrip(&t, &lua_variant_value(cx, e, v, 0));
        body.push(format!("assert.equal({}, original.kind)", lua_str(&lua_variant_kind(&v.name))));
        out.push((format!("roundtrips the {} variant", lua_variant_kind(&v.name)), body));
    }
    let lits: Vec<i128> = e.variants.iter().filter_map(int_literal).collect();
    let takes_number = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty,
        TypeRef::Primitive(p) if matches!(p, Primitive::Uint | Primitive::Int) || is_float(p)));
    if !lits.is_empty() && !takes_number && e.tagged.is_none() {
        if let Some(other) = (0i128..24).find(|k| !lits.contains(k)) {
            // A CBOR unsigned integer below 24 is the one byte itself.
            let data = if cx.json() { lua_str(&other.to_string()) } else { format!("string.char({other})") };
            out.push(("rejects an integer no variant names".into(), vec![
                format!("assert.has_error(function() M.{t}.decode({data}) end)"),
            ]));
        }
    }
    out
}

fn alias_specs(cx: &Cx, a: &AliasDef) -> Vec<(String, Vec<String>)> {
    let t = lua_type_name(&a.name);
    let mut out = vec![("roundtrips a valid value".into(), roundtrip(&t, &lua_value(cx, &a.ty, &Occurrence::Required, &a.constraints, false, 1)))];
    if let Some(bad) = lua_invalid_value(&a.constraints, &a.ty) {
        out.push(("rejects a value outside its constraints".into(), vec![
            format!("assert.has_error(function() M.{t}.encode({bad}) end)"),
        ]));
    }
    out
}

fn array_specs(cx: &Cx, a: &ArrayDef) -> Vec<(String, Vec<String>)> {
    let t = lua_type_name(&a.name);
    let item = lua_value(cx, &a.element, &Occurrence::Required, &[], true, 1);
    vec![
        ("roundtrips an empty array".into(), roundtrip(&t, &format!("M.{t}.new({{}})"))),
        ("roundtrips items".into(), roundtrip(&t, &format!("M.{t}.new({{ {item}, {item} }})"))),
    ]
}

fn table_specs(cx: &Cx, m: &MapDef) -> Vec<(String, Vec<String>)> {
    let t = lua_type_name(&m.name);
    vec![
        ("roundtrips an empty table".into(), roundtrip(&t, &format!("M.{t}.new({{}})"))),
        ("roundtrips entries".into(), roundtrip(&t, &lua_table_value(cx, m, true, 1))),
    ]
}

/// A table with one entry.
fn lua_table_value(cx: &Cx, m: &MapDef, explicit: bool, depth: usize) -> String {
    let key = lua_value(cx, &m.key_ty, &Occurrence::Required, &[], explicit, depth + 1);
    let val = lua_value(cx, &m.val_ty, &Occurrence::Required, &[], explicit, depth + 1);
    format!("M.{}.new({{ [{key}] = {val} }})", lua_type_name(&m.name))
}

// ── Test values ───────────────────────────────────────────────────────────────

/// Nesting depth past which a named type's test value is not built further.
const MAX_FIXTURE_DEPTH: usize = 4;

fn lua_struct_value(cx: &Cx, s: &StructDef, explicit: bool, depth: usize) -> String {
    let mut members: Vec<String> = s.fields.iter().filter_map(|f| {
        let v = lua_value(cx, &f.ty, &f.occurrence, &f.constraints, explicit, depth + 1);
        (v != "nil").then(|| format!("{} = {v}", lua_member_name(&f.name)))
    }).collect();
    // One other entry when explicit or when the map needs one.
    if let Some(r) = &s.rest {
        if explicit || matches!(r.occurrence, Occurrence::OneOrMore { .. }) {
            let v = lua_value(cx, &r.val_ty, &Occurrence::Required, &[], explicit, depth + 1);
            members.push(format!("{} = {{ [\"other-key\"] = {v} }}", lua_member_name(&r.name)));
        }
    }
    if members.is_empty() {
        format!("M.{}.new({{}})", lua_type_name(&s.name))
    } else {
        format!("M.{}.new({{ {} }})", lua_type_name(&s.name), members.join(", "))
    }
}

fn lua_variant_value(cx: &Cx, e: &EnumDef, v: &EnumVariant, depth: usize) -> String {
    let t = lua_type_name(&e.name);
    let k = lua_variant_kind(&v.name);
    if !has_payload(v) {
        return format!("M.{t}.{k}()");
    }
    format!("M.{t}.{k}({})", lua_value(cx, &v.ty, &Occurrence::Required, &[], true, depth + 1))
}

/// A valid test value: the literal a type is pinned to, a value inside its
/// constraints, or a default/explicit value.
fn lua_value(cx: &Cx, ty: &TypeRef, occ: &Occurrence, constraints: &[Constraint], explicit: bool, depth: usize) -> String {
    match occ {
        Occurrence::Optional if !explicit => return "nil".into(),
        Occurrence::ZeroOrMore { .. } | Occurrence::Bounded { .. } => return "{}".into(),
        Occurrence::OneOrMore { .. } => return format!("{{ {} }}", lua_value(cx, ty, &Occurrence::Required, constraints, explicit, depth)),
        _ => {}
    }
    if let Some(p) = base_primitive(ty) {
        if let Some(l) = constraints.iter().find_map(|c| match c {
            Constraint::Eq(v) if literal_fits(v, p) => lua_literal(v).map(|l| if is_float(p) && !l.contains('.') { format!("{l}.0") } else { l }),
            _ => None,
        }) {
            return l;
        }
        for c in constraints {
            match (c, p) {
                // `.bits`: every allowed bit set (dgtlrift/cddlc#107).
                (Constraint::Bits(b), Primitive::Uint) => return lua_int(cddlc_codegen::bits_mask_u64(b) as i128),
                (Constraint::Bits(b), Primitive::Bstr) => return lua_bytes(&cddlc_codegen::bits_mask_bytes(b)),
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Tstr) => return format!("string.rep(\"a\", {n})"),
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Bstr | Primitive::Any) => return format!("string.rep(\"\\0\", {n})"),
                (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) => return lua_int(*lo as i128),
                (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) => return lua_int(*lo as i128),
                // Only an upper bound: the largest value under it.
                (Constraint::ValueRangeInt { min: None, max: Some(hi), inclusive }, Primitive::Int) => {
                    return lua_int(if *inclusive { *hi as i128 } else { *hi as i128 - 1 });
                }
                _ => {}
            }
        }
    }
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => (if explicit { "42" } else { "0" }).into(),
            Primitive::Int => (if explicit { "-1" } else { "0" }).into(),
            Primitive::Bool => explicit.to_string(),
            // 1.5 is exact as a float32, so it reads back unchanged from any decoder.
            Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float => (if explicit { "1.5" } else { "0.0" }).into(),
            Primitive::Tstr => (if explicit { "\"hello\"" } else { "\"\"" }).into(),
            Primitive::Bstr | Primitive::Any => (if explicit { "\"\\1\\2\"" } else { "\"\"" }).into(),
            Primitive::Null | Primitive::Undefined => "M.null".into(),
        },
        TypeRef::Tagged(_, inner) => lua_value(cx, inner, &Occurrence::Required, constraints, explicit, depth),
        // A choice's first alternative (null isn't a value of it).
        TypeRef::Choice(cs) => match cs.first() {
            Some(first) => lua_value(cx, first, &Occurrence::Required, &[], explicit, depth),
            None => "M.null".into(),
        },
        TypeRef::Named(n) => {
            let t = lua_type_name(n);
            match cx.module.get(n) {
                _ if depth > MAX_FIXTURE_DEPTH => "nil".into(),
                Some(TypeDef::Struct(s)) => lua_struct_value(cx, s, explicit, depth),
                Some(TypeDef::Alias(a)) => lua_value(cx, &a.ty, &Occurrence::Required, &a.constraints, explicit, depth + 1),
                Some(TypeDef::Enum(e)) => e.variants.first().map(|v| lua_variant_value(cx, e, v, depth)).unwrap_or_else(|| "nil".into()),
                Some(TypeDef::Array(_)) => format!("M.{t}.new({{}})"),
                Some(TypeDef::Map(m)) => lua_table_value(cx, m, explicit, depth),
                None => "nil".into(),
            }
        }
    }
}

/// A value just outside an alias's constraints, if there is a simple one.
fn lua_invalid_value(constraints: &[Constraint], ty: &TypeRef) -> Option<String> {
    let p = base_primitive(ty)?;
    for c in constraints {
        match (c, p) {
            (Constraint::SizeExact(n), Primitive::Tstr) => return Some(format!("string.rep(\"a\", {})", n + 1)),
            (Constraint::SizeExact(n), Primitive::Bstr | Primitive::Any) => return Some(format!("string.rep(\"\\0\", {})", n + 1)),
            (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) if *lo > i64::MIN => return Some(lua_int(*lo as i128 - 1)),
            (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) if *lo > 0 => return Some(lua_int(*lo as i128 - 1)),
            (Constraint::ValueRangeUint { max: Some(hi), .. }, Primitive::Uint) if *hi < u64::MAX => return Some(lua_int(*hi as i128 + 1)),
            // `.bits`: the lowest bit it doesn't allow.
            (Constraint::Bits(b), Primitive::Uint | Primitive::Bstr) => {
                let Some(bad) = (0u64..64).find(|n| !b.contains(n)) else { continue };
                if *p == Primitive::Uint {
                    return Some(lua_int((1u64 << bad) as i128));
                }
                let mut bytes = vec![0u8; bad as usize / 8 + 1];
                bytes[bad as usize / 8] = 1 << (bad % 8);
                return Some(lua_bytes(&bytes));
            }
            _ => {}
        }
    }
    None
}
