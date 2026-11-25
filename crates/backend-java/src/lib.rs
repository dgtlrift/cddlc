/// Java backend for cddlc: Java SE, or Jakarta EE with `--java-flavor ee`.
///
/// Emits a Maven project:
///   <module>-cbor/
///     pom.xml
///     src/main/java/<pkg>/Cddl.java          ← value model + codec
///     src/main/java/<pkg>/<Type>.java        ← one file per type
///     src/test/java/<pkg>/RoundtripTest.java ← JUnit roundtrip tests
///
/// Structs, arrays and aliases are records; an enum is a Java `enum` when all
/// its variants are literals, otherwise a sealed interface with a record per
/// variant. Every type converts to a format-neutral value (`toRaw` /
/// `fromRaw`), so nested types, tags and JSON's base64url bytes are handled in
/// one place, and the `Cddl` class does the I/O: Jackson for CBOR, Jackson
/// (SE) or Jakarta JSON-P (EE) for JSON.
///
/// Jakarta EE output also carries Bean Validation annotations for the CDDL
/// constraints, and, for JSON, JSON-B serializers that write the same JSON as
/// `encode()`.

mod runtime;

use cddlc_codegen::{
    to_pascal_case, Backend, CodegenError, CodegenOptions, CodegenOutput, GeneratedFile,
    IndentWriter, JavaFlavor,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, FieldDef, IrModule, LiteralValue,
    MapDef, Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

/// Library versions the generated `pom.xml` pins.
pub mod versions {
    pub const JACKSON: &str = "2.22.4";
    pub const JUNIT: &str = "6.1.3";
    pub const COMPILER_PLUGIN: &str = "3.16.0";
    pub const SUREFIRE_PLUGIN: &str = "3.6.1";
    pub const JAKARTA_VALIDATION: &str = "3.1.1";
    pub const JAKARTA_JSONB: &str = "3.0.4";
    pub const JAKARTA_JSONP: &str = "2.1.3";
    pub const HIBERNATE_VALIDATOR: &str = "9.1.6.Final";
    pub const EXPRESSLY: &str = "6.0.0";
    pub const YASSON: &str = "3.0.5";
    pub const PARSSON: &str = "1.1.9";
}

pub struct JavaBackend;

impl Backend for JavaBackend {
    fn language_name(&self) -> &str { "Java" }
    fn file_extension(&self) -> &str { "java" }

    fn generate(&self, module: &IrModule, opts: &CodegenOptions) -> Result<CodegenOutput, CodegenError> {
        let name = module.name.as_str();
        let dir = format!("{name}-{}", if opts.is_json() { "json" } else { "cbor" });
        let pkg = java_package_name(name, opts.namespace.as_deref());
        let pkg_dir = pkg.replace('.', "/");
        let cx = Cx { module, opts, pkg: &pkg };

        let mut files = vec![
            GeneratedFile { path: format!("{dir}/pom.xml"), content: emit_pom(&dir, opts) },
            GeneratedFile { path: format!("{dir}/src/main/java/{pkg_dir}/Cddl.java"), content: emit_runtime(&cx) },
        ];
        for (_, def) in &module.types {
            let class = java_type_name(def.name());
            files.push(GeneratedFile {
                path: format!("{dir}/src/main/java/{pkg_dir}/{class}.java"),
                content: emit_type_file(&cx, def),
            });
        }
        files.push(GeneratedFile {
            path: format!("{dir}/src/test/java/{pkg_dir}/RoundtripTest.java"),
            content: emit_tests(&cx),
        });
        Ok(CodegenOutput { files })
    }
}

/// What every emitter needs.
struct Cx<'a> {
    module: &'a IrModule,
    opts:   &'a CodegenOptions,
    pkg:    &'a str,
}

impl Cx<'_> {
    fn ee(&self) -> bool { matches!(self.opts.java_flavor, JavaFlavor::Ee) }
    fn json(&self) -> bool { self.opts.is_json() }
    /// JSON-B serializers: Jakarta EE JSON output.
    fn jsonb(&self) -> bool { self.ee() && self.json() }
    fn format(&self) -> &'static str { if self.json() { "JSON" } else { "CBOR" } }
}

// ── Names ─────────────────────────────────────────────────────────────────────

const JAVA_KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float",
    "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native",
    "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp",
    "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void",
    "volatile", "while", "true", "false", "null", "var", "yield", "record", "sealed", "permits",
    "non-sealed", "exports", "module", "open", "opens", "provides", "requires", "to", "transitive",
    "uses", "with",
];

/// The Java package: `--namespace` when given, else the module name, each part
/// a lowercase identifier (`simple-values` → `simple_values`).
pub fn java_package_name(module: &str, namespace: Option<&str>) -> String {
    let src = namespace.unwrap_or(module);
    src.split('.')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut s: String = p.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' })
                .collect();
            if s.starts_with(|c: char| c.is_ascii_digit()) { s.insert(0, '_'); }
            if JAVA_KEYWORDS.contains(&s.as_str()) { s.push('_'); }
            s
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Class names that would hide a `java.lang` or `java.util` type the generated
/// code uses, or the runtime class.
const RESERVED_TYPES: &[&str] = &[
    "Cddl", "Object", "String", "Long", "Integer", "Short", "Byte", "Float", "Double", "Boolean",
    "Character", "Number", "Math", "System", "Record", "Enum", "Override", "Deprecated", "Class",
    "Void", "Iterable", "Exception", "RuntimeException", "IllegalArgumentException", "List", "Map",
    "ArrayList", "LinkedHashMap", "Arrays", "Objects", "Test", "RoundtripTest", "InteropTest",
];

/// The Java class name of a CDDL rule.
pub fn java_type_name(name: &str) -> String {
    let n = to_pascal_case(name);
    if RESERVED_TYPES.contains(&n.as_str()) { format!("{n}_") } else { n }
}

/// Member names that would clash with a generated method.
const RESERVED_MEMBERS: &[&str] = &[
    "encode", "decode", "validate", "toRaw", "fromRaw", "kind", "equals", "hashCode", "toString",
    "getClass", "notify", "notifyAll", "wait", "clone", "finalize", "that",
];

/// The Java name of a struct member (record component): camelCase.
pub fn java_member_name(name: &str) -> String {
    let p = to_pascal_case(name);
    let mut s: String = p.chars().take(1).flat_map(|c| c.to_lowercase()).chain(p.chars().skip(1)).collect();
    if s.is_empty() { s = "member".into(); }
    if JAVA_KEYWORDS.contains(&s.as_str()) || RESERVED_MEMBERS.contains(&s.as_str()) { s.push('_'); }
    s
}

/// The constant of a literal-only enum variant (`Ok` → `OK`).
pub fn java_enum_constant(variant: &str) -> String {
    cddlc_codegen::to_screaming_snake(&to_pascal_case(variant))
}

/// The record of a valued enum's variant, renamed if it would clash with the
/// enum or a reserved name.
pub fn java_variant_record(enum_name: &str, variant: &str) -> String {
    let n = java_type_name(variant);
    if n == java_type_name(enum_name) { format!("{n}Variant") } else { n }
}

fn camel(s: &str) -> String {
    let p = to_pascal_case(s);
    p.chars().take(1).flat_map(|c| c.to_lowercase()).chain(p.chars().skip(1)).collect()
}

/// A Java string literal.
pub fn java_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn java_bytes(b: &[u8]) -> String {
    if b.is_empty() { return "new byte[0]".into(); }
    format!("new byte[] {{{}}}", b.iter().map(|x| format!("(byte) 0x{x:02x}")).collect::<Vec<_>>().join(", "))
}

fn long_lit(n: i128) -> String {
    if n > i64::MAX as i128 { format!("0x{n:x}L") } else { format!("{n}L") }
}

fn header(w: &mut IndentWriter, cx: &Cx) {
    for l in cddlc_codegen::header_comment(cx.opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line(&format!("// Format: {}; flavor: {}", if cx.json() { "JSON (RFC 8259)" } else { "CBOR (RFC 8949)" },
        if cx.ee() { "Jakarta EE" } else { "Java SE" }));
    if cx.opts.dcbor { w.line("// Encoding: deterministic (dCBOR)"); }
    w.blank();
    w.line(&format!("package {};", cx.pkg));
    w.blank();
}

fn emit_javadoc(w: &mut IndentWriter, doc: Option<&str>, fallback: &str) {
    w.line("/**");
    match doc {
        Some(d) if !d.trim().is_empty() => {
            for l in d.lines() {
                let l = l.replace("*/", "*&#47;");
                if l.trim().is_empty() { w.line(" *"); } else { w.line(&format!(" * {l}")); }
            }
        }
        _ => w.line(&format!(" * {fallback}")),
    }
    w.line(" */");
}

// ── pom.xml ───────────────────────────────────────────────────────────────────

fn dep(groups: &mut String, g: &str, a: &str, v: &str, scope: Option<&str>) {
    groups.push_str(&format!("    <dependency>\n      <groupId>{g}</groupId>\n      <artifactId>{a}</artifactId>\n      <version>{v}</version>\n"));
    if let Some(s) = scope { groups.push_str(&format!("      <scope>{s}</scope>\n")); }
    groups.push_str("    </dependency>\n");
}

fn emit_pom(dir: &str, opts: &CodegenOptions) -> String {
    use versions::*;
    let ee = matches!(opts.java_flavor, JavaFlavor::Ee);
    let mut deps = String::new();
    if opts.is_json() {
        if ee {
            dep(&mut deps, "jakarta.json", "jakarta.json-api", JAKARTA_JSONP, Some("provided"));
            dep(&mut deps, "jakarta.json.bind", "jakarta.json.bind-api", JAKARTA_JSONB, Some("provided"));
        } else {
            dep(&mut deps, "com.fasterxml.jackson.core", "jackson-core", JACKSON, None);
        }
    } else {
        dep(&mut deps, "com.fasterxml.jackson.dataformat", "jackson-dataformat-cbor", JACKSON, None);
    }
    if ee {
        dep(&mut deps, "jakarta.validation", "jakarta.validation-api", JAKARTA_VALIDATION, Some("provided"));
        // What an application server provides, for the tests.
        dep(&mut deps, "org.hibernate.validator", "hibernate-validator", HIBERNATE_VALIDATOR, Some("test"));
        dep(&mut deps, "org.glassfish.expressly", "expressly", EXPRESSLY, Some("test"));
        if opts.is_json() {
            dep(&mut deps, "org.eclipse", "yasson", YASSON, Some("test"));
            dep(&mut deps, "org.eclipse.parsson", "parsson", PARSSON, Some("test"));
        }
    }
    dep(&mut deps, "org.junit.jupiter", "junit-jupiter", JUNIT, Some("test"));

    let mut head = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    head.push_str(&cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Xml));
    format!(r#"{head}<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>

  <groupId>cddlc.generated</groupId>
  <artifactId>{dir}</artifactId>
  <version>0.1.0</version>
  <packaging>jar</packaging>
  <description>@generated by cddlc ({flavor})</description>

  <properties>
    <maven.compiler.release>17</maven.compiler.release>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>

  <dependencies>
{deps}  </dependencies>

  <build>
    <plugins>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-compiler-plugin</artifactId>
        <version>{COMPILER_PLUGIN}</version>
      </plugin>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-surefire-plugin</artifactId>
        <version>{SUREFIRE_PLUGIN}</version>
      </plugin>
    </plugins>
  </build>
</project>
"#, flavor = if ee { "Jakarta EE" } else { "Java SE" })
}

// ── Runtime class ─────────────────────────────────────────────────────────────

fn emit_runtime(cx: &Cx) -> String {
    let mut w = IndentWriter::new(4);
    header(&mut w, cx);
    let (codec, imports): (String, &[&str]) = if !cx.json() {
        (runtime::CBOR_JACKSON.to_owned(), runtime::CBOR_JACKSON_IMPORTS)
    } else if cx.ee() {
        (runtime::json_jsonp(), runtime::JSON_JSONP_IMPORTS)
    } else {
        (runtime::json_jackson(), runtime::JSON_JACKSON_IMPORTS)
    };
    let mut all: Vec<&str> = runtime::COMMON_IMPORTS.iter().chain(imports.iter()).copied().collect();
    all.sort_unstable();
    all.dedup();
    for i in all {
        w.line(&format!("import {i};"));
    }
    w.blank();
    let mut s = w.finish();
    s.push_str(&runtime::COMMON.replace("@@FORMAT@@", cx.format()));
    s.push_str(&codec);
    s
}

// ── Types ─────────────────────────────────────────────────────────────────────

fn java_primitive(p: &Primitive, boxed: bool) -> &'static str {
    match (p, boxed) {
        (Primitive::Bool, false) => "boolean",
        (Primitive::Bool, true) => "Boolean",
        (Primitive::Uint | Primitive::Int, false) => "long",
        (Primitive::Uint | Primitive::Int, true) => "Long",
        (Primitive::Float16 | Primitive::Float32, false) => "float",
        (Primitive::Float16 | Primitive::Float32, true) => "Float",
        (Primitive::Float64 | Primitive::Float, false) => "double",
        (Primitive::Float64 | Primitive::Float, true) => "Double",
        (Primitive::Tstr, _) => "String",
        (Primitive::Bstr | Primitive::Any, _) => "byte[]",
        (Primitive::Null | Primitive::Undefined, _) => "Cddl.Null",
    }
}

/// The Java type of a value; `qual` prefixes named types with the package
/// (inside a sealed enum, where a variant record may share a type's name).
fn java_type(ty: &TypeRef, boxed: bool, qual: Option<&str>) -> String {
    match ty {
        TypeRef::Primitive(p) => java_primitive(p, boxed).to_owned(),
        TypeRef::Named(n) => match qual {
            Some(q) => format!("{q}.{}", java_type_name(n)),
            None => java_type_name(n),
        },
        TypeRef::Tagged(_, inner) => java_type(inner, boxed, qual),
        TypeRef::Choice(_) => "Object".into(),
    }
}

fn is_repeated(occ: &Occurrence) -> bool {
    matches!(occ, Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. } | Occurrence::Bounded { .. })
}

fn member_type(f: &FieldDef) -> String {
    match &f.occurrence {
        Occurrence::Optional => java_type(&f.ty, true, None),
        o if is_repeated(o) => format!("List<{}>", java_type(&f.ty, true, None)),
        _ => java_type(&f.ty, false, None),
    }
}

/// The primitive under tags, if `ty` is one.
fn base_primitive(ty: &TypeRef) -> Option<&Primitive> {
    match ty {
        TypeRef::Primitive(p) => Some(p),
        TypeRef::Tagged(_, inner) => base_primitive(inner),
        _ => None,
    }
}

fn is_bytes(ty: &TypeRef) -> bool {
    matches!(base_primitive(ty), Some(Primitive::Bstr | Primitive::Any))
}

/// Whether a Java value of this type is a reference that must not be null.
fn is_reference(ty: &TypeRef) -> bool {
    match base_primitive(ty) {
        Some(p) => matches!(p, Primitive::Tstr | Primitive::Bstr | Primitive::Any | Primitive::Null | Primitive::Undefined),
        None => true,
    }
}

/// A Java expression for `expr`'s raw value.
fn to_raw(ty: &TypeRef, expr: &str) -> String {
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => format!("Cddl.uint({expr})"),
            Primitive::Int => format!("Cddl.integer({expr})"),
            Primitive::Float16 | Primitive::Float32 => format!("Cddl.f32({expr})"),
            Primitive::Float64 | Primitive::Float => format!("Cddl.f64({expr})"),
            Primitive::Bool => format!("Cddl.bool({expr})"),
            Primitive::Tstr => expr.to_owned(),
            Primitive::Bstr | Primitive::Any => format!("Cddl.bytes({expr})"),
            Primitive::Null | Primitive::Undefined => "null".into(),
        },
        TypeRef::Named(_) => format!("{expr}.toRaw()"),
        TypeRef::Tagged(tag, inner) => format!("Cddl.tag({}L, {})", tag.0, to_raw(inner, expr)),
        TypeRef::Choice(cs) if cs.iter().all(|c| matches!(c, TypeRef::Primitive(_))) => format!("Cddl.choiceToRaw({expr})"),
        TypeRef::Choice(_) => expr.to_owned(),
    }
}

/// A Java expression reading the raw `raw` as a value of `ty`, its type checked.
fn from_raw(ty: &TypeRef, raw: &str, at: &str, qual: Option<&str>) -> String {
    let at_lit = java_str(at);
    match ty {
        TypeRef::Primitive(p) => {
            let f = match p {
                Primitive::Uint => "asUint",
                Primitive::Int => "asInt",
                Primitive::Float16 | Primitive::Float32 => "asFloat32",
                Primitive::Float64 | Primitive::Float => "asFloat64",
                Primitive::Bool => "asBool",
                Primitive::Tstr => "asText",
                Primitive::Bstr | Primitive::Any => "asBytes",
                Primitive::Null | Primitive::Undefined => "asNull",
            };
            format!("Cddl.{f}({raw}, {at_lit})")
        }
        TypeRef::Named(_) => format!("{}.fromRaw({raw})", java_type(ty, false, qual)),
        TypeRef::Tagged(tag, inner) => from_raw(inner, &format!("Cddl.untag({raw}, {}L, {at_lit})", tag.0), at, qual),
        // A choice of primitives: each alternative's reader, in order.
        TypeRef::Choice(cs) if cs.iter().all(|c| matches!(c, TypeRef::Primitive(_))) => {
            let alts: Vec<String> = cs.iter().map(|c| format!("r -> {}", from_raw(c, "r", at, qual))).collect();
            format!("Cddl.choice({raw}, {at_lit}, {})", alts.join(", "))
        }
        TypeRef::Choice(_) => raw.to_owned(),
    }
}

// ── Constraints ───────────────────────────────────────────────────────────────

fn is_float(p: &Primitive) -> bool {
    matches!(p, Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float)
}

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

/// A Java literal of primitive `p`.
fn java_literal(v: &LiteralValue, p: &Primitive) -> Option<String> {
    Some(match v {
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Text(s) => java_str(s),
        LiteralValue::Uint(n) if is_float(p) => format!("{n}.0{}", if matches!(p, Primitive::Float16 | Primitive::Float32) { "f" } else { "" }),
        LiteralValue::Int(n) if is_float(p) => format!("{n}.0{}", if matches!(p, Primitive::Float16 | Primitive::Float32) { "f" } else { "" }),
        LiteralValue::Uint(n) => long_lit(*n as i128),
        LiteralValue::Int(n) => long_lit(*n as i128),
        LiteralValue::Float(f) if matches!(p, Primitive::Float16 | Primitive::Float32) => format!("{f:?}f"),
        LiteralValue::Float(f) => format!("{f:?}"),
        LiteralValue::Bytes(b) => java_bytes(b),
        LiteralValue::Null => return None,
    })
}

/// A Java boolean expression that holds when `x` (a value of primitive `p`)
/// meets the constraint, or `None` when it does not apply.
fn java_check(c: &Constraint, p: &Primitive, x: &str) -> Option<String> {
    let len = match p {
        Primitive::Tstr => Some(format!("Cddl.utf8Length({x})")),
        Primitive::Bstr | Primitive::Any => Some(format!("{x}.length")),
        _ => None,
    };
    let is_uint = matches!(p, Primitive::Uint);
    let numeric = matches!(p, Primitive::Uint | Primitive::Int) || is_float(p);
    // Unsigned comparison for uint, which a long holds as its bit pattern.
    let cmp = |op: &str, bound: i128| -> String {
        if is_uint { format!("Long.compareUnsigned({x}, {}) {op} 0", long_lit(bound)) } else if is_float(p) { format!("{x} {op} {bound}") } else { format!("{x} {op} {}", long_lit(bound)) }
    };
    let join = |parts: Vec<String>| (!parts.is_empty()).then(|| parts.join(" && "));
    match c {
        Constraint::SizeExact(n) => len.map(|l| format!("{l} == {n}")),
        Constraint::SizeRange { min, max } => {
            let l = len?;
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("{l} >= {lo}")); }
            if let Some(hi) = max { parts.push(format!("{l} <= {hi}")); }
            join(parts)
        }
        Constraint::ValueRangeInt { min, max, inclusive } if numeric => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if let Some(lo) = min.filter(|lo| !(is_uint && *lo <= 0)) { parts.push(cmp(">=", lo as i128)); }
            // No uint is at or below a negative upper bound.
            match max {
                Some(hi) if is_uint && *hi < 0 => return Some("false".into()),
                Some(hi) => parts.push(cmp(le, *hi as i128)),
                None => {}
            }
            join(parts)
        }
        Constraint::ValueRangeUint { min, max, inclusive } if numeric => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            // No long reaches a lower bound past its range; every long is below such
            // an upper bound.
            if min.is_some_and(|lo| matches!(p, Primitive::Int) && lo > i64::MAX as u64) { return Some("false".into()); }
            if let Some(lo) = min.filter(|lo| *lo > 0) { parts.push(cmp(">=", lo as i128)); }
            if let Some(hi) = max.filter(|hi| is_uint || is_float(p) || *hi <= i64::MAX as u64) { parts.push(cmp(le, hi as i128)); }
            join(parts)
        }
        Constraint::ValueRangeF64 { min, max, inclusive } if is_float(p) => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("{x} >= {lo:?}")); }
            if let Some(hi) = max { parts.push(format!("{x} {le} {hi:?}")); }
            join(parts)
        }
        Constraint::Eq(v) | Constraint::Ne(v) if literal_fits(v, p) => {
            let lit = java_literal(v, p)?;
            let eq = match v {
                LiteralValue::Text(_) => format!("{lit}.equals({x})"),
                LiteralValue::Bytes(_) => format!("java.util.Arrays.equals({x}, {lit})"),
                _ => format!("{x} == {lit}"),
            };
            Some(if matches!(c, Constraint::Ne(_)) { format!("!({eq})") } else { eq })
        }
        Constraint::Regexp { pattern, .. } if matches!(p, Primitive::Tstr) => Some(format!("{x}.matches({})", java_str(pattern))),
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
        Constraint::Regexp { pattern, .. } => format!("must match {pattern}"),
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
        if let Some(check) = java_check(c, p, x) {
            w.line(&format!("if (!({check})) throw Cddl.invalid({}, {});", java_str(at), java_str(&describe(c))));
        }
    }
}

fn has_checks(ty: &TypeRef, constraints: &[Constraint]) -> bool {
    base_primitive(ty).is_some_and(|p| constraints.iter().any(|c| java_check(c, p, "x").is_some()))
}

// ── Jakarta EE annotations ────────────────────────────────────────────────────

const BV: &str = "jakarta.validation.constraints";

/// The Bean Validation annotations of a CDDL constraint on a value of `p`.
fn constraint_annotations(c: &Constraint, p: &Primitive) -> Vec<String> {
    let text_or_bytes = matches!(p, Primitive::Tstr | Primitive::Bstr | Primitive::Any);
    let int_like = matches!(p, Primitive::Uint | Primitive::Int);
    let fits = |n: i128| n >= i64::MIN as i128 && n <= i64::MAX as i128;
    let min_max = |lo: Option<i128>, hi: Option<i128>, inclusive: bool| -> Vec<String> {
        let mut out = Vec::new();
        if let Some(lo) = lo.filter(|lo| fits(*lo) && !(matches!(p, Primitive::Uint) && *lo <= 0)) {
            out.push(format!("@{BV}.Min({lo}L)"));
        }
        if let Some(hi) = hi.map(|h| if inclusive { h } else { h - 1 }).filter(|h| fits(*h)) {
            out.push(format!("@{BV}.Max({hi}L)"));
        }
        out
    };
    match c {
        Constraint::SizeExact(n) if text_or_bytes => vec![format!("@{BV}.Size(min = {n}, max = {n})")],
        Constraint::SizeRange { min, max } if text_or_bytes => {
            let mut args = Vec::new();
            if let Some(lo) = min { args.push(format!("min = {lo}")); }
            if let Some(hi) = max { args.push(format!("max = {hi}")); }
            if args.is_empty() { vec![] } else { vec![format!("@{BV}.Size({})", args.join(", "))] }
        }
        Constraint::ValueRangeInt { min, max, inclusive } if int_like => min_max(min.map(i128::from), max.map(i128::from), *inclusive),
        Constraint::ValueRangeUint { min, max, inclusive } if int_like => min_max(min.map(i128::from), max.map(i128::from), *inclusive),
        Constraint::ValueRangeInt { min, max, inclusive } if is_float(p) => decimal_bounds(min.map(|v| v.to_string()), max.map(|v| v.to_string()), *inclusive),
        Constraint::ValueRangeUint { min, max, inclusive } if is_float(p) => decimal_bounds(min.map(|v| v.to_string()), max.map(|v| v.to_string()), *inclusive),
        Constraint::ValueRangeF64 { min, max, inclusive } if is_float(p) => decimal_bounds(min.map(|v| format!("{v:?}")), max.map(|v| format!("{v:?}")), *inclusive),
        Constraint::Eq(LiteralValue::Bool(true)) if matches!(p, Primitive::Bool) => vec![format!("@{BV}.AssertTrue")],
        Constraint::Eq(LiteralValue::Bool(false)) if matches!(p, Primitive::Bool) => vec![format!("@{BV}.AssertFalse")],
        Constraint::Eq(LiteralValue::Text(t)) if matches!(p, Primitive::Tstr) => vec![format!("@{BV}.Pattern(regexp = {})", java_str(&regex_quote(t)))],
        Constraint::Eq(LiteralValue::Uint(n)) if int_like => min_max(Some(*n as i128), Some(*n as i128), true),
        Constraint::Eq(LiteralValue::Int(n)) if int_like => min_max(Some(*n as i128), Some(*n as i128), true),
        Constraint::Regexp { pattern, .. } if matches!(p, Primitive::Tstr) => vec![format!("@{BV}.Pattern(regexp = {})", java_str(pattern))],
        _ => vec![],
    }
}

fn decimal_bounds(min: Option<String>, max: Option<String>, inclusive: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(lo) = min { out.push(format!("@{BV}.DecimalMin({})", java_str(&lo))); }
    if let Some(hi) = max { out.push(format!("@{BV}.DecimalMax(value = {}, inclusive = {inclusive})", java_str(&hi))); }
    out
}

/// A literal as a Java regex matching exactly it.
fn regex_quote(s: &str) -> String {
    format!("\\Q{}\\E", s.replace("\\E", "\\E\\\\E\\Q"))
}

/// The annotations of a record component (Jakarta EE only).
fn component_annotations(cx: &Cx, ty: &TypeRef, occ: &Occurrence, constraints: &[Constraint]) -> String {
    if !cx.ee() { return String::new(); }
    let mut out = Vec::new();
    let required = !matches!(occ, Occurrence::Optional);
    if required && (is_reference(ty) || is_repeated(occ)) {
        out.push(format!("@{BV}.NotNull"));
    }
    if matches!(ty, TypeRef::Named(_)) {
        out.push("@jakarta.validation.Valid".into());
    }
    if let (Some(p), false) = (base_primitive(ty), is_repeated(occ)) {
        for c in constraints {
            out.extend(constraint_annotations(c, p));
        }
    }
    if out.is_empty() { String::new() } else { format!("{} ", out.join(" ")) }
}

/// JSON-B annotations naming a type's codec (Jakarta EE JSON only).
fn jsonb_annotations(w: &mut IndentWriter, cx: &Cx, codec_owner: &str) {
    if cx.jsonb() {
        w.line(&format!("@jakarta.json.bind.annotation.JsonbTypeSerializer({codec_owner}.JsonbCodec.class)"));
        w.line(&format!("@jakarta.json.bind.annotation.JsonbTypeDeserializer({codec_owner}.JsonbCodec.class)"));
    }
}

/// JSON-B annotation for a Java `enum`: Yasson reads enums by constant name and
/// ignores a type-level deserializer, but honours an adapter.
fn jsonb_enum_annotation(w: &mut IndentWriter, cx: &Cx, class: &str) {
    if cx.jsonb() {
        w.line(&format!("@jakarta.json.bind.annotation.JsonbTypeAdapter({class}.JsonbAdapter.class)"));
    }
}

/// The JSON-B adapter of a Java `enum`: to and from the JSON-P value encode/decode write.
fn emit_jsonb_enum_adapter(w: &mut IndentWriter, cx: &Cx, class: &str) {
    if !cx.jsonb() { return; }
    w.blank();
    w.line("/** JSON-B support: writes and reads the same JSON as {@code encode} / {@code decode}. */");
    w.line(&format!("public static final class JsonbAdapter implements jakarta.json.bind.adapter.JsonbAdapter<{class}, jakarta.json.JsonValue> {{"));
    w.indent();
    w.line("@Override");
    w.line(&format!("public jakarta.json.JsonValue adaptToJson({class} value) {{"));
    w.line("    return Cddl.toJson(value.toRaw());");
    w.line("}");
    w.blank();
    w.line("@Override");
    w.line(&format!("public {class} adaptFromJson(jakarta.json.JsonValue value) {{"));
    w.line(&format!("    return {class}.fromRaw(Cddl.fromJson(value));"));
    w.line("}");
    w.dedent();
    w.line("}");
}

/// The JSON-B codec class: it writes and reads the same JSON as encode/decode.
fn emit_jsonb_codec(w: &mut IndentWriter, cx: &Cx, class: &str) {
    if !cx.jsonb() { return; }
    w.blank();
    w.line("/** JSON-B support: writes and reads the same JSON as {@code encode} / {@code decode}. */");
    w.line(&format!("public static final class JsonbCodec implements jakarta.json.bind.serializer.JsonbSerializer<{class}>, jakarta.json.bind.serializer.JsonbDeserializer<{class}> {{"));
    w.indent();
    w.line("@Override");
    w.line(&format!("public void serialize({class} value, jakarta.json.stream.JsonGenerator generator, jakarta.json.bind.serializer.SerializationContext ctx) {{"));
    w.line("    generator.write(Cddl.toJson(value.toRaw()));");
    w.line("}");
    w.blank();
    w.line("@Override");
    w.line(&format!("public {class} deserialize(jakarta.json.stream.JsonParser parser, jakarta.json.bind.serializer.DeserializationContext ctx, java.lang.reflect.Type type) {{"));
    w.line(&format!("    return {class}.fromRaw(Cddl.fromJson(parser.getValue()));"));
    w.line("}");
    w.dedent();
    w.line("}");
}

// ── Type files ────────────────────────────────────────────────────────────────

fn emit_type_file(cx: &Cx, def: &TypeDef) -> String {
    let mut w = IndentWriter::new(4);
    header(&mut w, cx);
    w.line("import java.util.ArrayList;");
    w.line("import java.util.LinkedHashMap;");
    w.line("import java.util.List;");
    w.line("import java.util.Map;");
    w.blank();
    match def {
        TypeDef::Struct(s) => emit_struct(&mut w, cx, s),
        TypeDef::Enum(e) if e.variants.iter().all(|v| !has_payload(v)) => emit_literal_enum(&mut w, cx, e),
        TypeDef::Enum(e) => emit_sealed_enum(&mut w, cx, e),
        TypeDef::Array(a) => emit_array(&mut w, cx, a),
        TypeDef::Alias(a) => emit_alias(&mut w, cx, a),
        TypeDef::Map(m) => emit_table(&mut w, cx, m),
    }
    w.finish()
}

/// `encode` / `decode`, shared by every class.
fn emit_encode_decode(w: &mut IndentWriter, cx: &Cx, class: &str, is_static_ctx: bool) {
    let f = cx.format();
    w.blank();
    w.line(&format!("/** Encodes this value as {f}; throws {{@link IllegalArgumentException}} if a constraint fails. */"));
    if is_static_ctx {
        w.line("default byte[] encode() {");
    } else {
        w.line("public byte[] encode() {");
    }
    w.line("    return Cddl.encode(toRaw());");
    w.line("}");
    w.blank();
    w.line(&format!("/** Decodes a {class} from {f}; throws {{@link IllegalArgumentException}} on invalid input. */"));
    w.line(&format!("{}static {class} decode(byte[] data) {{", if is_static_ctx { "" } else { "public " }));
    w.line("    return fromRaw(Cddl.decode(data));");
    w.line("}");
}

/// `equals` / `hashCode` for a record holding byte strings, which compare by
/// content rather than identity.
fn emit_content_equality(w: &mut IndentWriter, class: &str, members: &[String]) {
    w.blank();
    w.line("@Override");
    w.line("public boolean equals(Object o) {");
    let mut cond = format!("o instanceof {class} that");
    for m in members {
        cond.push_str(&format!(" && Cddl.eq(this.{m}, that.{m})"));
    }
    w.line(&format!("    return {cond};"));
    w.line("}");
    w.blank();
    w.line("@Override");
    w.line("public int hashCode() {");
    let hs: Vec<String> = members.iter().map(|m| format!("Cddl.hash(this.{m})")).collect();
    w.line(&format!("    return java.util.Objects.hash({});", hs.join(", ")));
    w.line("}");
}

fn struct_fields<'a>(s: &'a StructDef, cx: &Cx) -> Vec<&'a FieldDef> {
    let mut fs: Vec<_> = s.fields.iter().collect();
    if cx.opts.dcbor {
        fs.sort_by(|a, b| a.name.cmp(&b.name));
    }
    fs
}

fn emit_struct(w: &mut IndentWriter, cx: &Cx, s: &StructDef) {
    let class = java_type_name(&s.name);
    emit_javadoc(w, s.doc.as_deref(), &format!("The {} map.", s.name));
    if s.deprecated { w.line("@Deprecated"); }
    jsonb_annotations(w, cx, &class);
    // The record's components: the members, then the map's other entries
    // (dgtlrift/cddlc#82).
    let mut components: Vec<String> = s.fields.iter().map(|f| {
        let ann = component_annotations(cx, &f.ty, &f.occurrence, &f.constraints);
        format!("{ann}{} {}", member_type(f), java_member_name(&f.name))
    }).collect();
    if let Some(r) = &s.rest {
        let ann = if cx.ee() { format!("@{BV}.NotNull ") } else { String::new() };
        components.push(format!("{ann}Map<String, {}> {}", java_type(&r.val_ty, true, None), java_member_name(&r.name)));
    }
    if components.is_empty() {
        w.line(&format!("public record {class}() {{"));
    } else {
        w.line(&format!("public record {class}("));
        w.indent();
        w.indent();
        let n = components.len();
        for (i, c) in components.iter().enumerate() {
            let sep = if i + 1 < n { "," } else { ") {" };
            w.line(&format!("{c}{sep}"));
        }
        w.dedent();
        w.dedent();
    }
    w.indent();

    // validate
    w.line("/** Checks the CDDL constraints on every member; throws {@link IllegalArgumentException}. */");
    w.line("public void validate() {");
    w.indent();
    for f in &s.fields {
        let m = format!("this.{}", java_member_name(&f.name));
        let at = format!("{class}.{}", f.name);
        match &f.occurrence {
            Occurrence::Optional => {
                if has_checks(&f.ty, &f.constraints) {
                    w.line(&format!("if ({m} != null) {{"));
                    w.indent();
                    emit_checks(w, &f.ty, &f.constraints, &m, &at);
                    w.dedent();
                    w.line("}");
                }
            }
            o if is_repeated(o) => {
                w.line(&format!("if ({m} == null) throw Cddl.invalid({}, \"missing\");", java_str(&at)));
            }
            _ => {
                if is_reference(&f.ty) {
                    w.line(&format!("if ({m} == null) throw Cddl.invalid({}, \"missing\");", java_str(&at)));
                }
                emit_checks(w, &f.ty, &f.constraints, &m, &at);
            }
        }
    }
    if let Some(r) = &s.rest {
        let m = format!("this.{}", java_member_name(&r.name));
        let at = java_str(&format!("{class}.{}", r.name));
        w.line(&format!("if ({m} == null) throw Cddl.invalid({at}, \"missing\");"));
        let named: Vec<String> = s.fields.iter().map(|f| java_str(&f.name)).collect();
        w.line(&format!(
            "for (Map.Entry<String, {}> e : {m}.entrySet()) {{",
            java_type(&r.val_ty, true, None)
        ));
        w.line(&format!("    if (e.getKey() == null || e.getValue() == null) throw Cddl.invalid({at}, \"null entry\");"));
        if !named.is_empty() {
            w.line(&format!(
                "    if (List.of({}).contains(e.getKey())) throw Cddl.invalid({at}, \"other entry \" + e.getKey() + \" is a named member's key\");",
                named.join(", ")
            ));
        }
        w.line("}");
    }
    w.dedent();
    w.line("}");
    w.blank();

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_record_raw(w, s, &class);
    } else {
        emit_map_raw(w, cx, s, &class);
    }
    emit_encode_decode(w, cx, &class, false);

    let byte_members: Vec<String> = s.fields.iter().map(|f| java_member_name(&f.name))
        .chain(s.rest.iter().map(|r| java_member_name(&r.name)))
        .collect();
    if s.fields.iter().any(|f| is_bytes(&f.ty)) {
        emit_content_equality(w, &class, &byte_members);
    }
    emit_jsonb_codec(w, cx, &class);
    w.dedent();
    w.line("}");
}

/// A map struct's `toRaw` / `fromRaw`: its members by key, in schema order.
fn emit_map_raw(w: &mut IndentWriter, cx: &Cx, s: &StructDef, class: &str) {
    let class = class.to_owned();
    // toRaw
    w.line("/** This value in the format-neutral value model. */");
    w.line("public Object toRaw() {");
    w.indent();
    w.line("validate();");
    w.line("Map<String, Object> m = new LinkedHashMap<>();");
    for f in struct_fields(s, cx) {
        let member = format!("this.{}", java_member_name(&f.name));
        let key = java_str(&f.name);
        match &f.occurrence {
            Occurrence::Optional => {
                w.line(&format!("if ({member} != null) m.put({key}, {});", to_raw(&f.ty, &member)));
            }
            o if is_repeated(o) => {
                w.line("{");
                w.indent();
                w.line("List<Object> l = new ArrayList<>();");
                w.line(&format!("for ({} x : {member}) l.add({});", java_type(&f.ty, true, None), to_raw(&f.ty, "x")));
                w.line(&format!("m.put({key}, l);"));
                w.dedent();
                w.line("}");
            }
            _ => w.line(&format!("m.put({key}, {});", to_raw(&f.ty, &member))),
        }
    }
    if let Some(r) = &s.rest {
        w.line(&format!(
            "for (Map.Entry<String, {}> e : this.{}.entrySet()) m.put(e.getKey(), {});",
            java_type(&r.val_ty, true, None),
            java_member_name(&r.name),
            to_raw(&r.val_ty, "e.getValue()")
        ));
    }
    let ret = match s.tagged { Some(t) => format!("Cddl.tag({}L, m)", t.0), None => "m".into() };
    w.line(&format!("return {ret};"));
    w.dedent();
    w.line("}");
    w.blank();

    // fromRaw
    w.line(&format!("/** Reads a {class} from the value model, checking each member's type and constraints. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match s.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(&class)), None => "raw".into() };
    w.line(&format!("Map<String, Object> m = Cddl.asMap({raw}, {});", java_str(&class)));
    let mut args = Vec::new();
    // A map has only the members its schema gives it (RFC 8610 §3.5;
    // dgtlrift/cddlc#102), unless it has other entries.
    if s.rest.is_none() {
        let named: Vec<String> = s.fields.iter().map(|f| java_str(&f.name)).collect();
        w.line(&format!(
            "for (String k : m.keySet()) if (!List.<String>of({}).contains(k)) throw Cddl.invalid({}, \"unknown member \" + k);",
            named.join(", "),
            java_str(&class)
        ));
    }
    for f in &s.fields {
        let local = format!("f_{}", java_member_name(&f.name));
        let key = java_str(&f.name);
        let at = format!("{class}.{}", f.name);
        match &f.occurrence {
            Occurrence::Optional => {
                let is_null = matches!(base_primitive(&f.ty), Some(Primitive::Null | Primitive::Undefined));
                // A present `nil` member is null; for any other type null means absent.
                let cond = if is_null { format!("m.containsKey({key})") } else { format!("Cddl.present(m, {key})") };
                w.line(&format!("{} {local} = {cond} ? {} : null;", member_type(f), from_raw(&f.ty, &format!("m.get({key})"), &at, None)));
            }
            o if is_repeated(o) => {
                w.line(&format!("{} {local} = new ArrayList<>();", member_type(f)));
                w.line(&format!("if (Cddl.present(m, {key})) {{"));
                w.line(&format!("    for (Object x : Cddl.asList(m.get({key}), {})) {local}.add({});", java_str(&at), from_raw(&f.ty, "x", &at, None)));
                w.line("}");
            }
            _ => {
                w.line(&format!("{} {local} = {};", member_type(f), from_raw(&f.ty, &format!("Cddl.required(m, {key}, {})", java_str(&class)), &at, None)));
            }
        }
        args.push(local);
    }
    if let Some(r) = &s.rest {
        let val = java_type(&r.val_ty, true, None);
        let named: Vec<String> = s.fields.iter().map(|f| java_str(&f.name)).collect();
        w.line(&format!("Map<String, {val}> rest = new LinkedHashMap<>();"));
        w.line("for (Map.Entry<String, Object> e : m.entrySet()) {");
        w.indent();
        if !named.is_empty() {
            w.line(&format!("if (List.of({}).contains(e.getKey())) continue;", named.join(", ")));
        }
        w.line(&format!("rest.put(e.getKey(), {});", from_raw(&r.val_ty, "e.getValue()", &format!("{class} value"), None)));
        w.dedent();
        w.line("}");
        args.push("rest".into());
    }
    w.line(&format!("{class} out = new {class}({});", args.join(", ")));
    w.line("out.validate();");
    w.line("return out;");
    w.dedent();
    w.line("}");
}

/// A record's `toRaw` / `fromRaw` (RFC 8610 §3.4): a list of the members'
/// values, in order. Optional members come last: those present are written up
/// to the first absent one, and one set after an absent one is an error.
fn emit_record_raw(w: &mut IndentWriter, s: &StructDef, class: &str) {
    w.line("/** This value in the format-neutral value model: an array of its members. */");
    w.line("public Object toRaw() {");
    w.indent();
    w.line("validate();");
    w.line("List<Object> a = new ArrayList<>();");
    let mut gap = false;
    for f in &s.fields {
        let member = format!("this.{}", java_member_name(&f.name));
        if f.occurrence.is_required() {
            w.line(&format!("a.add({});", to_raw(&f.ty, &member)));
        } else {
            if !gap {
                w.line("boolean gap = false;");
                gap = true;
            }
            w.line(&format!("if ({member} != null) {{"));
            w.indent();
            w.line(&format!(
                "if (gap) throw Cddl.invalid({}, \"set, but an optional member before it isn't\");",
                java_str(&format!("{class}.{}", f.name))
            ));
            w.line(&format!("a.add({});", to_raw(&f.ty, &member)));
            w.dedent();
            w.line("} else {");
            w.line("    gap = true;");
            w.line("}");
        }
    }
    let ret = match s.tagged { Some(t) => format!("Cddl.tag({}L, a)", t.0), None => "a".into() };
    w.line(&format!("return {ret};"));
    w.dedent();
    w.line("}");
    w.blank();

    let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();
    let want = if required == s.fields.len() { required.to_string() } else { format!("{required} to {}", s.fields.len()) };
    w.line(&format!("/** Reads a {class} from the value model, checking each member's type and constraints. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match s.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(class)), None => "raw".into() };
    w.line(&format!("List<Object> a = Cddl.asList({raw}, {});", java_str(class)));
    w.line(&format!("if (a.size() < {required} || a.size() > {}) {{", s.fields.len()));
    w.line(&format!("    throw Cddl.invalid({}, \"expected {want} items, got \" + a.size());", java_str(class)));
    w.line("}");
    let mut args = Vec::new();
    for (i, f) in s.fields.iter().enumerate() {
        let local = format!("f_{}", java_member_name(&f.name));
        let at = format!("{class}.{}", f.name);
        let conv = from_raw(&f.ty, &format!("a.get({i})"), &at, None);
        if f.occurrence.is_required() {
            w.line(&format!("{} {local} = {conv};", member_type(f)));
        } else {
            w.line(&format!("{} {local} = a.size() > {i} ? {conv} : null;", member_type(f)));
        }
        args.push(local);
    }
    w.line(&format!("{class} out = new {class}({});", args.join(", ")));
    w.line("out.validate();");
    w.line("return out;");
    w.dedent();
    w.line("}");
}

// ── Enums ─────────────────────────────────────────────────────────────────────

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

/// The raw value a literal variant is written as: its integer, or its CDDL text.
fn literal_wire(v: &EnumVariant) -> String {
    match int_literal(v) {
        Some(n) if n > i64::MAX as i128 => format!("Cddl.uint({})", long_lit(n)),
        Some(n) => long_lit(n),
        None => java_str(&cddlc_codegen::enum_variant_text(v)),
    }
}

/// An enum whose variants are all literals: a Java `enum`.
fn emit_literal_enum(w: &mut IndentWriter, cx: &Cx, e: &EnumDef) {
    let class = java_type_name(&e.name);
    emit_javadoc(w, e.doc.as_deref(), &format!("One of the {} literals.", e.name));
    if e.deprecated { w.line("@Deprecated"); }
    jsonb_enum_annotation(w, cx, &class);
    w.line(&format!("public enum {class} {{"));
    w.indent();
    let n = e.variants.len();
    for (i, v) in e.variants.iter().enumerate() {
        let sep = if i + 1 < n { "," } else { ";" };
        w.line(&format!("{}({}, {}){sep}", java_enum_constant(&v.name), java_str(&to_pascal_case(&v.name)), literal_wire(v)));
    }
    w.blank();
    w.line("private final String kind;");
    w.line("private final Object wire;");
    w.blank();
    w.line(&format!("{class}(String kind, Object wire) {{"));
    w.line("    this.kind = kind;");
    w.line("    this.wire = wire;");
    w.line("}");
    w.blank();
    w.line("/** The variant's name in the schema. */");
    w.line("public String kind() {");
    w.line("    return kind;");
    w.line("}");
    w.blank();
    w.line("/** This value in the format-neutral value model. */");
    w.line("public Object toRaw() {");
    let ret = match e.tagged { Some(t) => format!("Cddl.tag({}L, wire)", t.0), None => "wire".into() };
    w.line(&format!("    return {ret};"));
    w.line("}");
    w.blank();
    w.line(&format!("/** Reads a {class} from the value model: the variant whose literal it is. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match e.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(&class)), None => "raw".into() };
    w.line(&format!("Object v = {raw};"));
    w.line(&format!("for ({class} c : values()) {{"));
    w.line("    if (java.util.Objects.equals(c.wire, v)) return c;");
    w.line("}");
    w.line(&format!("throw Cddl.noVariant({}, v);", java_str(&class)));
    w.dedent();
    w.line("}");
    emit_encode_decode(w, cx, &class, false);
    emit_jsonb_enum_adapter(w, cx, &class);
    w.dedent();
    w.line("}");
}

/// An enum with valued variants: a sealed interface with a record per variant.
fn emit_sealed_enum(w: &mut IndentWriter, cx: &Cx, e: &EnumDef) {
    let class = java_type_name(&e.name);
    let qual = Some(cx.pkg);
    emit_javadoc(w, e.doc.as_deref(), &format!("One of the {} variants: a record per variant.", e.name));
    if e.deprecated { w.line("@Deprecated"); }
    jsonb_annotations(w, cx, &class);
    w.line(&format!("public sealed interface {class} {{"));
    w.indent();

    for v in &e.variants {
        let rec = java_variant_record(&e.name, &v.name);
        w.line(&format!("/** The {} variant. */", to_pascal_case(&v.name)));
        jsonb_annotations(w, cx, &class);
        let raw = if let Some(_) = int_literal(v) {
            literal_wire(v)
        } else if has_payload(v) {
            to_raw(&v.ty, "this.value")
        } else {
            literal_wire(v)
        };
        let raw = match e.tagged { Some(t) => format!("Cddl.tag({}L, {raw})", t.0), None => raw };
        if has_payload(v) {
            let ann = component_annotations(cx, &v.ty, &Occurrence::Required, &[]);
            w.line(&format!("record {rec}({ann}{} value) implements {class} {{", java_type(&v.ty, false, qual)));
        } else {
            w.line(&format!("record {rec}() implements {class} {{"));
        }
        w.indent();
        w.line("@Override");
        w.line("public Object toRaw() {");
        if has_payload(v) && is_reference(&v.ty) {
            w.line(&format!("    if (this.value == null) throw Cddl.invalid({}, \"missing\");", java_str(&format!("{class}.{rec}"))));
        }
        w.line(&format!("    return {raw};"));
        w.line("}");
        w.blank();
        w.line("@Override");
        w.line("public String kind() {");
        w.line(&format!("    return {};", java_str(&to_pascal_case(&v.name))));
        w.line("}");
        if has_payload(v) && is_bytes(&v.ty) {
            emit_content_equality(w, &rec, &["value".to_owned()]);
        }
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.line("/** The variant's name in the schema. */");
    w.line("String kind();");
    w.blank();
    w.line("/** This value in the format-neutral value model. */");
    w.line("Object toRaw();");
    w.blank();

    // fromRaw: literals first, then the variant whose type the value has.
    let at = java_str(&class);
    w.line(&format!("/** Reads a {class} from the value model: literals first, then by the value's type. */"));
    w.line(&format!("static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match e.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {at})", t.0), None => "raw".into() };
    w.line(&format!("Object v = {raw};"));
    let payload = |f: &dyn Fn(&Primitive) -> bool| {
        e.variants.iter().find(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(p) if f(p)))
    };
    let rec = |v: &EnumVariant| java_variant_record(&e.name, &v.name);
    let text_lits: Vec<&EnumVariant> = e.variants.iter().filter(|v| !has_payload(v) && int_literal(v).is_none()).collect();
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
    let float_arg = |v: &EnumVariant, x: &str| match &v.ty {
        TypeRef::Primitive(Primitive::Float16 | Primitive::Float32) => format!("(float) {x}"),
        _ => format!("(double) {x}"),
    };

    if !text_lits.is_empty() || tstr.is_some() || json_bytes.is_some() {
        w.line("if (v instanceof String s) {");
        w.indent();
        for v in &text_lits {
            w.line(&format!("if (s.equals({})) return new {}();", java_str(&cddlc_codegen::enum_variant_text(v)), rec(v)));
        }
        if let Some(v) = tstr {
            w.line(&format!("return new {}(s);", rec(v)));
        } else if let Some(v) = json_bytes {
            w.line("try {");
            w.line(&format!("    return new {}(Cddl.asBytes(s, {at}));", rec(v)));
            w.line("} catch (IllegalArgumentException e) {");
            w.line("    // not base64url: no variant matches");
            w.line("}");
        }
        w.dedent();
        w.line("}");
    }
    let pos_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n >= 0).collect();
    let neg_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n < 0).collect();
    if !pos_lits.is_empty() || uint.is_some() || int.is_some() || float.is_some() {
        w.line("if (Cddl.isUint(v)) {");
        w.indent();
        w.line(&format!("long u = Cddl.asUint(v, {at});"));
        for (v, n) in &pos_lits {
            w.line(&format!("if (u == {}) return new {}();", long_lit(*n), rec(v)));
        }
        if let Some(v) = uint {
            w.line(&format!("return new {}(u);", rec(v)));
        } else if let Some(v) = int {
            w.line(&format!("if (u >= 0) return new {}(u);", rec(v)));
        } else if let Some(v) = float {
            w.line(&format!("return new {}({});", rec(v), float_arg(v, "u")));
        }
        w.dedent();
        w.line("}");
    }
    if !neg_lits.is_empty() || int.is_some() || float.is_some() {
        w.line("if (v instanceof Long i && i < 0) {");
        w.indent();
        for (v, n) in &neg_lits {
            w.line(&format!("if (i == {}) return new {}();", long_lit(*n), rec(v)));
        }
        if let Some(v) = int {
            w.line(&format!("return new {}(i);", rec(v)));
        } else if let Some(v) = float {
            w.line(&format!("return new {}({});", rec(v), float_arg(v, "i")));
        }
        w.dedent();
        w.line("}");
    }
    if let Some(v) = float {
        w.line(&format!("if (v instanceof Double d) return new {}({});", rec(v), float_arg(v, "d")));
    }
    if let Some(v) = boolean {
        w.line(&format!("if (v instanceof Boolean b) return new {}(b);", rec(v)));
    }
    if let Some(v) = bytes {
        w.line(&format!("if (v instanceof byte[] b) return new {}(b);", rec(v)));
    }
    for v in e.variants.iter().filter(|v| has_payload(v) && matches!(&v.ty, TypeRef::Named(_))) {
        w.line("try {");
        w.line(&format!("    return new {}({});", rec(v), from_raw(&v.ty, "v", &class, qual)));
        w.line("} catch (IllegalArgumentException e) {");
        w.line("    // not this variant");
        w.line("}");
    }
    w.line(&format!("throw Cddl.noVariant({at}, v);"));
    w.dedent();
    w.line("}");
    emit_encode_decode(w, cx, &class, true);
    emit_jsonb_codec(w, cx, &class);
    w.dedent();
    w.line("}");
}

// ── Array ─────────────────────────────────────────────────────────────────────

fn emit_array(w: &mut IndentWriter, cx: &Cx, a: &ArrayDef) {
    let class = java_type_name(&a.name);
    let elem = java_type(&a.element, true, None);
    emit_javadoc(w, a.doc.as_deref(), &format!("The {} array.", a.name));
    if a.deprecated { w.line("@Deprecated"); }
    jsonb_annotations(w, cx, &class);
    let ann = if cx.ee() {
        let valid = if matches!(a.element, TypeRef::Named(_)) { " @jakarta.validation.Valid" } else { "" };
        format!("@{BV}.NotNull{valid} ")
    } else { String::new() };
    w.line(&format!("public record {class}({ann}List<{elem}> items) {{"));
    w.indent();
    w.line("/** Checks that the items are present; throws {@link IllegalArgumentException}. */");
    w.line("public void validate() {");
    w.line(&format!("    if (this.items == null) throw Cddl.invalid({}, \"missing items\");", java_str(&class)));
    w.line(&format!("    for ({elem} x : this.items) if (x == null) throw Cddl.invalid({}, \"null item\");", java_str(&class)));
    w.line("}");
    w.blank();
    w.line("/** This value in the format-neutral value model. */");
    w.line("public Object toRaw() {");
    w.indent();
    w.line("validate();");
    w.line("List<Object> l = new ArrayList<>();");
    w.line(&format!("for ({elem} x : this.items) l.add({});", to_raw(&a.element, "x")));
    let ret = match a.tagged { Some(t) => format!("Cddl.tag({}L, l)", t.0), None => "l".into() };
    w.line(&format!("return {ret};"));
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("/** Reads a {class} from the value model, checking each item's type. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match a.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(&class)), None => "raw".into() };
    w.line(&format!("List<{elem}> out = new ArrayList<>();"));
    w.line(&format!("for (Object x : Cddl.asList({raw}, {})) out.add({});", java_str(&class), from_raw(&a.element, "x", &format!("{class} item"), None)));
    w.line(&format!("return new {class}(out);"));
    w.dedent();
    w.line("}");
    emit_encode_decode(w, cx, &class, false);
    if is_bytes(&a.element) {
        emit_content_equality(w, &class, &["items".to_owned()]);
    }
    emit_jsonb_codec(w, cx, &class);
    w.dedent();
    w.line("}");
}

// ── Table ─────────────────────────────────────────────────────────────────────

/// A table (`{ * tstr => uint }`): a record holding a `Map` (dgtlrift/cddlc#82).
/// The value model reads a map's keys as text, so its keys are text.
fn emit_table(w: &mut IndentWriter, cx: &Cx, m: &MapDef) {
    let class = java_type_name(&m.name);
    let key = java_type(&m.key_ty, true, None);
    let val = java_type(&m.val_ty, true, None);
    emit_javadoc(w, m.doc.as_deref(), &format!("The {} table.", m.name));
    if m.deprecated { w.line("@Deprecated"); }
    jsonb_annotations(w, cx, &class);
    let ann = if cx.ee() { format!("@{BV}.NotNull ") } else { String::new() };
    w.line(&format!("public record {class}({ann}Map<{key}, {val}> entries) {{"));
    w.indent();
    w.line("/** Checks that the entries are present; throws {@link IllegalArgumentException}. */");
    w.line("public void validate() {");
    w.line(&format!("    if (this.entries == null) throw Cddl.invalid({}, \"missing entries\");", java_str(&class)));
    w.line(&format!(
        "    for (Map.Entry<{key}, {val}> e : this.entries.entrySet()) if (e.getKey() == null || e.getValue() == null) throw Cddl.invalid({}, \"null entry\");",
        java_str(&class)
    ));
    w.line("}");
    w.blank();
    w.line("/** This value in the format-neutral value model. */");
    w.line("public Object toRaw() {");
    w.indent();
    w.line("validate();");
    w.line("Map<Object, Object> m = new LinkedHashMap<>();");
    w.line(&format!(
        "for (Map.Entry<{key}, {val}> e : this.entries.entrySet()) m.put({}, {});",
        to_raw(&m.key_ty, "e.getKey()"),
        to_raw(&m.val_ty, "e.getValue()")
    ));
    let ret = match m.tagged { Some(t) => format!("Cddl.tag({}L, m)", t.0), None => "m".into() };
    w.line(&format!("return {ret};"));
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("/** Reads a {class} from the value model, checking each entry's types. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match m.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(&class)), None => "raw".into() };
    w.line(&format!("Map<{key}, {val}> out = new LinkedHashMap<>();"));
    w.line(&format!(
        "for (Map.Entry<String, Object> e : Cddl.asMap({raw}, {}).entrySet()) out.put({}, {});",
        java_str(&class),
        from_raw(&m.key_ty, "e.getKey()", &format!("{class} key"), None),
        from_raw(&m.val_ty, "e.getValue()", &format!("{class} value"), None)
    ));
    w.line(&format!("return new {class}(out);"));
    w.dedent();
    w.line("}");
    emit_encode_decode(w, cx, &class, false);
    emit_jsonb_codec(w, cx, &class);
    w.dedent();
    w.line("}");
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, cx: &Cx, a: &AliasDef) {
    let class = java_type_name(&a.name);
    let inner = java_type(&a.ty, false, None);
    emit_javadoc(w, a.doc.as_deref(), &format!("A {} value.", a.name));
    if a.deprecated { w.line("@Deprecated"); }
    jsonb_annotations(w, cx, &class);
    let ann = component_annotations(cx, &a.ty, &Occurrence::Required, &a.constraints);
    w.line(&format!("public record {class}({ann}{inner} value) {{"));
    w.indent();
    w.line("/** Checks the CDDL constraints on the value; throws {@link IllegalArgumentException}. */");
    w.line("public void validate() {");
    w.indent();
    if is_reference(&a.ty) {
        w.line(&format!("if (this.value == null) throw Cddl.invalid({}, \"missing\");", java_str(&class)));
    }
    emit_checks(w, &a.ty, &a.constraints, "this.value", &class);
    w.dedent();
    w.line("}");
    w.blank();
    w.line("/** This value in the format-neutral value model. */");
    w.line("public Object toRaw() {");
    w.line("    validate();");
    let raw = to_raw(&a.ty, "this.value");
    let ret = match a.tagged { Some(t) => format!("Cddl.tag({}L, {raw})", t.0), None => raw };
    w.line(&format!("    return {ret};"));
    w.line("}");
    w.blank();
    w.line(&format!("/** Reads a {class} from the value model, checking its type and constraints. */"));
    w.line(&format!("public static {class} fromRaw(Object raw) {{"));
    w.indent();
    let raw = match a.tagged { Some(t) => format!("Cddl.untag(raw, {}L, {})", t.0, java_str(&class)), None => "raw".into() };
    w.line(&format!("{class} out = new {class}({});", from_raw(&a.ty, &raw, &class, None)));
    w.line("out.validate();");
    w.line("return out;");
    w.dedent();
    w.line("}");
    emit_encode_decode(w, cx, &class, false);
    if is_bytes(&a.ty) {
        emit_content_equality(w, &class, &["value".to_owned()]);
    }
    emit_jsonb_codec(w, cx, &class);
    w.dedent();
    w.line("}");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

fn emit_tests(cx: &Cx) -> String {
    let mut w = IndentWriter::new(4);
    header(&mut w, cx);
    w.line("import static org.junit.jupiter.api.Assertions.assertEquals;");
    w.line("import static org.junit.jupiter.api.Assertions.assertFalse;");
    w.line("import static org.junit.jupiter.api.Assertions.assertThrows;");
    w.line("import static org.junit.jupiter.api.Assertions.assertTrue;");
    w.blank();
    w.line("import java.nio.charset.StandardCharsets;");
    w.line("import java.util.List;");
    w.line("import java.util.Map;");
    w.line("import org.junit.jupiter.api.Test;");
    w.blank();
    w.line("/** Roundtrip tests: each value encodes, decodes and compares equal. */");
    w.line("class RoundtripTest {");
    w.indent();
    if cx.ee() {
        w.line("private static final jakarta.validation.Validator VALIDATOR =");
        w.line("    jakarta.validation.Validation.buildDefaultValidatorFactory().getValidator();");
        if cx.jsonb() {
            w.line("private static final jakarta.json.bind.Jsonb JSONB = jakarta.json.bind.JsonbBuilder.create();");
        }
        w.blank();
    }
    w.line("/** Encodes the value, decodes it back and checks it is equal; Jakarta EE also checks it with Bean Validation and JSON-B. */");
    w.line("private static <T> void roundtrip(T original, byte[] encoded, java.util.function.Function<byte[], T> decode, Class<T> type) {");
    w.indent();
    w.line("assertEquals(original, decode.apply(encoded));");
    if cx.ee() {
        w.line("var violations = VALIDATOR.validate(original);");
        w.line("assertTrue(violations.isEmpty(), () -> \"constraint violations: \" + violations);");
    }
    if cx.jsonb() {
        w.line("String json = JSONB.toJson(original);");
        w.line("assertEquals(new String(encoded, StandardCharsets.UTF_8), json);");
        w.line("assertEquals(original, JSONB.fromJson(json, type));");
    }
    w.dedent();
    w.line("}");
    w.blank();

    let mut used = std::collections::HashSet::new();
    let mut test = |w: &mut IndentWriter, name: String, body: Vec<String>| {
        let mut n = name.clone();
        let mut i = 2;
        while !used.insert(n.clone()) { n = format!("{name}{i}"); i += 1; }
        w.line("@Test");
        w.line(&format!("void {n}() {{"));
        w.indent();
        for l in body { w.line(&l); }
        w.dedent();
        w.line("}");
        w.blank();
    };

    for (_, def) in &cx.module.types {
        match def {
            TypeDef::Struct(s) => {
                let class = java_type_name(&s.name);
                for (suffix, explicit) in [("Default", false), ("Explicit", true)] {
                    let value = java_struct_value(cx, s, explicit, 0);
                    test(&mut w, format!("{}Roundtrip{suffix}", camel(&s.name)), vec![
                        format!("{class} original = {value};"),
                        format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                    ]);
                }
                if s.tagged.is_some() && !cx.json() {
                    let value = java_struct_value(cx, s, false, 0);
                    test(&mut w, format!("{}EncodeHasTag", camel(&s.name)), vec![
                        format!("byte[] data = {value}.encode();"),
                        "assertEquals(6, (data[0] & 0xff) >> 5, \"CBOR major type 6 (tag)\");".into(),
                    ]);
                }
            }
            TypeDef::Enum(e) => {
                let class = java_type_name(&e.name);
                let takes_text = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
                for v in &e.variants {
                    // In JSON a byte string is text, which a tstr variant reads first.
                    if cx.json() && takes_text && has_payload(v) && is_bytes(&v.ty) { continue; }
                    let value = java_variant_value(cx, e, v, 0);
                    test(&mut w, format!("{}Roundtrip{}", camel(&e.name), to_pascal_case(&v.name)), vec![
                        format!("{class} original = {value};"),
                        format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                        format!("assertEquals({}, original.kind());", java_str(&to_pascal_case(&v.name))),
                    ]);
                }
                let lits: Vec<i128> = e.variants.iter().filter_map(int_literal).collect();
                let takes_number = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty,
                    TypeRef::Primitive(p) if matches!(p, Primitive::Uint | Primitive::Int) || is_float(p)));
                if !lits.is_empty() && !takes_number && e.tagged.is_none() {
                    if let Some(other) = (0i128..24).find(|k| !lits.contains(k)) {
                        // A CBOR unsigned integer below 24 is the one byte itself.
                        let data = if cx.json() { format!("\"{other}\".getBytes(StandardCharsets.UTF_8)") } else { format!("new byte[] {{{other}}}") };
                        test(&mut w, format!("{}RejectsUnnamedInteger", camel(&e.name)), vec![
                            format!("assertThrows(IllegalArgumentException.class, () -> {class}.decode({data}));"),
                        ]);
                    }
                }
            }
            TypeDef::Alias(a) => {
                let class = java_type_name(&a.name);
                let valid = java_value(cx, &a.ty, &Occurrence::Required, &a.constraints, false, 1);
                test(&mut w, format!("{}RoundtripValid", camel(&a.name)), vec![
                    format!("{class} original = new {class}({valid});"),
                    format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                ]);
                if let Some((bad, c)) = java_invalid_value(&a.constraints, &a.ty) {
                    let mut body = vec![format!("assertThrows(IllegalArgumentException.class, () -> new {class}({bad}).encode());")];
                    let annotated = base_primitive(&a.ty).is_some_and(|p| !constraint_annotations(c, p).is_empty());
                    if cx.ee() && annotated {
                        body.push(format!("assertFalse(VALIDATOR.validate(new {class}({bad})).isEmpty(), \"Bean Validation should reject it\");"));
                    }
                    test(&mut w, format!("{}InvalidRejected", camel(&a.name)), body);
                }
            }
            TypeDef::Array(a) => {
                let class = java_type_name(&a.name);
                let item = java_value(cx, &a.element, &Occurrence::Required, &[], true, 1);
                test(&mut w, format!("{}RoundtripEmpty", camel(&a.name)), vec![
                    format!("{class} original = new {class}(List.of());"),
                    format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                ]);
                test(&mut w, format!("{}RoundtripItems", camel(&a.name)), vec![
                    format!("{class} original = new {class}(List.of({item}, {item}));"),
                    format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                ]);
            }
            TypeDef::Map(m) => {
                let class = java_type_name(&m.name);
                test(&mut w, format!("{}RoundtripEmpty", camel(&m.name)), vec![
                    format!("{class} original = new {class}(Map.of());"),
                    format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                ]);
                test(&mut w, format!("{}RoundtripEntries", camel(&m.name)), vec![
                    format!("{class} original = {};", java_table_value(cx, m, true, 1)),
                    format!("roundtrip(original, original.encode(), {class}::decode, {class}.class);"),
                ]);
            }
        }
    }
    w.dedent();
    w.line("}");
    let s = w.finish();
    format!("{}\n", s.trim_end())
}

// ── Test values ───────────────────────────────────────────────────────────────

/// Nesting depth past which a named type's test value is not built further.
const MAX_FIXTURE_DEPTH: usize = 4;

fn java_struct_value(cx: &Cx, s: &StructDef, explicit: bool, depth: usize) -> String {
    let mut args: Vec<String> = s.fields.iter()
        .map(|f| java_value(cx, &f.ty, &f.occurrence, &f.constraints, explicit, depth + 1))
        .collect();
    // The other entries: one when explicit or when the map needs one.
    if let Some(r) = &s.rest {
        args.push(if explicit || matches!(r.occurrence, Occurrence::OneOrMore { .. }) {
            let v = java_value(cx, &r.val_ty, &Occurrence::Required, &[], explicit, depth + 1);
            format!("Map.of(\"other-key\", {v})")
        } else {
            "Map.of()".into()
        });
    }
    format!("new {}({})", java_type_name(&s.name), args.join(", "))
}

fn java_variant_value(cx: &Cx, e: &EnumDef, v: &EnumVariant, depth: usize) -> String {
    let class = java_type_name(&e.name);
    if e.variants.iter().all(|v| !has_payload(v)) {
        return format!("{class}.{}", java_enum_constant(&v.name));
    }
    let rec = java_variant_record(&e.name, &v.name);
    if !has_payload(v) {
        return format!("new {class}.{rec}()");
    }
    format!("new {class}.{rec}({})", java_value(cx, &v.ty, &Occurrence::Required, &[], true, depth + 1))
}

/// A valid test value: the literal a type is pinned to, a value inside its
/// constraints, or a default/explicit value.
fn java_value(cx: &Cx, ty: &TypeRef, occ: &Occurrence, constraints: &[Constraint], explicit: bool, depth: usize) -> String {
    match occ {
        Occurrence::Optional if !explicit => return "null".into(),
        Occurrence::ZeroOrMore { .. } | Occurrence::Bounded { .. } => return "List.of()".into(),
        Occurrence::OneOrMore { .. } => return format!("List.of({})", java_value(cx, ty, &Occurrence::Required, constraints, explicit, depth)),
        _ => {}
    }
    if let Some(p) = base_primitive(ty) {
        if let Some(l) = constraints.iter().find_map(|c| match c {
            Constraint::Eq(v) if literal_fits(v, p) => java_literal(v, p),
            _ => None,
        }) {
            return l;
        }
        for c in constraints {
            match (c, p) {
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Tstr) => return java_str(&"a".repeat(*n)),
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Bstr | Primitive::Any) => return format!("new byte[{n}]"),
                (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) => return long_lit(*lo as i128),
                (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) => return long_lit(*lo as i128),
                // Only an upper bound: the largest value under it.
                (Constraint::ValueRangeInt { min: None, max: Some(hi), inclusive }, Primitive::Int) => {
                    return long_lit(if *inclusive { *hi as i128 } else { *hi as i128 - 1 });
                }
                _ => {}
            }
        }
    }
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => (if explicit { "42L" } else { "0L" }).into(),
            Primitive::Int => (if explicit { "-1L" } else { "0L" }).into(),
            Primitive::Bool => explicit.to_string(),
            // 1.5 is exact as a float32, so it reads back unchanged from any decoder.
            Primitive::Float16 | Primitive::Float32 => (if explicit { "1.5f" } else { "0.0f" }).into(),
            Primitive::Float64 | Primitive::Float => (if explicit { "1.5" } else { "0.0" }).into(),
            Primitive::Tstr => (if explicit { "\"hello\"" } else { "\"\"" }).into(),
            Primitive::Bstr | Primitive::Any => (if explicit { "new byte[] {1, 2}" } else { "new byte[0]" }).into(),
            Primitive::Null | Primitive::Undefined => "Cddl.Null.NULL".into(),
        },
        TypeRef::Tagged(_, inner) => java_value(cx, inner, &Occurrence::Required, constraints, explicit, depth),
        // A choice's first alternative (null isn't a value of it).
        TypeRef::Choice(cs) => match cs.first() {
            Some(first) => java_value(cx, first, &Occurrence::Required, &[], explicit, depth),
            None => "null".into(),
        },
        TypeRef::Named(n) => {
            let class = java_type_name(n);
            match cx.module.get(n) {
                _ if depth > MAX_FIXTURE_DEPTH => "null".into(),
                Some(TypeDef::Struct(s)) => java_struct_value(cx, s, explicit, depth),
                Some(TypeDef::Alias(a)) => format!("new {class}({})", java_value(cx, &a.ty, &Occurrence::Required, &a.constraints, explicit, depth + 1)),
                Some(TypeDef::Enum(e)) => e.variants.first().map(|v| java_variant_value(cx, e, v, depth)).unwrap_or_else(|| "null".into()),
                Some(TypeDef::Array(_)) => format!("new {class}(List.of())"),
                Some(TypeDef::Map(m)) => java_table_value(cx, m, explicit, depth),
                None => "null".into(),
            }
        }
    }
}

/// A table with one entry.
fn java_table_value(cx: &Cx, m: &MapDef, explicit: bool, depth: usize) -> String {
    let key = java_value(cx, &m.key_ty, &Occurrence::Required, &[], explicit, depth + 1);
    let val = java_value(cx, &m.val_ty, &Occurrence::Required, &[], explicit, depth + 1);
    format!("new {}(Map.of({key}, {val}))", java_type_name(&m.name))
}

/// A value just outside an alias's constraints, and the constraint it breaks.
fn java_invalid_value<'a>(constraints: &'a [Constraint], ty: &TypeRef) -> Option<(String, &'a Constraint)> {
    let p = base_primitive(ty)?;
    for c in constraints {
        let bad = match (c, p) {
            (Constraint::SizeExact(n), Primitive::Tstr) => java_str(&"a".repeat(n + 1)),
            (Constraint::SizeExact(n), Primitive::Bstr | Primitive::Any) => format!("new byte[{}]", n + 1),
            (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) if *lo > i64::MIN => long_lit(*lo as i128 - 1),
            (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) if *lo > 0 => long_lit(*lo as i128 - 1),
            (Constraint::ValueRangeUint { max: Some(hi), .. }, Primitive::Uint) if *hi < i64::MAX as u64 => long_lit(*hi as i128 + 1),
            _ => continue,
        };
        return Some((bad, c));
    }
    None
}
