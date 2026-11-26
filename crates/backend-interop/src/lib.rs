/// Cross-language interoperability test generation for cddlc.
///
/// Generates canonical CBOR test vectors using the Rust/minicbor reference
/// encoder, then emits test harnesses for each target language that:
///   1. Decode the canonical hex bytes
///   2. Assert field values match
///   3. Re-encode and assert the hex matches (roundtrip)
///
/// Output:
///   <schema>-interop/
///     vectors.json
///     vectors.cbor
///     rust/interop_test.rs
///     c/test_interop.c
///     cpp/test_interop.cpp
///     csharp/InteropTests.cs
///     nodejs/interop.test.ts
///     python/test_interop.py

use cddlc_codegen::{
    to_pascal_case, to_snake_case,
    CodegenOptions, GeneratedFile, IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, FieldDef, IrModule, LiteralValue,
    Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};
use minicbor::encode::Encoder;

// ── Public API ────────────────────────────────────────────────────────────────

/// Which language harnesses to emit.
#[derive(Debug, Clone, Default)]
pub struct InteropLangs {
    pub rust:   bool,
    pub c:      bool,
    pub cpp:    bool,
    pub csharp: bool,
    pub nodejs: bool,
    pub python: bool,
    pub go:     bool,
    pub java:   bool,
    pub lua:    bool,
}

impl InteropLangs {
    /// Enable all languages.
    pub fn all() -> Self {
        Self { rust: true, c: true, cpp: true, csharp: true, nodejs: true, python: true, go: true, java: true, lua: true }
    }
}

/// Generate interop test vectors and language harnesses.
pub fn generate(
    module: &IrModule,
    langs:  &InteropLangs,
    opts:   &CodegenOptions,
) -> Vec<GeneratedFile> {
    let name      = module.name.as_str();
    let dir       = format!("{name}-interop");
    let vectors   = collect_vectors(module, opts);

    let mut files = vec![
        GeneratedFile { path: format!("{dir}/vectors.json"),  content: emit_json(&vectors, name, opts) },
        GeneratedFile { path: format!("{dir}/vectors.cbor"),  content: emit_cbor_hex(&vectors, opts) },
    ];

    if langs.rust {
        // Rust renames a rule whose type would hide one the crate uses
        // (`result` → `<module>-result`, dgtlrift/cddlc#191).
        let renamed = cddlc_codegen::rename_types(module, &cddlc_codegen::rust_reserved_renames(module));
        let rust_vectors = collect_vectors(&renamed, opts);
        files.push(GeneratedFile { path: format!("{dir}/rust/interop_test.rs"), content: emit_rust_harness(&rust_vectors, name, opts) });
    }
    if langs.c {
        // C renames a rule named like a C library type (`mode` → `<module>-mode`,
        // dgtlrift/cddlc#154); its harness uses the same names.
        let renamed = cddlc_codegen::rename_types(module, &cddlc_codegen::c_reserved_renames(module));
        let c_vectors = collect_vectors(&renamed, opts);
        files.push(GeneratedFile { path: format!("{dir}/c/test_interop.c"), content: emit_c_harness(&c_vectors, name, opts) });
    }
    if langs.cpp    { files.push(GeneratedFile { path: format!("{dir}/cpp/test_interop.cpp"),    content: emit_cpp_harness(&vectors, name, opts) }); }
    if langs.csharp {
        // C# renames a rule whose class would clash with its converter's
        // members (`write`, dgtlrift/cddlc#191).
        let renamed = cddlc_codegen::rename_types(module, &cddlc_codegen::csharp_reserved_renames(module));
        let cs_vectors = collect_vectors(&renamed, opts);
        files.push(GeneratedFile { path: format!("{dir}/csharp/InteropTests.cs"), content: emit_csharp_harness(&cs_vectors, name, opts) });
    }
    if langs.nodejs { files.push(GeneratedFile { path: format!("{dir}/nodejs/interop.test.ts"),  content: emit_nodejs_harness(&vectors, name, opts) }); }
    if langs.python { files.push(GeneratedFile { path: format!("{dir}/python/test_interop.py"),  content: emit_python_harness(&vectors, name, opts) }); }
    if langs.go     { files.push(GeneratedFile { path: format!("{dir}/go/interop_test.go"),      content: emit_go_harness(&vectors, name, opts) }); }
    if langs.java   { files.push(GeneratedFile { path: format!("{dir}/java/InteropTest.java"),   content: emit_java_harness(&vectors, name, opts) }); }
    if langs.lua    { files.push(GeneratedFile { path: format!("{dir}/lua/interop_spec.lua"),    content: emit_lua_harness(&vectors, name, opts) }); }

    files
}

/// Public wrapper for tests.
pub fn collect_vectors_pub(module: &IrModule, opts: &CodegenOptions) -> Vec<TestVector> {
    collect_vectors(module, opts)
}

// ── Test vector model ─────────────────────────────────────────────────────────

/// A single test case: a CBOR-encoded value with its expected field contents.
/// What kind of definition a vector encodes a value of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorShape {
    Struct,
    Enum,
    Alias,
    Array,
    /// An array of a repeated group (dgtlrift/cddlc#90).
    FlatArray,
    Map,
}

#[derive(Debug, Clone)]
pub struct TestVector {
    pub type_name:   String,
    /// The type's CDDL name (`device-id`), for harnesses that derive their own
    /// identifiers from it (C: `device_id_t`).
    pub cddl_name:   String,
    pub shape:       VectorShape,
    pub description: String,
    /// Canonical CBOR bytes (generated by minicbor reference encoder)
    pub cbor_bytes:  Vec<u8>,
    /// Human-readable field values for assertions
    pub fields:      Vec<FieldValue>,
}

impl TestVector {
    pub fn cbor_hex(&self) -> String {
        self.cbor_bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone)]
pub struct FieldValue {
    pub name:  String,
    pub value: FieldVal,
}

#[derive(Debug, Clone)]
pub enum FieldVal {
    Uint(u64),
    Int(i64),
    /// `single` writes it as a CBOR float32 (a `float32`/`float16` member);
    /// otherwise float64.
    Float { value: f64, single: bool },
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Null,
    /// `kind` is the variant's type name, `text` its CDDL string on the wire.
    Enum { kind: String, text: String },
}

// ── Vector collection ─────────────────────────────────────────────────────────

fn collect_vectors(module: &IrModule, opts: &CodegenOptions) -> Vec<TestVector> {
    let mut vecs = Vec::new();
    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                if let Some(v) = encode_struct(s, module, "default values", false, opts) { vecs.push(v); }
                if let Some(v) = encode_struct(s, module, "explicit values", true, opts) { vecs.push(v); }
            }
            TypeDef::Enum(e) => {
                for v in &e.variants {
                    if let Some(vec) = encode_enum_variant(e, v, module, opts) { vecs.push(vec); }
                }
            }
            TypeDef::Alias(a) => {
                if let Some(v) = encode_alias(a, module, opts) { vecs.push(v); }
            }
            TypeDef::Array(a) => {
                if let Some(v) = encode_array_empty(a, module, opts) { vecs.push(v); }
            }
            TypeDef::Map(m) => {
                if let Some(v) = encode_map_sample(m, module, opts) { vecs.push(v); }
            }
        }
    }
    vecs
}

// ── Struct encoder ────────────────────────────────────────────────────────────

type Enc<'a> = Encoder<&'a mut Vec<u8>>;

/// Nesting depth past which a vector can't be built (a recursive schema).
const MAX_SAMPLE_DEPTH: usize = 4;

fn encode_struct(
    s:        &StructDef,
    module:   &IrModule,
    desc:     &str,
    explicit: bool,
    opts:     &CodegenOptions,
) -> Option<TestVector> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    let field_values = write_struct(&mut enc, s, module, explicit, opts, 0)?;
    drop(enc);
    Some(TestVector {
        cddl_name:   s.name.to_string(),
        shape:       VectorShape::Struct,
        type_name:   to_pascal_case(&s.name),
        description: desc.into(),
        cbor_bytes:  buf,
        fields:      field_values,
    })
}

/// Writes a struct's map, or a record's array (its required members) and returns the members a
/// harness can assert on: those of a primitive type. Members of a named, tagged
/// or choice type are written as real values of that type but not asserted.
fn write_struct(
    enc:      &mut Enc,
    s:        &StructDef,
    module:   &IrModule,
    explicit: bool,
    opts:     &CodegenOptions,
    depth:    usize,
) -> Option<Vec<FieldValue>> {
    let mut fields: Vec<&FieldDef> = s.fields.iter()
        .filter(|f| !matches!(f.occurrence, Occurrence::Optional))
        .collect();
    if opts.dcbor {
        fields.sort_by(|a, b| cddlc_codegen::dcbor_key_order(a, b));
    }
    if let Some(tag) = s.tagged {
        enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
    }
    // A record (RFC 8610 §3.4) is an array of the members' values, in order.
    let record = s.encoding == cddlc_ir::StructEncoding::Array;
    // A map's other entries (dgtlrift/cddlc#82): one in the explicit vector,
    // and in the default one when the map needs at least one.
    let extra = match &s.rest {
        Some(r) if explicit || matches!(r.occurrence, Occurrence::OneOrMore { .. }) => 1,
        _ => 0,
    };
    if record {
        // A repeated member's items go inline: as many as it needs
        // (dgtlrift/cddlc#134).
        let items: u64 = fields.iter()
            .map(|f| if f.occurrence.is_repeated() { cddlc_codegen::repeat_bounds(&f.occurrence).0 } else { 1 })
            .sum();
        enc.array(items).ok()?;
    } else {
        enc.map(fields.len() as u64 + extra).ok()?;
    }

    let mut field_values = Vec::new();
    for f in fields {
        // The member's key (an integer key is an integer; dgtlrift/cddlc#176).
        if !record {
            match &f.key {
                cddlc_ir::FieldKey::Text(t) => { enc.str(t.as_str()).ok()?; }
                cddlc_ir::FieldKey::Uint(n) => { enc.u64(*n).ok()?; }
                cddlc_ir::FieldKey::Int(n) => { enc.i64(*n).ok()?; }
            }
        }
        if record && f.occurrence.is_repeated() {
            for _ in 0..cddlc_codegen::repeat_bounds(&f.occurrence).0 {
                match member_value(f, explicit) {
                    Some(val) => encode_field_val(enc, &val)?,
                    None => write_sample(enc, &f.ty, module, explicit, opts, depth + 1)?,
                }
            }
            continue;
        }
        match member_value(f, explicit) {
            Some(val) => {
                encode_field_val(enc, &val)?;
                field_values.push(FieldValue { name: to_snake_case(&f.name), value: val });
            }
            None => write_sample(enc, &f.ty, module, explicit, opts, depth + 1)?,
        }
    }
    if let (1, Some(r)) = (extra, &s.rest) {
        // A key no member has: 1000 when the key can be an integer (`* int`,
        // `* label` with `label = int / tstr`; dgtlrift/cddlc#136), else text.
        if rest_key_takes_int(&r.key_ty, module, 0) {
            enc.u64(1000).ok()?;
        } else {
            enc.str("other-key").ok()?;
        }
        write_sample(enc, &r.val_ty, module, explicit, opts, depth + 1)?;
    }
    Some(field_values)
}

/// Whether a map's other entries' key type has an integer alternative that
/// takes any integer (not a literal).
fn rest_key_takes_int(ty: &TypeRef, module: &IrModule, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match ty {
        TypeRef::Primitive(cddlc_ir::Primitive::Int | cddlc_ir::Primitive::Uint) => true,
        TypeRef::Named(n) => match module.get(n) {
            Some(TypeDef::Alias(a)) => a.constraints.is_empty() && rest_key_takes_int(&a.ty, module, depth + 1),
            Some(TypeDef::Enum(e)) => e.variants.iter().any(|v| v.constraints.is_empty() && rest_key_takes_int(&v.ty, module, depth + 1)),
            _ => false,
        },
        _ => false,
    }
}

/// The value of a primitive-typed member: its literal when it is pinned to one
/// (`always: true`), a value inside its constraints, or the default/explicit
/// value. `None` for members of a named, tagged or choice type.
fn member_value(f: &FieldDef, explicit: bool) -> Option<FieldVal> {
    let TypeRef::Primitive(_) = &f.ty else { return None };
    if let Some(lit) = f.constraints.iter().find_map(|c| match c {
        Constraint::Eq(v) => literal_val(v),
        _ => None,
    }) {
        return Some(lit);
    }
    if !f.constraints.is_empty() {
        return Some(valid_alias_val(&f.constraints, &f.ty));
    }
    Some(if explicit { explicit_field_val(&f.ty) } else { default_field_val(&f.ty) })
}

fn literal_val(v: &LiteralValue) -> Option<FieldVal> {
    Some(match v {
        LiteralValue::Bool(b)  => FieldVal::Bool(*b),
        LiteralValue::Uint(n)  => FieldVal::Uint(*n),
        LiteralValue::Int(n)   => FieldVal::Int(*n),
        LiteralValue::Float(f) => FieldVal::Float { value: *f, single: false },
        LiteralValue::Text(t)  => FieldVal::Text(t.clone()),
        LiteralValue::Bytes(b) => FieldVal::Bytes(b.clone()),
        LiteralValue::Null     => FieldVal::Null,
    })
}

/// Writes a valid value of `ty`. A named type is built from its definition:
/// a struct's map, an alias's (tagged, constrained) inner value, an enum's first
/// variant, an array with its minimum number of elements. `None` when no value
/// can be built (a bare map type, or nesting past `MAX_SAMPLE_DEPTH`).
fn write_sample(
    enc:      &mut Enc,
    ty:       &TypeRef,
    module:   &IrModule,
    explicit: bool,
    opts:     &CodegenOptions,
    depth:    usize,
) -> Option<()> {
    if depth > MAX_SAMPLE_DEPTH {
        return None;
    }
    match ty {
        TypeRef::Primitive(_) => {
            let val = if explicit { explicit_field_val(ty) } else { default_field_val(ty) };
            encode_field_val(enc, &val)
        }
        TypeRef::Tagged(tag, inner) => {
            enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
            write_sample(enc, inner, module, explicit, opts, depth)
        }
        TypeRef::Choice(alts) => write_sample(enc, alts.first()?, module, explicit, opts, depth),
        TypeRef::Named(n) => match module.get(n)? {
            TypeDef::Struct(s) => write_struct(enc, s, module, explicit, opts, depth).map(|_| ()),
            TypeDef::Alias(a) => write_alias(enc, a, module, opts, depth),
            TypeDef::Enum(e) => {
                if let Some(tag) = e.tagged {
                    enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
                }
                write_variant(enc, e.variants.first()?, module, opts, depth)
            }
            TypeDef::Array(a) => write_array(enc, a, module, explicit, opts, depth),
            TypeDef::Map(m) => write_map(enc, m, module, explicit, opts, depth),
        },
    }
}

/// Writes an alias's value: its tag, then a value inside its constraints.
fn write_alias(enc: &mut Enc, a: &AliasDef, module: &IrModule, opts: &CodegenOptions, depth: usize) -> Option<()> {
    if let Some(tag) = a.tagged {
        enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
    }
    match &a.ty {
        TypeRef::Primitive(_) => encode_field_val(enc, &valid_alias_val(&a.constraints, &a.ty)),
        other => write_sample(enc, other, module, false, opts, depth + 1),
    }
}

/// Writes an array with the fewest elements its occurrence allows.
fn write_array(
    enc:      &mut Enc,
    a:        &ArrayDef,
    module:   &IrModule,
    explicit: bool,
    opts:     &CodegenOptions,
    depth:    usize,
) -> Option<()> {
    if let Some(tag) = a.tagged {
        enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
    }
    let min = match &a.occurrence {
        Occurrence::OneOrMore { .. } => 1,
        Occurrence::Bounded { min, .. } => *min as u64,
        _ => 0,
    };
    if a.flat {
        // A repeated group (dgtlrift/cddlc#90): each record's members, inline.
        let width = match &a.element {
            TypeRef::Named(n) => match module.get(n)? { TypeDef::Struct(s) => s.fields.len() as u64, _ => return None },
            _ => return None,
        };
        enc.array(min * width).ok()?;
        for _ in 0..min {
            let mut item = Vec::new();
            write_sample(&mut Encoder::new(&mut item), &a.element, module, explicit, opts, depth + 1)?;
            // Drop the record's own array header (a one-byte head: fewer than 24 members).
            if !(0x80..0x98).contains(item.first()?) { return None; }
            enc.writer_mut().extend_from_slice(&item[1..]);
        }
        return Some(());
    }
    enc.array(min).ok()?;
    for _ in 0..min {
        write_sample(enc, &a.element, module, explicit, opts, depth + 1)?;
    }
    Some(())
}

/// Writes a table (`{ * tstr => uint }`) with one entry, or none when its
/// occurrence allows at most zero (dgtlrift/cddlc#82).
fn write_map(
    enc:      &mut Enc,
    m:        &cddlc_ir::MapDef,
    module:   &IrModule,
    explicit: bool,
    opts:     &CodegenOptions,
    depth:    usize,
) -> Option<()> {
    if let Some(tag) = m.tagged {
        enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
    }
    let entries = match &m.occurrence {
        // One sample key can't make more than one distinct entry.
        Occurrence::Bounded { min, .. } if *min > 1 => return None,
        Occurrence::Bounded { max: Some(0), .. } => 0,
        _ => 1,
    };
    enc.map(entries).ok()?;
    for _ in 0..entries {
        write_sample(enc, &m.key_ty, module, explicit, opts, depth + 1)?;
        write_sample(enc, &m.val_ty, module, explicit, opts, depth + 1)?;
    }
    Some(())
}

/// Writes one enum variant's value: its text or integer literal, or a value of
/// its type.
fn write_variant(enc: &mut Enc, v: &EnumVariant, module: &IrModule, opts: &CodegenOptions, depth: usize) -> Option<()> {
    let int_literal = v.constraints.iter().find_map(|c| match c {
        Constraint::Eq(LiteralValue::Uint(n)) => Some(FieldVal::Uint(*n)),
        Constraint::Eq(LiteralValue::Int(n))  => Some(FieldVal::Int(*n)),
        _ => None,
    });
    match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) => {
            enc.str(&cddlc_codegen::enum_variant_text(v)).ok().map(|_| ())
        }
        TypeRef::Primitive(Primitive::Null) => enc.null().ok().map(|_| ()),
        TypeRef::Primitive(Primitive::Uint | Primitive::Int) => {
            encode_field_val(enc, &int_literal.unwrap_or(FieldVal::Uint(0)))
        }
        TypeRef::Primitive(Primitive::Bool) => enc.bool(false).ok().map(|_| ()),
        other => write_sample(enc, other, module, false, opts, depth + 1),
    }
}

// ── Enum variant encoder ──────────────────────────────────────────────────────

fn encode_enum_variant(
    e:      &EnumDef,
    v:      &EnumVariant,
    module: &IrModule,
    opts:   &CodegenOptions,
) -> Option<TestVector> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);

    if let Some(tag) = e.tagged {
        enc.tag(minicbor::data::Tag::new(tag.0)).ok()?;
    }
    write_variant(&mut enc, v, module, opts, 0)?;
    drop(enc);

    // Only a text variant is asserted (by its kind); an enum has no `kind`
    // member a harness could compare an integer or boolean variant against.
    let fields = match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) => vec![FieldValue {
            name:  "kind".into(),
            value: FieldVal::Enum {
                kind: to_pascal_case(&v.name),
                text: cddlc_codegen::enum_variant_text(v),
            },
        }],
        _ => vec![],
    };
    Some(TestVector {
        cddl_name:   e.name.to_string(),
        shape:       VectorShape::Enum,
        type_name:   to_pascal_case(&e.name),
        description: format!("variant {}", to_pascal_case(&v.name)),
        cbor_bytes:  buf,
        fields,
    })
}

// ── Alias encoder ─────────────────────────────────────────────────────────────

fn encode_alias(a: &AliasDef, module: &IrModule, opts: &CodegenOptions) -> Option<TestVector> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    write_alias(&mut enc, a, module, opts, 0)?;
    drop(enc);

    // A primitive alias is asserted through its `value`; an alias of a named
    // type is only decoded.
    let fields = match &a.ty {
        TypeRef::Primitive(_) => vec![FieldValue {
            name:  "value".into(),
            value: valid_alias_val(&a.constraints, &a.ty),
        }],
        _ => vec![],
    };
    Some(TestVector {
        cddl_name:   a.name.to_string(),
        shape:       VectorShape::Alias,
        type_name:   to_pascal_case(&a.name),
        description: "valid value".into(),
        cbor_bytes:  buf,
        fields,
    })
}

// ── Array encoder ─────────────────────────────────────────────────────────────

fn encode_array_empty(a: &ArrayDef, module: &IrModule, opts: &CodegenOptions) -> Option<TestVector> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    write_array(&mut enc, a, module, false, opts, 0)?;
    drop(enc);
    Some(TestVector {
        cddl_name:   a.name.to_string(),
        shape:       if a.flat { VectorShape::FlatArray } else { VectorShape::Array },
        type_name:   to_pascal_case(&a.name),
        description: "minimal array".into(),
        cbor_bytes:  buf,
        fields:      vec![],
    })
}

fn encode_map_sample(m: &cddlc_ir::MapDef, module: &IrModule, opts: &CodegenOptions) -> Option<TestVector> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    write_map(&mut enc, m, module, false, opts, 0)?;
    drop(enc);
    Some(TestVector {
        cddl_name:   m.name.to_string(),
        shape:       VectorShape::Map,
        type_name:   to_pascal_case(&m.name),
        description: "one entry".into(),
        cbor_bytes:  buf,
        fields:      vec![],
    })
}

// ── Value helpers ─────────────────────────────────────────────────────────────

fn encode_field_val(enc: &mut Encoder<&mut Vec<u8>>, val: &FieldVal) -> Option<()> {
    match val {
        FieldVal::Uint(n)    => enc.u64(*n).ok().map(|_| ()),
        FieldVal::Int(n)     => enc.i64(*n).ok().map(|_| ()),
        FieldVal::Float { value, single: true }  => enc.f32(*value as f32).ok().map(|_| ()),
        FieldVal::Float { value, single: false } => enc.f64(*value).ok().map(|_| ()),
        FieldVal::Bool(b)    => enc.bool(*b).ok().map(|_| ()),
        FieldVal::Text(s)    => enc.str(s).ok().map(|_| ()),
        FieldVal::Bytes(b)   => enc.bytes(b).ok().map(|_| ()),
        FieldVal::Null       => enc.null().ok().map(|_| ()),
        FieldVal::Enum { text, .. } => enc.str(text).ok().map(|_| ()),
    }
}

fn default_field_val(ty: &TypeRef) -> FieldVal {
    match ty {
        TypeRef::Primitive(Primitive::Uint)  => FieldVal::Uint(0),
        TypeRef::Primitive(Primitive::Int)   => FieldVal::Int(0),
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => FieldVal::Float { value: 0.0, single: true },
        TypeRef::Primitive(Primitive::Float | Primitive::Float64) => FieldVal::Float { value: 0.0, single: false },
        TypeRef::Primitive(Primitive::Bool)  => FieldVal::Bool(false),
        TypeRef::Primitive(Primitive::Tstr)  => FieldVal::Text(String::new()),
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => FieldVal::Bytes(vec![]),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => FieldVal::Null,
        TypeRef::Named(_) | TypeRef::Tagged(_, _) | TypeRef::Choice(_) => FieldVal::Uint(0),
    }
}

fn explicit_field_val(ty: &TypeRef) -> FieldVal {
    match ty {
        TypeRef::Primitive(Primitive::Uint)  => FieldVal::Uint(42),
        TypeRef::Primitive(Primitive::Int)   => FieldVal::Int(-1),
        // 1.5 is exact as a float32, so every decoder reads back the same value.
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => FieldVal::Float { value: 1.5, single: true },
        TypeRef::Primitive(Primitive::Float | Primitive::Float64) => FieldVal::Float { value: 3.14, single: false },
        TypeRef::Primitive(Primitive::Bool)  => FieldVal::Bool(true),
        TypeRef::Primitive(Primitive::Tstr)  => FieldVal::Text("hello".into()),
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => FieldVal::Bytes(vec![0x01, 0x02]),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => FieldVal::Null,
        TypeRef::Named(_) | TypeRef::Tagged(_, _) | TypeRef::Choice(_) => FieldVal::Uint(42),
    }
}

fn valid_alias_val(constraints: &[Constraint], ty: &TypeRef) -> FieldVal {
    // A value pinned to a literal (`40 .plus 2`; dgtlrift/cddlc#86).
    if let Some(v) = constraints.iter().find_map(|c| match c {
        Constraint::Eq(lit) => literal_val(lit),
        _ => None,
    }) {
        return v;
    }
    for c in constraints {
        match c {
            Constraint::SizeExact(n) => match ty {
                TypeRef::Primitive(Primitive::Tstr) =>
                    return FieldVal::Text("a".repeat(*n)),
                TypeRef::Primitive(Primitive::Bstr) =>
                    return FieldVal::Bytes(vec![0u8; *n]),
                _ => {}
            },
            // The shortest size allowed: `tstr .size (1..16)` is "a".
            Constraint::SizeRange { min, .. } => {
                let n = min.unwrap_or(0);
                match ty {
                    TypeRef::Primitive(Primitive::Tstr) => return FieldVal::Text("a".repeat(n)),
                    TypeRef::Primitive(Primitive::Bstr) => return FieldVal::Bytes(vec![0u8; n]),
                    _ => {}
                }
            }
            Constraint::ValueRangeInt { min, .. } => {
                if let Some(lo) = min { return FieldVal::Int(*lo); }
            }
            Constraint::ValueRangeUint { min, .. } => {
                if let Some(lo) = min { return FieldVal::Uint(*lo); }
            }
            _ => {}
        }
    }
    default_field_val(ty)
}

// ── JSON vector file ──────────────────────────────────────────────────────────

fn emit_json(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    // JSON has no comments: the header is the "//" member, which readers skip.
    s.push_str(&format!("  \"//\": [\n{}\n  ],\n", cddlc_codegen::header_json(opts, 4)));
    s.push_str(&format!("  \"schema\": \"{schema}\",\n"));
    s.push_str("  \"generated_by\": \"cddlc\",\n");
    s.push_str("  \"version\": \"1\",\n");
    s.push_str("  \"vectors\": [\n");

    for (i, v) in vectors.iter().enumerate() {
        s.push_str("    {\n");
        s.push_str(&format!("      \"type\": \"{}\",\n", v.type_name));
        s.push_str(&format!("      \"description\": \"{}\",\n", v.description));
        s.push_str(&format!("      \"cbor_hex\": \"{}\",\n", v.cbor_hex()));
        s.push_str("      \"fields\": {");
        let fields: Vec<String> = v.fields.iter().map(|f| {
            format!("\"{}\": {}", f.name, json_value(&f.value))
        }).collect();
        s.push_str(&fields.join(", "));
        s.push_str("}\n");
        if i < vectors.len() - 1 {
            s.push_str("    },\n");
        } else {
            s.push_str("    }\n");
        }
    }

    s.push_str("  ]\n");
    s.push_str("}\n");
    s
}

fn json_value(v: &FieldVal) -> String {
    match v {
        FieldVal::Uint(n)     => n.to_string(),
        FieldVal::Int(n)      => n.to_string(),
        FieldVal::Float { value: f, .. }    => {
            if f.fract() == 0.0 { format!("{f:.1}") } else { format!("{f}") }
        }
        FieldVal::Bool(b)     => b.to_string(),
        FieldVal::Text(s)     => format!("\"{}\"", s.replace('"', "\\\"")),
        FieldVal::Bytes(b)    => {
            let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
            format!("\"0x{hex}\"")
        }
        FieldVal::Null        => "null".into(),
        FieldVal::Enum { kind, .. } => format!("\"{}\"", kind),
    }
}

// ── CBOR vector file (as hex string for easy storage) ────────────────────────

fn emit_cbor_hex(vectors: &[TestVector], opts: &CodegenOptions) -> String {
    // Encode the entire vectors array as CBOR, return as hex string
    // (stored as text file containing hex for portability)
    let mut all = String::new();
    all.push_str(&cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Hash));
    all.push_str("# CBOR test vectors (one hex line per vector)\n");
    for v in vectors {
        all.push_str(&format!("{}:{}\n", v.type_name, v.cbor_hex()));
    }
    all
}

// ── Rust harness ──────────────────────────────────────────────────────────────

fn emit_rust_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    let crate_name = format!("{}_cbor", to_snake_case(&schema));

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (Rust)");
    w.line(&format!("// Copy into the generated crate's tests/ (`{schema}-cbor/tests/`) and run `cargo test`."));
    w.line("// Decodes each canonical CBOR vector, checks its fields, and re-encodes it to the same bytes.");
    w.blank();
    w.line("#![allow(clippy::float_cmp, clippy::unnecessary_cast)]");
    w.blank();
    w.line(&format!("use {crate_name}::*;"));
    w.blank();

    for v in vectors {
        let test_name = format!(
            "interop_{}_{}",
            to_snake_case(&v.type_name),
            to_snake_case(&v.description.replace(' ', "_"))
        );
        let bytes = v.cbor_bytes.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ");
        w.line("#[test]");
        w.line(&format!("fn {test_name}() {{"));
        w.indent();
        w.line(&format!("let bytes: &[u8] = &[{bytes}];"));
        // An array of a repeated group has its own functions (its members sit inline).
        let flat = v.shape == VectorShape::FlatArray;
        let snake = to_snake_case(&v.cddl_name);
        if flat {
            w.line(&format!(
                "let decoded: {} = decode_{snake}(&mut minicbor::Decoder::new(bytes), &mut ()).expect(\"decode\");",
                v.type_name
            ));
        } else {
            w.line(&format!(
                "let decoded: {} = minicbor::decode(bytes).expect(\"decode\");",
                v.type_name
            ));
        }
        for f in &v.fields {
            let field = cddlc_codegen::rust_field_ident(&f.name);
            match &f.value {
                // An enum vector names its variant; Rust variants may carry a payload.
                FieldVal::Enum { kind, .. } =>
                    w.line(&format!("assert!(matches!(decoded, {}::{kind} {{ .. }}));", v.type_name)),
                // An alias's value is checked by the re-encode below: a Rust alias is
                // either a `type` or a newtype, so there is no common `value` field.
                _ if v.shape == VectorShape::Alias => {}
                FieldVal::Text(s) =>
                    w.line(&format!("assert_eq!(decoded.{field}, {s:?});")),
                FieldVal::Float { value, .. } =>
                    w.line(&format!("assert!((decoded.{field} as f64 - {value:?}).abs() < 1e-6);")),
                FieldVal::Uint(n) =>
                    w.line(&format!("assert_eq!(decoded.{field} as u64, {n});")),
                FieldVal::Int(n) =>
                    w.line(&format!("assert_eq!(decoded.{field} as i64, {n});")),
                FieldVal::Bool(b) =>
                    w.line(&format!("assert_eq!(decoded.{field}, {b});")),
                FieldVal::Bytes(b) => {
                    let arr = b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ");
                    w.line(&format!("assert_eq!(&decoded.{field}[..], &[{arr}][..]);"));
                }
                // A null member has no value to compare.
                FieldVal::Null => {}
            }
        }
        w.blank();
        w.line("// Re-encode and verify the bytes match");
        if flat {
            w.line("let mut reencoded = Vec::new();");
            w.line(&format!("encode_{snake}(&decoded, &mut minicbor::Encoder::new(&mut reencoded), &mut ()).expect(\"encode\");"));
        } else {
            w.line("let reencoded = minicbor::to_vec(&decoded).expect(\"encode\");");
        };
        w.line("assert_eq!(reencoded, bytes);");
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.finish()
}

// ── C harness ─────────────────────────────────────────────────────────────────

fn emit_c_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line("/* interop test harness (C) */");
    w.line(&format!("#include \"{schema}.h\""));
    w.line("#include <assert.h>");
    w.line("#include <string.h>");
    w.line("#include <stdio.h>");
    w.line("#include <stdlib.h>");
    w.blank();
    // A recursive member's memory (dgtlrift/cddlc#156).
    w.line("static void *interop_alloc(void *ctx, size_t size) { (void)ctx; return calloc(1, size); }");
    w.line("static void interop_release(void *ctx, void *ptr) { (void)ctx; free(ptr); }");
    w.line(&format!("static const {}_callbacks_t interop_cb = {{ .alloc = interop_alloc, .release = interop_release }};", to_snake_case(schema)));
    w.blank();

    // Emit hex arrays
    for (i, v) in vectors.iter().enumerate() {
        w.line(&format!("static const uint8_t vec_{i}[] = {{ {} }};",
            v.cbor_bytes.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ")));
        w.line(&format!("static const size_t  vec_{i}_len = {};", v.cbor_bytes.len()));
    }
    w.blank();

    let pfx = to_snake_case(schema);

    for (i, v) in vectors.iter().enumerate() {
        let fn_name = format!("test_interop_{i}_{}", v.description.replace(' ', "_"));
        w.line(&format!("static void {fn_name}(void) {{"));
        w.indent();
        // C names come from the CDDL name, as the C backend derives them.
        let c_name = to_snake_case(&v.cddl_name);
        w.line(&format!("{c_name}_t decoded = {{0}};"));
        w.line("nanocbor_value_t dec;");
        w.line(&format!("nanocbor_decoder_init(&dec, vec_{i}, vec_{i}_len);"));
        w.line(&format!("assert({pfx}_{c_name}_decode(&dec, &decoded, &interop_cb, NULL) == 0);"));
        w.blank();

        // A plain C alias is a bare typedef with no `value` member; its vector
        // is checked by the decode succeeding.
        let fields: &[FieldValue] = if v.shape == VectorShape::Alias { &[] } else { &v.fields };
        for f in fields {
            let fname = &f.name;
            match &f.value {
                FieldVal::Uint(n) =>
                    w.line(&format!("assert(decoded.{fname} == {n}ULL);")),
                FieldVal::Int(n) =>
                    w.line(&format!("assert(decoded.{fname} == {n}LL);")),
                FieldVal::Bool(b) =>
                    w.line(&format!("assert(decoded.{fname} == {});", if *b { "true" } else { "false" })),
                FieldVal::Float { value: f, .. } =>
                    w.line(&format!("assert((decoded.{fname} - {f}) < 1e-6 && ({f} - decoded.{fname}) < 1e-6);")),
                FieldVal::Text(s) =>
                    w.line(&format!("assert(decoded.{fname}.len == {} && memcmp(decoded.{fname}.ptr, {:?}, {}) == 0);",
                        s.len(), s, s.len())),
                FieldVal::Enum { kind, .. } =>
                    w.line(&format!("assert(decoded.tag == {}_{});",
                        cddlc_codegen::to_screaming_snake(&v.cddl_name),
                        cddlc_codegen::to_screaming_snake(kind))),
                _ => {}
            }
        }

        w.blank();
        w.line(&format!("printf(\"PASS: {}\\n\");", fn_name));
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.line("int main(void) {");
    w.indent();
    for (i, v) in vectors.iter().enumerate() {
        let fn_name = format!("test_interop_{i}_{}", v.description.replace(' ', "_"));
        w.line(&format!("{fn_name}();"));
    }
    w.line("printf(\"All interop tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}

// ── C++ harness ───────────────────────────────────────────────────────────────

fn emit_cpp_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    let ns = to_snake_case(schema);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (C++)");
    w.line(&format!("#include \"{schema}.hpp\""));
    w.line("#include <cassert>");
    w.line("#include <cmath>");
    w.line("#include <cstring>");
    w.line("#include <cstdio>");
    w.line("#include <array>");
    w.blank();
    w.line(&format!("using namespace {ns};"));
    w.blank();

    for (i, v) in vectors.iter().enumerate() {
        let bytes = v.cbor_bytes.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ");
        w.line(&format!("static constexpr std::array<uint8_t, {}> kVec{i} = {{ {bytes} }};", v.cbor_bytes.len()));
    }
    w.blank();

    for (i, v) in vectors.iter().enumerate() {
        let fn_name = format!("test_interop_{i}_{}", v.description.replace(' ', "_"));
        w.line(&format!("static void {fn_name}() {{"));
        w.indent();
        w.line(&format!("uint8_t buf[{}];", v.cbor_bytes.len()));
        w.line(&format!("std::memcpy(buf, kVec{i}.data(), kVec{i}.size());"));
        w.line("nanocbor_value_t dec;");
        w.line(&format!("nanocbor_decoder_init(&dec, buf, kVec{i}.size());"));
        w.line(&format!("{} decoded{{}};", to_pascal_case(&v.type_name)));
        w.line("assert(decode(&dec, decoded) == 0);");
        w.blank();

        for f in &v.fields {
            let fname = &f.name;
            match &f.value {
                FieldVal::Uint(n) =>
                    w.line(&format!("assert(decoded.{fname} == {n}ULL);")),
                FieldVal::Int(n) =>
                    w.line(&format!("assert(decoded.{fname} == {n}LL);")),
                FieldVal::Bool(b) =>
                    w.line(&format!("assert(decoded.{fname} == {});", if *b { "true" } else { "false" })),
                FieldVal::Float { value: f, .. } =>
                    w.line(&format!("assert(std::abs(decoded.{fname} - {f}) < 1e-6);")),
                FieldVal::Text(s) =>
                    w.line(&format!("assert(decoded.{fname} == std::string_view({:?}));", s)),
                // A C++ enum records its variant in `kind`.
                FieldVal::Enum { kind, .. } =>
                    w.line(&format!("assert(decoded.kind == {}::Kind::{kind});", to_pascal_case(&v.type_name))),
                _ => {}
            }
        }

        w.blank();
        w.line(&format!("std::printf(\"PASS: {}\\n\");", fn_name));
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.line("int main() {");
    w.indent();
    for (i, v) in vectors.iter().enumerate() {
        let fn_name = format!("test_interop_{i}_{}", v.description.replace(' ', "_"));
        w.line(&format!("{fn_name}();"));
    }
    w.line("std::printf(\"All C++ interop tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}

// ── C# harness ────────────────────────────────────────────────────────────────

fn emit_csharp_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let ns  = to_pascal_case(schema);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (C#)");
    w.blank();
    w.line("using System;");
    w.line("using System.IO;");
    w.line("using System.Text.Json;");
    w.line("using System.Formats.Cbor;");
    w.line("using Xunit;");
    w.line(&format!("using {ns};"));
    w.blank();
    w.line(&format!("namespace {ns}.Interop;"));
    w.blank();
    w.line("public sealed class InteropTests");
    w.line("{");
    w.indent();

    for (_i, v) in vectors.iter().enumerate() {
        let type_name = to_pascal_case(&v.type_name);
        let method = format!("Interop_{type_name}_{}", v.description.replace(' ', "_"));
        let hex    = v.cbor_hex();

        w.line("[Fact]");
        w.line(&format!("public void {method}()"));
        w.line("{");
        w.indent();
        w.line(&format!("var bytes = Convert.FromHexString(\"{hex}\");"));
        w.line("var reader = new CborReader(bytes);");
        w.line(&format!("var decoded = {type_name}.Decode(reader);"));
        w.blank();

        for f in &v.fields {
            let pname = to_pascal_case(&f.name);
            match &f.value {
                FieldVal::Uint(n)  => w.line(&format!("Assert.Equal({n}UL, decoded.{pname});")),
                FieldVal::Int(n)   => w.line(&format!("Assert.Equal({n}L, decoded.{pname});")),
                FieldVal::Bool(b)  => w.line(&format!("Assert.Equal({}, decoded.{pname});", if *b { "true" } else { "false" })),
                FieldVal::Float { value: f, .. } => w.line(&format!("Assert.True(Math.Abs(decoded.{pname} - {f}) < 1e-6);")),
                FieldVal::Text(s)  => w.line(&format!("Assert.Equal({:?}, decoded.{pname});", s)),
                FieldVal::Enum { kind, .. } => w.line(&format!("Assert.IsType<{type_name}.{kind}>(decoded);")),
                FieldVal::Null     => w.line(&format!("Assert.Null(decoded.{pname});")),
                FieldVal::Bytes(b) => {
                    let hex = b.iter().map(|x| format!("{x:02x}")).collect::<String>();
                    w.line(&format!("Assert.Equal(Convert.FromHexString(\"{hex}\"), decoded.{pname});"));
                }
            }
        }

        w.blank();
        w.line("// Re-encode and verify");
        w.line("var writer = new CborWriter();");
        w.line("decoded.Encode(writer);");
        w.line(&format!("Assert.Equal(\"{hex}\", Convert.ToHexString(writer.Encode()).ToLower());"));
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.dedent();
    w.line("}");
    w.finish()
}

// ── Node.js / TypeScript harness ──────────────────────────────────────────────

fn emit_nodejs_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    // The Node.js backend names the source file after the module, as is.
    let mod_name = schema;
    let mut w    = IndentWriter::new(2);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (TypeScript)");
    w.line(&format!("// Copy into the generated package's tests/ (`{schema}-cbor/tests/`) and run `npm test`."));
    w.blank();

    // Collect unique type names for imports
    let mut types: Vec<String> = vectors.iter()
        .map(|v| to_pascal_case(&v.type_name))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    types.sort();
    let decode_fns: Vec<String> = types.iter()
        .map(|t| format!("decode{t}"))
        .collect();
    let encode_fns: Vec<String> = types.iter()
        .map(|t| format!("encode{t}"))
        .collect();
    let mut imports = types.clone();
    imports.extend(decode_fns.iter().cloned());
    imports.extend(encode_fns.iter().cloned());
    imports.sort();

    w.line(&format!("import {{ {} }} from '../src/{mod_name}.js';", imports.join(", ")));
    w.blank();

    // Helper to convert hex to Uint8Array
    w.line("function fromHex(hex: string): Uint8Array {");
    w.indent();
    w.line("const bytes = new Uint8Array(hex.length / 2);");
    w.line("for (let i = 0; i < hex.length; i += 2) {");
    w.indent();
    w.line("bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);");
    w.dedent();
    w.line("}");
    w.line("return bytes;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("function toHex(bytes: Uint8Array): string {");
    w.indent();
    w.line("return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');");
    w.dedent();
    w.line("}");
    w.blank();

    // Group vectors by type
    for type_name in &types {
        let type_vecs: Vec<&TestVector> = vectors.iter()
            .filter(|v| &to_pascal_case(&v.type_name) == type_name)
            .collect();
        if type_vecs.is_empty() { continue; }

        let decode_fn = format!("decode{type_name}");
        let encode_fn = format!("encode{type_name}");

        w.line(&format!("describe('{type_name} interop', () => {{"));
        w.indent();

        for v in &type_vecs {
            let desc = &v.description;
            let hex  = v.cbor_hex();

            w.line(&format!("test({desc:?}, () => {{"));
            w.indent();
            w.line(&format!("const bytes = fromHex('{hex}');"));
            w.line(&format!("const decoded = {decode_fn}(bytes);"));
            w.blank();

            for f in &v.fields {
                // A TypeScript alias is the value itself, not an object with `value`.
                let fname = if v.shape == VectorShape::Alias { String::new() } else { format!(".{}", f.name) };
                match &f.value {
                    FieldVal::Uint(n) =>
                        w.line(&format!("expect(decoded{fname}).toEqual({n}n);")),
                    FieldVal::Int(n) =>
                        w.line(&format!("expect(decoded{fname}).toEqual({n}n);")),
                    FieldVal::Bool(b) =>
                        w.line(&format!("expect(decoded{fname}).toEqual({b});")),
                    FieldVal::Float { value: f, .. } =>
                        w.line(&format!("expect(Math.abs(Number(decoded{fname}) - {f})).toBeLessThan(1e-4);")),
                    FieldVal::Text(s) =>
                        w.line(&format!("expect(decoded{fname}).toEqual({s:?});")),
                    FieldVal::Enum { kind, .. } =>
                        w.line(&format!("expect(decoded.kind).toEqual({kind:?});")),
                    FieldVal::Null =>
                        w.line(&format!("expect(decoded{fname}).toBeNull();")),
                    FieldVal::Bytes(b) => {
                        let arr = b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
                        w.line(&format!("expect(decoded{fname}).toEqual(new Uint8Array([{arr}]));"));
                    }
                }
            }

            w.blank();
            w.line("// Re-encode and verify");
            w.line(&format!("const reencoded = {encode_fn}(decoded);"));
            w.line(&format!("expect(toHex(reencoded)).toEqual('{hex}');"));
            w.dedent();
            w.line("});");
            w.blank();
        }

        w.dedent();
        w.line("});");
        w.blank();
    }

    w.finish()
}

// ── Python harness ────────────────────────────────────────────────────────────

fn emit_python_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    let mod_name = to_snake_case(&schema);
    let mut w    = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Hash) {
        w.line(&l);
    }
    w.line("# interop test harness (Python)");
    w.line("import cbor2");
    w.line("import pytest");

    // Import all types
    let mut types: Vec<String> = vectors.iter()
        .map(|v| to_pascal_case(&v.type_name))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    types.sort();

    w.line(&format!("from {mod_name} import ("));
    w.indent();
    for t in &types { w.line(&format!("{t},")); }
    w.dedent();
    w.line(")");
    w.blank();

    // Helper
    w.line("def from_hex(h: str) -> bytes:");
    w.indent();
    w.line("return bytes.fromhex(h)");
    w.dedent();
    w.blank();

    // Group by type
    for type_name in &types {
        let type_vecs: Vec<&TestVector> = vectors.iter()
            .filter(|v| &to_pascal_case(&v.type_name) == type_name)
            .collect();
        if type_vecs.is_empty() { continue; }

        w.line(&format!("class Test{type_name}Interop:"));
        w.indent();

        for v in &type_vecs {
            let test_name = format!("test_{}", to_snake_case(&v.description.replace(' ', "_")));
            let hex = v.cbor_hex();

            w.line(&format!("def {test_name}(self):"));
            w.indent();
            w.line(&format!("decoded = {type_name}.decode(from_hex(\"{hex}\"))"));
            w.blank();

            for f in &v.fields {
                let fname = &f.name;
                match &f.value {
                    FieldVal::Uint(n) =>
                        w.line(&format!("assert decoded.{fname} == {n}")),
                    FieldVal::Int(n) =>
                        w.line(&format!("assert decoded.{fname} == {n}")),
                    FieldVal::Bool(b) =>
                        w.line(&format!("assert decoded.{fname} == {}", if *b { "True" } else { "False" })),
                    FieldVal::Float { value: f, .. } =>
                        w.line(&format!("assert abs(decoded.{fname} - {f}) < 1e-4")),
                    FieldVal::Text(s) =>
                        w.line(&format!("assert decoded.{fname} == {s:?}")),
                    FieldVal::Enum { kind, .. } =>
                        w.line(&format!("assert decoded.kind == {kind:?}")),
                    FieldVal::Null =>
                        w.line(&format!("assert decoded.{fname} is None")),
                    FieldVal::Bytes(b) => {
                        let s = b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ");
                        w.line(&format!("assert decoded.{fname} == bytes([{s}])"));
                    }
                }
            }

            w.blank();
            w.line("# Re-encode and verify");
            w.line(&format!("reencoded = decoded.encode()"));
            w.line(&format!("assert reencoded.hex() == \"{hex}\""));
            w.dedent();
            w.blank();
        }

        w.dedent();
    }

    w.finish()
}

// ── Go harness ────────────────────────────────────────────────────────────────

/// Go: copy into the generated package's directory and run `go test`. Each test
/// decodes a vector, checks its primitive members and re-encodes it byte for byte.
fn emit_go_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    use backend_go::{go_field_name, go_kind_const, go_package_name, go_str};
    let pkg = go_package_name(schema, opts.namespace.as_deref());
    let mut w = IndentWriter::new(1);

    w.line("// Code generated by cddlc. DO NOT EDIT.");
    w.blank();
    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (Go)");
    w.line(&format!("// Copy into the generated package (`{schema}-cbor/`) and run `go test`."));
    w.blank();
    w.line(&format!("package {pkg}"));
    w.blank();
    w.line("import (");
    w.line("\t\"encoding/hex\"");
    w.line("\t\"math\"");
    w.line("\t\"testing\"");
    w.line(")");
    w.blank();
    w.line("var _ = math.Abs");
    w.blank();
    w.line("func interopBytes(t *testing.T, s string) []byte {");
    w.line("\tt.Helper()");
    w.line("\tb, err := hex.DecodeString(s)");
    w.line("\tif err != nil {");
    w.line("\t\tt.Fatal(err)");
    w.line("\t}");
    w.line("\treturn b");
    w.line("}");
    w.blank();

    let mut used = std::collections::HashSet::new();
    for v in vectors {
        // The generated package's name for the type (a rule named like a
        // runtime type gets a trailing `_`; dgtlrift/cddlc#149).
        let ty = backend_go::go_type_name(&v.cddl_name);
        let base = format!("TestInterop{ty}{}", to_pascal_case(&v.description.replace(' ', "_")));
        let mut name = base.clone();
        let mut i = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}{i}");
            i += 1;
        }
        let hex = v.cbor_hex();
        w.line(&format!("func {name}(t *testing.T) {{"));
        w.line(&format!("\tgot, err := Decode{ty}(interopBytes(t, {}))", go_str(&hex)));
        w.line("\tif err != nil {");
        w.line("\t\tt.Fatalf(\"decode: %v\", err)");
        w.line("\t}");
        for f in &v.fields {
            // An alias vector's one value is the alias itself.
            let target = if v.shape == VectorShape::Alias { "got".to_owned() } else { format!("got.{}", go_field_name(&f.name)) };
            let check = match &f.value {
                FieldVal::Enum { kind, .. } if v.shape == VectorShape::Enum => {
                    Some((format!("got.Kind != {}", go_kind_const(&v.cddl_name, kind)), "got.Kind".to_owned(), go_kind_const(&v.cddl_name, kind)))
                }
                FieldVal::Enum { .. } | FieldVal::Null => None,
                FieldVal::Float { value, .. } => Some((format!("math.Abs(float64({target})-{value:?}) > 1e-4"), target.clone(), format!("{value:?}"))),
                FieldVal::Uint(n) => Some((format!("uint64({target}) != {n}"), target.clone(), n.to_string())),
                FieldVal::Int(n) => Some((format!("int64({target}) != {n}"), target.clone(), n.to_string())),
                FieldVal::Bool(b) => Some((format!("bool({target}) != {b}"), target.clone(), b.to_string())),
                FieldVal::Text(t) => Some((format!("string({target}) != {}", go_str(t)), target.clone(), go_str(t))),
                FieldVal::Bytes(b) => {
                    let lit = format!("\"{}\"", b.iter().map(|x| format!("\\x{x:02x}")).collect::<String>());
                    Some((format!("string({target}) != {lit}"), format!("[]byte({target})"), format!("[]byte({lit})")))
                }
            };
            let Some((cond, got, want)) = check else { continue };
            w.line(&format!("\tif {cond} {{"));
            w.line(&format!("\t\tt.Errorf(\"%s: got %v, want %v\", {}, {got}, {want})", go_str(&f.name)));
            w.line("\t}");
        }
        w.line("\tre, err := got.Encode()");
        w.line("\tif err != nil {");
        w.line("\t\tt.Fatalf(\"re-encode: %v\", err)");
        w.line("\t}");
        w.line(&format!("\tif h := hex.EncodeToString(re); h != {} {{", go_str(&hex)));
        w.line(&format!("\t\tt.Errorf(\"re-encoded %s, want %s\", h, {})", go_str(&hex)));
        w.line("\t}");
        w.line("}");
        w.blank();
    }
    format!("{}\n", w.finish().trim_end())
}

// ── Java harness ──────────────────────────────────────────────────────────────

/// Java (SE or Jakarta EE): copy into the generated project's
/// `src/test/java/<package>/` and run `mvn test`. Each test decodes a vector,
/// checks its primitive members and the enum variant, and re-encodes it byte
/// for byte.
fn emit_java_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    use backend_java::{java_member_name, java_package_name, java_str, java_type_name};
    let pkg = java_package_name(schema, opts.namespace.as_deref());
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// interop test harness (Java)");
    w.line(&format!("// Copy into the generated project (`{schema}-cbor/src/test/java/{}/`) and run `mvn test`.", pkg.replace('.', "/")));
    w.blank();
    w.line(&format!("package {pkg};"));
    w.blank();
    w.line("import static org.junit.jupiter.api.Assertions.assertArrayEquals;");
    w.line("import static org.junit.jupiter.api.Assertions.assertEquals;");
    w.blank();
    w.line("import java.util.HexFormat;");
    w.line("import org.junit.jupiter.api.Test;");
    w.blank();
    w.line("/** Decodes the shared vectors, checks their values and re-encodes them byte for byte. */");
    w.line("class InteropTest {");
    w.indent();
    w.line("private static byte[] bytes(String hex) {");
    w.line("    return HexFormat.of().parseHex(hex);");
    w.line("}");
    w.blank();

    let mut used = std::collections::HashSet::new();
    for v in vectors {
        let class = java_type_name(&v.cddl_name);
        let camel: String = {
            let p = to_pascal_case(&v.cddl_name);
            p.chars().take(1).flat_map(|c| c.to_lowercase()).chain(p.chars().skip(1)).collect()
        };
        let base = format!("{camel}{}", to_pascal_case(&v.description.replace(' ', "_")));
        let mut name = base.clone();
        let mut i = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}{i}");
            i += 1;
        }
        let hex = v.cbor_hex();
        w.line("@Test");
        w.line(&format!("void {name}() {{"));
        w.indent();
        w.line(&format!("{class} got = {class}.decode(bytes({}));", java_str(&hex)));
        for f in &v.fields {
            // An alias vector's one value is the alias's `value`.
            let target = if v.shape == VectorShape::Alias { "got.value()".to_owned() } else { format!("got.{}()", java_member_name(&f.name)) };
            let label = java_str(&f.name);
            match &f.value {
                FieldVal::Enum { kind, .. } if v.shape == VectorShape::Enum => {
                    w.line(&format!("assertEquals({}, got.kind());", java_str(kind)));
                }
                FieldVal::Enum { .. } | FieldVal::Null => {}
                FieldVal::Uint(n) => w.line(&format!("assertEquals({n}L, {target}, {label});")),
                FieldVal::Int(n) => w.line(&format!("assertEquals({n}L, {target}, {label});")),
                FieldVal::Bool(b) => w.line(&format!("assertEquals({b}, {target}, {label});")),
                FieldVal::Text(t) => w.line(&format!("assertEquals({}, {target}, {label});", java_str(t))),
                FieldVal::Float { value, .. } => w.line(&format!("assertEquals({value:?}, {target}, 1e-4, {label});")),
                FieldVal::Bytes(b) => {
                    let lit = if b.is_empty() { "new byte[0]".to_owned() } else {
                        format!("new byte[] {{{}}}", b.iter().map(|x| format!("(byte) 0x{x:02x}")).collect::<Vec<_>>().join(", "))
                    };
                    w.line(&format!("assertArrayEquals({lit}, {target}, {label});"));
                }
            }
        }
        w.line(&format!("assertEquals({}, HexFormat.of().formatHex(got.encode()), \"re-encoded\");", java_str(&hex)));
        w.dedent();
        w.line("}");
        w.blank();
    }
    w.dedent();
    w.line("}");
    format!("{}\n", w.finish().trim_end())
}

// ── Lua harness ───────────────────────────────────────────────────────────────

/// Lua: copy into the generated package's `spec/` and run `busted`. Each test
/// decodes a vector, checks its primitive members and the enum variant, and
/// re-encodes it byte for byte.
fn emit_lua_harness(vectors: &[TestVector], schema: &str, opts: &CodegenOptions) -> String {
    use backend_lua::{lua_bytes, lua_member_name, lua_module_name, lua_str, lua_type_name};
    let modname = lua_module_name(schema, opts.namespace.as_deref());
    let mut w = IndentWriter::new(2);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::DashDash) {
        w.line(&l);
    }
    w.line("-- interop test harness (Lua)");
    w.line(&format!("-- Copy into the generated package (`{schema}-cbor/spec/`) and run `busted`."));
    w.blank();
    w.line(&format!("local M = require({})", lua_str(&modname)));
    w.blank();
    w.line("local function unhex(h) return (h:gsub(\"..\", function(b) return string.char(tonumber(b, 16)) end)) end");
    w.line("local function hex(s) return (s:gsub(\".\", function(c) return string.format(\"%02x\", c:byte()) end)) end");
    w.blank();
    w.line("describe(\"interop vectors\", function()");
    w.indent();
    let mut used = std::collections::HashSet::new();
    for v in vectors {
        let t = lua_type_name(&v.cddl_name);
        let base = format!("{t}: {}", v.description);
        let mut name = base.clone();
        let mut i = 2;
        while !used.insert(name.clone()) {
            name = format!("{base} ({i})");
            i += 1;
        }
        let hex = v.cbor_hex();
        w.line(&format!("it({}, function()", lua_str(&name)));
        w.indent();
        w.line(&format!("local got = M.{t}.decode(unhex({}))", lua_str(&hex)));
        for f in &v.fields {
            // An alias vector's one value is the alias itself.
            let target = if v.shape == VectorShape::Alias { "got".to_owned() } else { format!("got.{}", lua_member_name(&f.name)) };
            match &f.value {
                FieldVal::Enum { kind, .. } if v.shape == VectorShape::Enum => w.line(&format!("assert.equal({}, got.kind)", lua_str(kind))),
                FieldVal::Enum { .. } | FieldVal::Null => {}
                FieldVal::Uint(n) => w.line(&format!("assert.equal({n}, {target})")),
                FieldVal::Int(n) => w.line(&format!("assert.equal({n}, {target})")),
                FieldVal::Bool(b) => w.line(&format!("assert.equal({b}, {target})")),
                FieldVal::Text(t) => w.line(&format!("assert.equal({}, {target})", lua_str(t))),
                FieldVal::Bytes(b) => w.line(&format!("assert.equal({}, {target})", lua_bytes(b))),
                FieldVal::Float { value, .. } => w.line(&format!("assert.near({value:?}, {target}, 1e-4)")),
            }
        }
        w.line(&format!("assert.equal({}, hex(M.{t}.encode(got)))", lua_str(&hex)));
        w.dedent();
        w.line("end)");
        w.blank();
    }
    w.dedent();
    w.line("end)");
    format!("{}\n", w.finish().trim_end())
}
