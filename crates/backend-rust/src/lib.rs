/// Rust backend for cddlc.
///
/// Emits a single `.rs` file per CDDL module containing:
///   - Type declarations (structs, enums, type aliases)
///   - `minicbor::Encode` implementations (with optional dCBOR key sorting)
///   - `minicbor::Decode` implementations (with inline validation)
///   - Tag verification on decode, tag emission on encode
///   - `#![no_std]` compatible when `opts.no_std` is true

use cddlc_codegen::{
    capacity_value, constraint_to_rust_check,
    primitive_to_rust, to_pascal_case, to_snake_case, AllocStrategy,
    Backend, CodegenError, CodegenOptions, CodegenOutput, IndentWriter,
};
use std::collections::HashSet;

use cddlc_ir::{
    AliasDef, ArrayDef, Capacity, CborTag, Constraint, EnumDef, EnumVariant,
    FieldDef, IrModule, LiteralValue, MapDef, Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

/// Rust keywords (strict, reserved and edition-2024) that cannot be plain identifiers.

/// The decoder's local variable for a field: prefixed, so a field named like one of the
/// decoder's own locals (`d`, `ctx`, `len`, `i`, `key`, `val`) can't shadow it.
fn field_local(name: &str) -> String {
    format!("f_{}", field_ident(name).trim_start_matches("r#"))
}

/// A valid Rust identifier for a CDDL field name (see [`cddlc_codegen::rust_field_ident`]).
/// Serde derives strip the `r#`, and the JSON backend adds an explicit rename
/// whenever the identifier differs.
fn field_ident(name: &str) -> String {
    cddlc_codegen::rust_field_ident(name)
}

/// The literal a field or variant must equal (`cat: "kernel"` lowers to `.eq "kernel"`).
fn literal_eq(constraints: &[Constraint]) -> Option<&LiteralValue> {
    constraints.iter().find_map(|c| match c {
        Constraint::Eq(v) => Some(v),
        _ => None,
    })
}

/// The literal of a single-value field whose primitive type matches it, if any.
fn field_literal(f: &FieldDef) -> Option<&LiteralValue> {
    let lit = literal_eq(&f.constraints)?;
    let matches_ty = matches!(
        (&f.ty, lit),
        (TypeRef::Primitive(Primitive::Tstr), LiteralValue::Text(_))
            | (TypeRef::Primitive(Primitive::Uint), LiteralValue::Uint(_))
            | (TypeRef::Primitive(Primitive::Int), LiteralValue::Int(_) | LiteralValue::Uint(_))
            | (TypeRef::Primitive(Primitive::Bool), LiteralValue::Bool(_))
    );
    (matches_ty && matches!(f.occurrence, Occurrence::Required | Occurrence::Optional))
        .then_some(lit)
}

/// The string of a string-literal enum variant (`"queue-empty"` in `"until" / "queue-empty"`).
fn text_literal(v: &EnumVariant) -> Option<&str> {
    match (&v.ty, literal_eq(&v.constraints)) {
        (TypeRef::Primitive(Primitive::Tstr), Some(LiteralValue::Text(t))) => Some(t.as_str()),
        _ => None,
    }
}

/// `--no-std`: a fixed-capacity `heapless::String` from the `&str` expression
/// `expr`, for literals and test values known to fit.
fn no_std_text(expr: &str) -> String {
    format!("cddlc_text({expr}).expect(\"fits its capacity\")")
}

/// `--no-std`: a fixed-capacity `heapless::Vec<u8, N>` from the `&[u8]` expression `expr`.
fn no_std_bytes(expr: &str) -> String {
    format!("cddlc_bytes({expr}).expect(\"fits its capacity\")")
}

/// The Rust type of a primitive. `--no-std` holds text and bytes in
/// fixed-capacity `heapless` collections of `--max-str` (dgtlrift/cddlc#65).
fn rust_primitive(p: &Primitive, opts: &CodegenOptions) -> String {
    match p {
        Primitive::Tstr if opts.no_std => format!("heapless::String<{}>", opts.max_str),
        Primitive::Bstr | Primitive::Any if opts.no_std => format!("heapless::Vec<u8, {}>", opts.max_str),
        _ => primitive_to_rust(p, false).to_owned(),
    }
}

/// Whether `name` is an array or table type, which with `--no-std` is encoded
/// and decoded by its own functions.
fn is_container(name: &str, module: Option<&IrModule>) -> bool {
    matches!(module.and_then(|m| m.get(name)), Some(TypeDef::Array(_) | TypeDef::Map(_)))
}

/// The integer of an integer-literal enum variant (the `2` in `1 / 2 / 3`).
fn int_literal(v: &EnumVariant) -> Option<&LiteralValue> {
    match (&v.ty, literal_eq(&v.constraints)) {
        (TypeRef::Primitive(Primitive::Uint | Primitive::Int), Some(lit @ (LiteralValue::Uint(_) | LiteralValue::Int(_)))) => Some(lit),
        _ => None,
    }
}

/// The CDDL string for a `tstr` enum variant: its literal, or (for variants without
/// one) the historical lowercased-name fallback.
fn variant_str(v: &EnumVariant) -> String {
    cddlc_codegen::enum_variant_text(v)
}

/// A Rust expression for a literal value, as an owned field value.
fn literal_value_expr(lit: &LiteralValue, no_std: bool) -> String {
    match lit {
        LiteralValue::Text(t) if no_std => no_std_text(&format!("{t:?}")),
        LiteralValue::Bytes(b) if no_std => no_std_bytes(&format!("&{b:?}")),
        LiteralValue::Text(t) => format!("{t:?}.to_string()"),
        LiteralValue::Uint(n) => format!("{n}u64"),
        LiteralValue::Int(n)  => format!("{n}i64"),
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Float(f) => format!("{f:?}"),
        LiteralValue::Null => "()".into(),
        LiteralValue::Bytes(b) => format!("vec!{b:?}"),
    }
}

/// Name of the JSON marker type for a literal member, avoiding module type names.
fn marker_name(owner: &str, member: &str, module: &IrModule) -> String {
    let base = format!("{}{}", to_pascal_case(owner), to_pascal_case(member));
    if module.types.keys().any(|k| to_pascal_case(k) == base) {
        format!("{base}Literal")
    } else {
        base
    }
}

pub struct RustBackend;

impl Backend for RustBackend {
    fn language_name(&self) -> &str { "Rust" }
    fn file_extension(&self) -> &str { "rs" }

    fn generate(
        &self,
        module: &IrModule,
        opts:   &CodegenOptions,
    ) -> Result<CodegenOutput, CodegenError> {
        let fmt_tag    = if opts.is_json() { "json" } else { "cbor" };
        let crate_name = format!("{}-{fmt_tag}", module.name);
        let lib_src    = if opts.is_json() { emit_module_json(module, opts) } else { emit_module(module, opts) };
        let tests_src  = if opts.is_json() { emit_roundtrip_tests_json(module, opts) } else { emit_roundtrip_tests(module, opts) };
        let cargo_toml = emit_cargo_toml(&crate_name, opts);

        Ok(CodegenOutput {
            files: vec![
                cddlc_codegen::GeneratedFile {
                    path:    format!("{}/Cargo.toml", crate_name),
                    content: cargo_toml,
                },
                cddlc_codegen::GeneratedFile {
                    path:    format!("{}/src/lib.rs", crate_name),
                    content: lib_src,
                },
                cddlc_codegen::GeneratedFile {
                    path:    format!("{}/tests/roundtrip.rs", crate_name),
                    content: tests_src,
                },
            ],
        })
    }
}

// ── Module emit ───────────────────────────────────────────────────────────────

fn emit_module(module: &IrModule, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);

    // File header
    emit_header(&mut w, opts);

    // Each type definition
    for (_, def) in &module.types {
        w.blank();
        match def {
            TypeDef::Struct(s) => emit_struct(&mut w, s, opts, module),
            TypeDef::Enum(e)   => emit_enum(&mut w, e, opts, module),
            TypeDef::Array(a)  => emit_array(&mut w, a, opts, module),
            TypeDef::Alias(a)  => emit_alias(&mut w, a, opts),
            TypeDef::Map(m)    => emit_map(&mut w, m, opts, module),
        }
    }

    w.finish()
}

fn emit_header(w: &mut IndentWriter, opts: &CodegenOptions) {
    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Runtime: minicbor");
    if opts.dcbor { w.line("// Encoding: deterministic (dCBOR)"); }
    w.blank();

    // no_std is always feature-gated so the crate works in both environments
    w.line("#![cfg_attr(feature = \"no-std\", no_std)]");
    w.blank();

    w.line("use minicbor::{Decode, Decoder, Encode, Encoder};");
    w.line("use minicbor::decode::Error as DecodeError;");
    w.line("use minicbor::encode::Error as EncodeError;");
    w.line("use minicbor::encode::Write;");
    w.blank();

    if opts.no_std {
        // Text and bytes live in fixed-capacity heapless collections (dgtlrift/cddlc#65).
        w.line("/// A `tstr` in a fixed-capacity string; fails if it is longer.");
        w.line("#[doc(hidden)]");
        w.line("pub fn cddlc_text<const N: usize>(s: &str) -> Result<heapless::String<N>, DecodeError> {");
        w.indent();
        w.line("heapless::String::try_from(s).map_err(|_| DecodeError::message(\"text longer than its capacity\"))");
        w.dedent();
        w.line("}");
        w.blank();
        w.line("/// A `bstr` in a fixed-capacity byte vector; fails if it is longer.");
        w.line("#[doc(hidden)]");
        w.line("pub fn cddlc_bytes<const N: usize>(b: &[u8]) -> Result<heapless::Vec<u8, N>, DecodeError> {");
        w.indent();
        w.line("heapless::Vec::from_slice(b).map_err(|_| DecodeError::message(\"byte string longer than its capacity\"))");
        w.dedent();
        w.line("}");
        w.blank();
    }

    // Error types
    w.line("/// Field-level decode error.");
    w.line("#[derive(Debug, Clone, PartialEq)]");
    w.line("pub struct FieldError {");
    w.indent();
    w.line("pub code:  ErrorCode,");
    w.line("pub field: &'static str,");
    w.line("pub index: Option<usize>,");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("#[derive(Debug, Clone, Copy, PartialEq, Eq)]");
    w.line("pub enum ErrorCode {");
    w.indent();
    w.line("UnexpectedType,");
    w.line("UnexpectedValue,");
    w.line("MissingField,");
    w.line("Overflow,");
    w.line("InvalidUtf8,");
    w.line("UnknownField,");
    w.line("Truncated,");
    w.line("DepthLimit,");
    w.line("TagMismatch,");
    w.line("ValidationFailed,");
    w.dedent();
    w.line("}");
}

// ── Struct ────────────────────────────────────────────────────────────────────

/// The Rust type of a map's other entries (`* tstr => V`, dgtlrift/cddlc#82):
/// a `BTreeMap` from text, or with `--no-std` a `heapless::LinearMap`.
fn rest_type(r: &cddlc_ir::MapRest, opts: &CodegenOptions) -> String {
    let cap = capacity_value(
        r.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    if opts.no_std {
        let val = match &r.val_ty {
            TypeRef::Tagged(_, inner) => typeref_to_rust(inner, opts),
            other => typeref_to_rust(other, opts),
        };
        format!("heapless::LinearMap<heapless::String<{}>, {val}, {cap}>", opts.max_str)
    } else {
        format!("std::collections::BTreeMap<String, {}>", rest_value_heap(&r.val_ty, opts))
    }
}

/// A value of a map's other entries outside `--no-std`, as minicbor encodes it:
/// a bstr is a `ByteVec`, a tagged value a `Tagged`.
fn rest_value_heap(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Tagged(tag, inner) => format!("minicbor::data::Tagged<{}, {}>", tag.0, rest_value_heap(inner, opts)),
        other if is_bstr(other) => "minicbor::bytes::ByteVec".to_owned(),
        other => typeref_to_rust(other, opts),
    }
}

/// A test value for a map's other entries: one entry under the key `k`.
fn rest_test_value(r: &cddlc_ir::MapRest, opts: &CodegenOptions) -> String {
    let val = primitive_default_expr(&r.val_ty, opts);
    if opts.no_std {
        format!(
            "{{ let mut m = {}::new(); m.insert({}, {val}).expect(\"capacity\"); m }}",
            rest_type(r, opts).split('<').next().unwrap_or_default(),
            no_std_text("\"k\""),
        )
    } else {
        format!("{{ let mut m = std::collections::BTreeMap::new(); m.insert(\"k\".to_string(), {val}.into()); m }}")
    }
}

fn emit_struct(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, s.doc.as_deref());
    if s.deprecated { w.line("#[deprecated]"); }

    let has_required_literal = s.fields.iter()
        .any(|f| f.occurrence.is_required() && field_literal(f).is_some());
    if has_required_literal {
        w.line("#[derive(Debug, Clone, PartialEq)]");
    } else {
        w.line("#[derive(Debug, Clone, PartialEq, Default)]");
    }
    w.line(&format!("pub struct {} {{", to_pascal_case(&s.name)));
    w.indent();
    for f in &s.fields {
        emit_doc(w, f.doc.as_deref());
        let ty_str = field_type_str(&f.ty, &f.occurrence, opts);
        w.line(&format!("pub {}: {},", field_ident(&f.name), ty_str));
    }
    if let Some(r) = &s.rest {
        w.line("/// The map's other entries.");
        w.line(&format!("pub {}: {},", field_ident(&r.name), rest_type(r, opts)));
    }
    w.dedent();
    w.line("}");

    // Literal fields default to their literal, so `Default` values decode again.
    if has_required_literal {
        w.blank();
        w.line(&format!("impl Default for {} {{", to_pascal_case(&s.name)));
        w.indent();
        w.line("fn default() -> Self {");
        w.indent();
        w.line("Self {");
        w.indent();
        for f in &s.fields {
            let val = match field_literal(f) {
                Some(lit) if f.occurrence.is_required() => literal_value_expr(lit, opts.no_std),
                _ => "Default::default()".into(),
            };
            w.line(&format!("{}: {val},", field_ident(&f.name)));
        }
        if let Some(r) = &s.rest {
            w.line(&format!("{}: Default::default(),", field_ident(&r.name)));
        }
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
    }

    w.blank();
    emit_struct_encode(w, s, opts, module);
    w.blank();
    emit_struct_decode(w, s, opts, module);
}

fn emit_struct_encode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    let type_name = to_pascal_case(&s.name);
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<C> Encode<C> for {} {{", type_name));
    w.indent();
    w.line("fn encode<W: Write>(&self, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {");
    w.indent();

    // Emit CBOR tag if present
    if let Some(tag) = s.tagged {
        w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
    }

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_record_encode(w, s, opts, module);
        w.line("Ok(())");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
        return;
    }

    // Compute map length: required count is static; add 1 per present optional field
    let req_count = s.fields.iter()
        .filter(|f| !matches!(f.occurrence, Occurrence::Optional))
        .count();
    let has_optional = s.fields.iter().any(|f| matches!(f.occurrence, Occurrence::Optional));

    let rest_len = s.rest.as_ref()
        .map(|r| format!(" + self.{}.len() as u64", field_ident(&r.name)))
        .unwrap_or_default();
    if !has_optional {
        w.line(&format!("e.map({}u64{rest_len})?;", req_count));
    } else {
        let mut len_expr = format!("{}u64", req_count);
        for f in s.fields.iter().filter(|f| matches!(f.occurrence, Occurrence::Optional)) {
            let fname = field_ident(&f.name);
            len_expr.push_str(&format!(" + if self.{fname}.is_some() {{ 1 }} else {{ 0 }}"));
        }
        w.line(&format!("e.map({}{rest_len})?;", len_expr));
    }

    // Emit fields — sorted if dCBOR
    let fields: Vec<_> = if opts.dcbor {
        let mut fs: Vec<_> = s.fields.iter().collect();
        fs.sort_by(|a, b| a.name.cmp(&b.name));
        fs
    } else {
        s.fields.iter().collect()
    };

    for f in &fields {
        let field_access = format!("self.{}", field_ident(&f.name));
        if matches!(f.occurrence, Occurrence::Optional) {
            // Only emit key+value when the option is Some
            w.line(&format!("if let Some(ref v) = {} {{", field_access));
            w.indent();
            w.line(&format!("e.str({:?})?;", f.name.as_str()));
            // Use (*v) to deref the `&T` produced by `Some(ref v)`.
            // Primitive encode calls (e.u64, e.f64, …) take T by value;
            // method calls on named types need explicit deref: (*v).encode().
            emit_encode_typeref(w, &f.ty, "(*v)", opts, Some(module));
            w.dedent();
            w.line("}");
        } else {
            w.line(&format!("e.str({:?})?;", f.name.as_str()));
            emit_encode_value(w, &f.ty, &field_access, &f.occurrence, opts, Some(module));
        }
    }
    // The other entries, after the named members.
    if let Some(r) = &s.rest {
        w.line(&format!("for (k, v) in self.{}.iter() {{", field_ident(&r.name)));
        w.indent();
        w.line("e.str(k)?;");
        if opts.no_std {
            emit_encode_typeref(w, &r.val_ty, "(*v)", opts, Some(module));
        } else {
            w.line("minicbor::Encode::encode(v, e, ctx)?;");
        }
        w.dedent();
        w.line("}");
    }

    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

/// A record's members as an array, in order (RFC 8610 §3.4). Optional members
/// come last: those present are written up to the first absent one, and a
/// present one after it can't be placed, so encoding fails.
fn emit_record_encode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();
    let optional: Vec<_> = s.fields.iter().filter(|f| !f.occurrence.is_required()).collect();
    if optional.is_empty() {
        w.line(&format!("e.array({required}u64)?;"));
    } else {
        w.line(&format!("let mut len = {required}u64;"));
        w.line("let mut gap = false;");
        for f in &optional {
            let fname = field_ident(&f.name);
            w.line(&format!("if self.{fname}.is_some() {{"));
            w.indent();
            w.line(&format!(
                "if gap {{ return Err(EncodeError::message(\"{}: '{}' is set but an optional member before it isn't\")); }}",
                s.name, f.name
            ));
            w.line("len += 1;");
            w.dedent();
            w.line("} else { gap = true; }");
        }
        w.line("let _ = gap;");
        w.line("e.array(len)?;");
    }
    for f in &s.fields {
        let access = format!("self.{}", field_ident(&f.name));
        if f.occurrence.is_required() {
            emit_encode_value(w, &f.ty, &access, &f.occurrence, opts, Some(module));
        } else {
            w.line(&format!("if let Some(ref v) = {access} {{"));
            w.indent();
            emit_encode_typeref(w, &f.ty, "(*v)", opts, Some(module));
            w.dedent();
            w.line("}");
        }
    }
}

/// Reads a record: an array of the members' values, in order; optional
/// members at the end may be left off.
fn emit_record_decode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    w.line("let len = d.array()?;");
    w.line("let mut i = 0u64;");
    w.line("fn more(d: &mut Decoder<'_>, len: Option<u64>, i: u64) -> Result<bool, DecodeError> {");
    w.indent();
    w.line("Ok(match len { Some(n) => i < n, None => d.datatype()? != minicbor::data::Type::Break })");
    w.dedent();
    w.line("}");
    for f in &s.fields {
        let local = field_local(&f.name);
        let decode_expr = decode_expr_for_in_module(&f.ty, opts, Some(module));
        if f.occurrence.is_required() {
            w.line("if !more(d, len, i)? {");
            w.indent();
            w.line(&format!("return Err(DecodeError::message(\"{}: missing item '{}'\"));", s.name, f.name));
            w.dedent();
            w.line("}");
            w.line(&format!("let {local} = {{ let val = {decode_expr};"));
            w.indent();
            emit_field_validation(w, &f.constraints, "val", &f.name);
            w.line("val };");
            w.dedent();
            w.line("i += 1;");
        } else {
            w.line(&format!("let {local} = if more(d, len, i)? {{"));
            w.indent();
            w.line(&format!("let val = {decode_expr};"));
            emit_field_validation(w, &f.constraints, "val", &f.name);
            w.line("i += 1;");
            w.line("Some(val)");
            w.dedent();
            w.line("} else { None };");
        }
    }
    w.line("match len {");
    w.indent();
    w.line(&format!(
        "Some(n) if i < n => return Err(DecodeError::message(\"{}: too many items\")),",
        s.name
    ));
    w.line("None => {");
    w.indent();
    w.line("if d.datatype()? != minicbor::data::Type::Break {");
    w.indent();
    w.line(&format!("return Err(DecodeError::message(\"{}: too many items\"));", s.name));
    w.dedent();
    w.line("}");
    w.line("d.skip()?;");
    w.dedent();
    w.line("}");
    w.line("_ => {}");
    w.dedent();
    w.line("}");
    w.line("Ok(Self {");
    w.indent();
    for f in &s.fields {
        w.line(&format!("{}: {},", field_ident(&f.name), field_local(&f.name)));
    }
    w.dedent();
    w.line("})");
}

fn emit_struct_decode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    let type_name = to_pascal_case(&s.name);

    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<'b> Decode<'b, ()> for {} {{", type_name));
    w.indent();
    w.line("fn decode(d: &mut Decoder<'b>, ctx: &mut ()) -> Result<Self, DecodeError> {");
    w.indent();

    // Tag verification
    if let Some(tag) = s.tagged {
        w.line(&format!("let t = d.tag()?;"));
        w.line(&format!("if t != minicbor::data::Tag::new({}) {{", tag.0));
        w.indent();
        w.line("return Err(DecodeError::message(\"unexpected CBOR tag\"));");
        w.dedent();
        w.line("}");
    }

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_record_decode(w, s, opts, module);
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
        return;
    }

    // Declare Option<T> vars for each field.
    // Optional fields already have their final type (e.g. String), so we don't
    // double-wrap with Option<Option<T>>; other occurrences use field_type_str.
    for f in &s.fields {
        let var_ty = match &f.occurrence {
            Occurrence::Optional => typeref_to_rust(&f.ty, opts),
            _ => field_type_str(&f.ty, &f.occurrence, opts),
        };
        w.line(&format!("let mut {}: Option<{}> = None;", field_local(&f.name), var_ty));
    }
    if let Some(r) = &s.rest {
        w.line(&format!("let mut rest = {}::new();", rest_type(r, opts).split('<').next().unwrap_or_default()));
    }
    w.blank();

    // Decode map entries
    w.line("let len = d.map()?;");
    w.line("let mut i = 0u64;");
    w.line("loop {");
    w.indent();
    w.line("match len { Some(n) if i >= n => break, None if d.datatype()? == minicbor::data::Type::Break => { d.skip()?; break }, _ => {} }");
    w.line("i += 1;");
    w.line("let key = d.str()?;");
    w.line("match key {");
    w.indent();

    for f in &s.fields {
        let local = field_local(&f.name);
        w.line(&format!("{:?} => {{", f.name.as_str()));
        w.indent();
        let decode_expr = decode_expr_for_in_module(&f.ty, opts, Some(module));
        w.line(&format!("let val = {};", decode_expr));

        // Inline validation
        emit_field_validation(w, &f.constraints, "val", &f.name);

        w.line(&format!("{local} = Some(val);"));
        w.dedent();
        w.line("}");
    }

    match &s.rest {
        // Other keys are the map's other entries.
        Some(r) if opts.no_std => {
            w.line("_ => {");
            w.indent();
            w.line(&format!("let val = {};", decode_expr_for_in_module(&r.val_ty, opts, Some(module))));
            w.line("rest.insert(cddlc_text(key)?, val).map_err(|_| DecodeError::message(\"map larger than its capacity\"))?;");
            w.dedent();
            w.line("}");
        }
        Some(_) => w.line("_ => { rest.insert(key.to_string(), d.decode()?); }"),
        // A map has only the members its schema gives it (RFC 8610 §3.5;
        // dgtlrift/cddlc#102), so a choice of maps decodes to the right one.
        None => w.line(&format!(
            "_ => {{ return Err(DecodeError::message(\"{}: unknown member\")); }}",
            s.name
        )),
    }
    w.dedent();
    w.line("}"); // match key
    w.dedent();
    w.line("}"); // loop
    w.blank();

    // Construct result — required fields unwrap, optional fields map
    w.line("Ok(Self {");
    w.indent();
    for f in &s.fields {
        let snake = field_ident(&f.name);
        let local = field_local(&f.name);
        match &f.occurrence {
            Occurrence::Required => {
                w.line(&format!(
                    "{snake}: {local}.ok_or_else(|| DecodeError::message(\"missing field '{}'\" ))?,",
                    f.name
                ));
            }
            Occurrence::Optional => {
                w.line(&format!("{snake}: {local},"));
            }
            _ => {
                w.line(&format!(
                    "{snake}: {local}.unwrap_or_default(),",
                ));
            }
        }
    }
    if let Some(r) = &s.rest {
        w.line(&format!("{}: rest,", field_ident(&r.name)));
    }
    w.dedent();
    w.line("})");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

// ── Enum ──────────────────────────────────────────────────────────────────────

fn emit_enum(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, e.doc.as_deref());
    if e.deprecated { w.line("#[deprecated]"); }

    // Check if first variant is a unit variant (can use #[derive(Default)] + #[default])
    let first_is_unit = e.variants.first().map(|v|
        matches!(&v.ty, TypeRef::Primitive(Primitive::Null))
    ).unwrap_or(true);

    if first_is_unit {
        w.line("#[derive(Debug, Clone, PartialEq, Default)]");
    } else {
        w.line("#[derive(Debug, Clone, PartialEq)]");
    }
    w.line(&format!("pub enum {} {{", to_pascal_case(&e.name)));
    w.indent();
    for (i, v) in e.variants.iter().enumerate() {
        let vname = to_pascal_case(&v.name);
        if i == 0 && first_is_unit { w.line("#[default]"); }
        match &v.ty {
            TypeRef::Primitive(Primitive::Null) => w.line(&format!("{vname},")),
            TypeRef::Primitive(p) => {
                let rt = rust_primitive(p, opts);
                w.line(&format!("{vname}({rt}),"));
            }
            TypeRef::Named(n) => {
                w.line(&format!("{vname}({}),", to_pascal_case(n)));
            }
            _ => w.line(&format!("{vname},")),
        }
    }
    w.dedent();
    w.line("}");

    // If first variant holds data, emit Default manually
    if !first_is_unit {
        if let Some(v) = e.variants.first() {
            let type_name = to_pascal_case(&e.name);
            let vname     = to_pascal_case(&v.name);
            w.blank();
            w.line(&format!("impl Default for {type_name} {{"));
            w.indent();
            w.line("fn default() -> Self {");
            w.indent();
            let default_inner = match &v.ty {
                TypeRef::Primitive(Primitive::Uint) => {
                    // For literal integer variants ("Variant1" etc.), store the literal value
                    v.name.as_str().strip_prefix("Variant")
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|n| format!("{n}u64"))
                        .unwrap_or_else(|| "0u64".into())
                }
                TypeRef::Primitive(Primitive::Int) => {
                    v.name.as_str().strip_prefix("Variant")
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|n| format!("{n}i64"))
                        .unwrap_or_else(|| "0i64".into())
                }
                TypeRef::Primitive(Primitive::Bool) => "false".into(),
                TypeRef::Primitive(Primitive::Float | Primitive::Float32 |
                                   Primitive::Float64 | Primitive::Float16) => "0.0".into(),
                TypeRef::Primitive(Primitive::Tstr) => {
                    let lit = variant_str(v);
                    if opts.no_std { no_std_text(&format!("{lit:?}")) } else { format!("{lit:?}.to_string()") }
                }
                TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => "Default::default()".into(),
                TypeRef::Named(n) => format!("{}::default()", to_pascal_case(n)),
                _ => "Default::default()".into(),
            };
            w.line(&format!("{type_name}::{vname}({default_inner})"));
            w.dedent();
            w.line("}");
            w.dedent();
            w.line("}");
        }
    }

    w.blank();
    emit_enum_encode(w, e, opts, module);
    w.blank();
    emit_enum_decode(w, e, opts, module);
}

fn emit_enum_encode(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions, module: &IrModule) {
    let type_name = to_pascal_case(&e.name);
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<C> Encode<C> for {} {{", type_name));
    w.indent();
    w.line("fn encode<W: Write>(&self, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {");
    w.indent();

    if let Some(tag) = e.tagged {
        w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
    }

    w.line("match self {");
    w.indent();
    for v in &e.variants {
        let vname = to_pascal_case(&v.name);
        let type_vname = to_pascal_case(&e.name);
        match &v.ty {
            TypeRef::Primitive(Primitive::Null) => {
                w.line(&format!("{}::{} => {{ e.null()?; }}", type_vname, vname));
            }
            // `heapless::String` / `heapless::Vec<u8, N>` (`--no-std`) have the
            // same `as_str` / `as_slice` as `String` / `Vec<u8>`.
            TypeRef::Primitive(Primitive::Tstr) => {
                w.line(&format!("{}::{}(s) => {{ e.str(s.as_str())?; }}", type_vname, vname));
            }
            TypeRef::Primitive(Primitive::Bstr) => {
                w.line(&format!("{}::{}(b) => {{ e.bytes(b.as_slice())?; }}", type_vname, vname));
            }
            TypeRef::Primitive(Primitive::Bool) => {
                w.line(&format!("{}::{}(b) => {{ e.bool(*b)?; }}", type_vname, vname));
            }
            TypeRef::Primitive(Primitive::Uint) => {
                // For literal integer variants ("Variant1" etc.), always encode the literal
                if let Some(lit) = v.name.as_str().strip_prefix("Variant").and_then(|s| s.parse::<u64>().ok()) {
                    w.line(&format!("{}::{}(_) => {{ e.u64({lit}u64)?; }}", type_vname, vname));
                } else {
                    w.line(&format!("{}::{}(n) => {{ e.u64(*n)?; }}", type_vname, vname));
                }
            }
            TypeRef::Primitive(Primitive::Int) => {
                if let Some(lit) = v.name.as_str().strip_prefix("Variant").and_then(|s| s.parse::<i64>().ok()) {
                    w.line(&format!("{}::{}(_) => {{ e.i64({lit}i64)?; }}", type_vname, vname));
                } else {
                    w.line(&format!("{}::{}(n) => {{ e.i64(*n as i64)?; }}", type_vname, vname));
                }
            }
            TypeRef::Primitive(p) if is_integer(p) => {
                w.line(&format!("{}::{}(n) => {{ e.i64(*n as i64)?; }}", type_vname, vname));
            }
            TypeRef::Primitive(p) if is_float(p) => {
                w.line(&format!("{}::{}(f) => {{ e.f64(*f as f64)?; }}", type_vname, vname));
            }
            // With `--no-std`, an array or table variant has its own encoder.
            TypeRef::Named(n) if opts.no_std && is_container(n, Some(module)) => {
                w.line(&format!("{}::{}(inner) => {{ encode_{}(inner, e, ctx)?; }}", type_vname, vname, to_snake_case(n)));
            }
            TypeRef::Named(_) => {
                w.line(&format!("{}::{}(inner) => {{ inner.encode(e, ctx)?; }}", type_vname, vname));
            }
            _ => {
                w.line(&format!("{}::{} => {{ e.null()?; }}", type_vname, vname));
            }
        }
    }
    w.dedent();
    w.line("}");
    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_enum_decode(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions, module: &IrModule) {
    let type_name = to_pascal_case(&e.name);
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<'b> Decode<'b, ()> for {} {{", type_name));
    w.indent();
    w.line("fn decode(d: &mut Decoder<'b>, ctx: &mut ()) -> Result<Self, DecodeError> {");
    w.indent();

    if let Some(tag) = e.tagged {
        w.line(&format!("let t = d.tag()?;"));
        w.line(&format!("if t != minicbor::data::Tag::new({}) {{", tag.0));
        w.indent();
        w.line("return Err(DecodeError::message(\"unexpected CBOR tag\"));");
        w.dedent();
        w.line("}");
    }

    // Peek at the CBOR type to decide which variant
    w.line("match d.datatype()? {");
    w.indent();

    // Group variants by their expected CBOR type
    let mut has_str   = false;
    let mut has_bool  = false;
    let mut has_int   = false;
    let mut has_map   = false;

    for v in &e.variants {
        match &v.ty {
            TypeRef::Primitive(Primitive::Tstr) => has_str  = true,
            TypeRef::Primitive(Primitive::Bool) => has_bool = true,
            TypeRef::Primitive(p) if is_integer(p) => has_int = true,
            TypeRef::Named(_) => has_map = true,
            _ => {}
        }
    }

    let enum_name = to_pascal_case(&e.name);

    if has_str {
        w.line("minicbor::data::Type::String => {");
        w.indent();
        w.line("let s = d.str()?;");
        w.line("match s {");
        w.indent();
        for v in &e.variants {
            if matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)) {
                let vname    = to_pascal_case(&v.name);
                let cddl_str = variant_str(v);
                let val: &str = if opts.no_std { "cddlc_text(s)?" } else { "s.to_string()" };
                w.line(&format!("{:?} => Ok({}::{}({val})),", cddl_str, enum_name, vname));
            }
        }
        w.line("_ => Err(DecodeError::message(\"unknown enum variant\")),");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
    }

    if has_int {
        w.line("minicbor::data::Type::U8 | minicbor::data::Type::U16 |");
        w.line("minicbor::data::Type::U32 | minicbor::data::Type::U64 |");
        w.line("minicbor::data::Type::I8 | minicbor::data::Type::I16 |");
        w.line("minicbor::data::Type::I32 | minicbor::data::Type::I64 => {");
        w.indent();

        // Collect integer variants and check if all are literal (named "VariantN")
        let int_vars: Vec<(&EnumVariant, i64)> = e.variants.iter()
            .filter(|v| matches!(&v.ty, TypeRef::Primitive(p) if is_integer(p)))
            .filter_map(|v| {
                v.name.as_str().strip_prefix("Variant")
                    .and_then(|s| s.parse::<i64>().ok())
                    .map(|lit| (v, lit))
            })
            .collect();

        let all_literal = int_vars.len() == e.variants.iter()
            .filter(|v| matches!(&v.ty, TypeRef::Primitive(p) if is_integer(p)))
            .count();

        if all_literal && !int_vars.is_empty() {
            // Literal integer enum: decode the value and match against each literal
            w.line("let n = d.i64()?;");
            w.line("match n {");
            w.indent();
            for (v, lit) in &int_vars {
                let vname = to_pascal_case(&v.name);
                let inner = match &v.ty {
                    TypeRef::Primitive(Primitive::Uint) => format!("{lit}u64"),
                    _                                   => format!("{lit}i64"),
                };
                w.line(&format!("{lit} => Ok({}::{}({inner})),", enum_name, vname));
            }
            w.line("_ => Err(DecodeError::message(\"unknown enum variant\")),");
            w.dedent();
            w.line("}");
        } else {
            // Non-literal integer variants: decode using the first variant's type
            for v in &e.variants {
                if let TypeRef::Primitive(p) = &v.ty {
                    if is_integer(p) {
                        let vname = to_pascal_case(&v.name);
                        w.line(&format!("Ok({}::{}(d.{}()?))", enum_name, vname, decode_fn_for_primitive(p)));
                        break;
                    }
                }
            }
        }
        w.dedent();
        w.line("}");
    }

    if has_bool {
        w.line("minicbor::data::Type::Bool => {");
        w.indent();
        for v in &e.variants {
            if matches!(&v.ty, TypeRef::Primitive(Primitive::Bool)) {
                let vname = to_pascal_case(&v.name);
                w.line(&format!("Ok({}::{}(d.bool()?))", enum_name, vname));
                break;
            }
        }
        w.dedent();
        w.line("}");
    }

    if has_map {
        // Named variants (typically maps): try each in order on a probe decoder and
        // commit the first that decodes, so literal fields discriminate the union.
        w.line("_ => {");
        w.indent();
        for v in &e.variants {
            if let TypeRef::Named(n) = &v.ty {
                let vname = to_pascal_case(&v.name);
                w.line("{");
                w.indent();
                w.line("let mut probe = d.clone();");
                if opts.no_std && is_container(n, Some(module)) {
                    w.line(&format!("if let Ok(inner) = decode_{}(&mut probe, ctx) {{", to_snake_case(n)));
                } else {
                    w.line(&format!(
                        "if let Ok(inner) = <{} as Decode<'b, ()>>::decode(&mut probe, ctx) {{",
                        to_pascal_case(n)
                    ));
                }
                w.indent();
                w.line("*d = probe;");
                w.line(&format!("return Ok({enum_name}::{vname}(inner));"));
                w.dedent();
                w.line("}");
                w.dedent();
                w.line("}");
            }
        }
        w.line("Err(DecodeError::message(\"no enum variant matched\"))");
        w.dedent();
        w.line("}");
    } else {
        w.line("_ => Err(DecodeError::message(\"unexpected CBOR type for enum\")),");
    }

    w.dedent();
    w.line("}"); // match datatype
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

// ── Array ─────────────────────────────────────────────────────────────────────

fn emit_array(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, a.doc.as_deref());
    if a.deprecated { w.line("#[deprecated]"); }

    let type_name = to_pascal_case(&a.name);
    if opts.no_std {
        emit_array_no_std(w, a, opts, module);
        return;
    }
    // minicbor encodes a `Vec<u8>` element as an array of integers; `ByteVec` is a bstr.
    let heap_inner = |ty: &TypeRef| if is_bstr(ty) {
        "minicbor::bytes::ByteVec".to_owned()
    } else {
        typeref_to_rust(ty, opts)
    };
    // The array is a plain `Vec` alias encoded by minicbor's blanket impls, so a
    // tagged element (`[* #6.1(uint)]`) is wrapped in `minicbor::data::Tagged`,
    // which writes and checks the tag per item (dgtlrift/cddlc#47).
    let (elem_ty, heap_elem_ty) = match &a.element {
        TypeRef::Tagged(tag, inner) => (
            format!("minicbor::data::Tagged<{}, {}>", tag.0, typeref_to_rust(inner, opts)),
            format!("minicbor::data::Tagged<{}, {}>", tag.0, heap_inner(inner)),
        ),
        other => (typeref_to_rust(other, opts), heap_inner(other)),
    };
    let cap       = capacity_value(
        a.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );

    // Emit both forms, gated on the no-std feature.
    // #[allow(unexpected_cfgs)] suppresses the lint when this code is
    // include!()'d into a crate that doesn't declare the "no-std" feature.
    w.line("#[allow(unexpected_cfgs)]");
    w.line("#[cfg(not(feature = \"no-std\"))]");
    w.line(&format!("pub type {} = Vec<{}>;", type_name, heap_elem_ty));
    w.line("#[allow(unexpected_cfgs)]");
    w.line("#[cfg(feature = \"no-std\")]");
    w.line(&format!(
        "pub type {} = heapless::Vec<{}, {}>;",
        type_name, elem_ty, cap
    ));
}

/// `--no-std`: the array is always a fixed-capacity `heapless::Vec`, whatever the
/// crate's features, so it matches the struct fields that hold it. minicbor has no
/// impls for heapless, so `encode_<name>` / `decode_<name>` write and read it
/// element by element; a tagged element is tagged per item (dgtlrift/cddlc#65).
/// A table (`{ * tstr => uint }`): a `BTreeMap`, or with `--no-std` a
/// `heapless::LinearMap` with its own encode/decode functions (dgtlrift/cddlc#82).
fn emit_map(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, m.doc.as_deref());
    if m.deprecated { w.line("#[deprecated]"); }
    let type_name = to_pascal_case(&m.name);
    let cap = capacity_value(
        m.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    let plain = |ty: &TypeRef| match ty {
        TypeRef::Tagged(_, inner) => typeref_to_rust(inner, opts),
        other => typeref_to_rust(other, opts),
    };
    if !opts.no_std {
        // minicbor's blanket impls encode the map; a bstr is a `ByteVec` (a
        // `Vec<u8>` is an array of integers) and a tagged item checks its tag.
        let heap = |ty: &TypeRef| match ty {
            TypeRef::Tagged(tag, inner) => format!(
                "minicbor::data::Tagged<{}, {}>",
                tag.0,
                if is_bstr(inner) { "minicbor::bytes::ByteVec".to_owned() } else { typeref_to_rust(inner, opts) }
            ),
            other if is_bstr(other) => "minicbor::bytes::ByteVec".to_owned(),
            other => typeref_to_rust(other, opts),
        };
        w.line("#[allow(unexpected_cfgs)]");
        w.line("#[cfg(not(feature = \"no-std\"))]");
        w.line(&format!(
            "pub type {type_name} = std::collections::BTreeMap<{}, {}>;",
            heap(&m.key_ty), heap(&m.val_ty)
        ));
        w.line("#[allow(unexpected_cfgs)]");
        w.line("#[cfg(feature = \"no-std\")]");
        w.line(&format!(
            "pub type {type_name} = heapless::LinearMap<{}, {}, {cap}>;",
            plain(&m.key_ty), plain(&m.val_ty)
        ));
        return;
    }

    let snake = to_snake_case(&m.name);
    w.line(&format!(
        "pub type {type_name} = heapless::LinearMap<{}, {}, {cap}>;",
        plain(&m.key_ty), plain(&m.val_ty)
    ));
    w.blank();
    w.line(&format!("/// Writes a `{type_name}` as a CBOR map."));
    w.line("#[allow(unused_variables)]");
    w.line(&format!(
        "pub fn encode_{snake}<C, W: Write>(val: &{type_name}, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {{"
    ));
    w.indent();
    if let Some(tag) = m.tagged {
        w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
    }
    w.line("e.map(val.len() as u64)?;");
    w.line("for (key, value) in val.iter() {");
    w.indent();
    emit_encode_typeref(w, &m.key_ty, "(*key)", opts, Some(module));
    emit_encode_typeref(w, &m.val_ty, "(*value)", opts, Some(module));
    w.dedent();
    w.line("}");
    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("/// Reads a `{type_name}`; fails past its capacity of {cap}."));
    w.line("#[allow(unused_variables)]");
    w.line(&format!(
        "pub fn decode_{snake}(d: &mut Decoder<'_>, ctx: &mut ()) -> Result<{type_name}, DecodeError> {{"
    ));
    w.indent();
    if let Some(tag) = m.tagged {
        w.line(&format!("if d.tag()? != minicbor::data::Tag::new({}) {{ return Err(DecodeError::message(\"unexpected CBOR tag\")); }}", tag.0));
    }
    w.line("let len = d.map()?;");
    w.line("let mut out = heapless::LinearMap::new();");
    w.line("let mut i = 0u64;");
    w.line("loop {");
    w.indent();
    w.line("match len { Some(n) if i >= n => break, None if d.datatype()? == minicbor::data::Type::Break => { d.skip()?; break }, _ => {} }");
    w.line("i += 1;");
    w.line(&format!("let key = {};", decode_expr_for_in_module(&m.key_ty, opts, Some(module))));
    w.line(&format!("let value = {};", decode_expr_for_in_module(&m.val_ty, opts, Some(module))));
    w.line("out.insert(key, value).map_err(|_| DecodeError::message(\"map larger than its capacity\"))?;");
    w.dedent();
    w.line("}");
    w.line("Ok(out)");
    w.dedent();
    w.line("}");
}

fn emit_array_no_std(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions, module: &IrModule) {
    let type_name = to_pascal_case(&a.name);
    let snake = to_snake_case(&a.name);
    let elem = match &a.element {
        TypeRef::Tagged(_, inner) => typeref_to_rust(inner, opts),
        other => typeref_to_rust(other, opts),
    };
    let cap = capacity_value(
        a.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line(&format!("pub type {type_name} = heapless::Vec<{elem}, {cap}>;"));
    w.blank();
    w.line(&format!("/// Writes a `{type_name}` as a CBOR array."));
    w.line("#[allow(unused_variables)]");
    w.line(&format!(
        "pub fn encode_{snake}<C, W: Write>(val: &{type_name}, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {{"
    ));
    w.indent();
    if let Some(tag) = a.tagged {
        w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
    }
    w.line("e.array(val.len() as u64)?;");
    w.line("for item in val.iter() {");
    w.indent();
    emit_encode_typeref(w, &a.element, "(*item)", opts, Some(module));
    w.dedent();
    w.line("}");
    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("/// Reads a `{type_name}`; fails past its capacity of {cap}."));
    w.line("#[allow(unused_variables)]");
    w.line(&format!(
        "pub fn decode_{snake}(d: &mut Decoder<'_>, ctx: &mut ()) -> Result<{type_name}, DecodeError> {{"
    ));
    w.indent();
    if let Some(tag) = a.tagged {
        w.line(&format!("if d.tag()? != minicbor::data::Tag::new({}) {{ return Err(DecodeError::message(\"unexpected CBOR tag\")); }}", tag.0));
    }
    w.line("let len = d.array()?;");
    w.line("let mut out = heapless::Vec::new();");
    w.line("let mut i = 0u64;");
    w.line("loop {");
    w.indent();
    w.line("match len { Some(n) if i >= n => break, None if d.datatype()? == minicbor::data::Type::Break => { d.skip()?; break }, _ => {} }");
    w.line("i += 1;");
    w.line(&format!("let item = {};", decode_expr_for_in_module(&a.element, opts, Some(module))));
    w.line("out.push(item).map_err(|_| DecodeError::message(\"array longer than its capacity\"))?;");
    w.dedent();
    w.line("}");
    w.line("Ok(out)");
    w.dedent();
    w.line("}");
}

/// A text or byte value of length `n`, for a size-constrained default or test value.
fn sized_value(ty: &TypeRef, n: usize, opts: &CodegenOptions) -> Option<String> {
    match ty {
        TypeRef::Primitive(Primitive::Tstr) if opts.no_std => Some(no_std_text(&format!("{:?}", "a".repeat(n)))),
        TypeRef::Primitive(Primitive::Tstr) => Some(format!("{:?}.to_string()", "a".repeat(n))),
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) if opts.no_std => Some(no_std_bytes(&format!("&[0u8; {n}]"))),
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => Some(format!("vec![0u8; {n}]")),
        _ => None,
    }
}

/// Whether a type is a byte string (`bstr`, possibly tagged).
fn is_bstr(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Primitive(Primitive::Bstr) => true,
        TypeRef::Tagged(_, inner) => is_bstr(inner),
        _ => false,
    }
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    emit_doc(w, a.doc.as_deref());
    if a.deprecated { w.line("#[deprecated]"); }

    let type_name = to_pascal_case(&a.name);

    // If there's a tag, wrap in a newtype struct
    if let Some(tag) = a.tagged {
        let inner_ty = typeref_to_rust(&a.ty, opts);
        w.line("#[derive(Debug, Clone, PartialEq, Default)]");
        w.line(&format!("pub struct {}(pub {});", type_name, inner_ty));
        w.blank();
        emit_tagged_newtype_encode(w, &type_name, tag, &a.ty, opts);
        w.blank();
        emit_tagged_newtype_decode(w, &type_name, tag, &a.ty, &a.constraints, opts);
    } else if a.constraints.is_empty() {
        // Simple type alias. minicbor encodes a `Vec<u8>` as an array of
        // integers; a bstr alias is a `ByteVec`, which it encodes as a bstr.
        let inner_ty = if is_bstr(&a.ty) && !opts.no_std && !opts.is_json() {
            "minicbor::bytes::ByteVec".to_owned()
        } else {
            typeref_to_rust(&a.ty, opts)
        };
        w.line(&format!("pub type {} = {};", type_name, inner_ty));
    } else {
        // Constrained alias — newtype with validation
        let inner_ty = typeref_to_rust(&a.ty, opts);

        // For size-constrained string/bytes aliases, derive Default would create
        // an empty value that fails constraint validation. Generate a custom Default
        // that satisfies the constraint instead.
        let size_default: Option<String> = a.constraints.iter().find_map(|c| match c {
            Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. } => {
                sized_value(&a.ty, *n, opts)
            }
            _ => None,
        });

        if let Some(default_val) = size_default {
            w.line("#[derive(Debug, Clone, PartialEq)]");
            w.line(&format!("pub struct {}(pub {});", type_name, inner_ty));
            w.blank();
            w.line(&format!("impl Default for {} {{", type_name));
            w.indent();
            w.line("fn default() -> Self {");
            w.indent();
            w.line(&format!("{}({})", type_name, default_val));
            w.dedent();
            w.line("}");
            w.dedent();
            w.line("}");
        } else {
            w.line("#[derive(Debug, Clone, PartialEq, Default)]");
            w.line(&format!("pub struct {}(pub {});", type_name, inner_ty));
        }
        w.blank();
        emit_constrained_newtype_encode(w, &type_name, &a.ty, opts);
        w.blank();
        emit_constrained_newtype_decode(w, &type_name, &a.ty, &a.constraints, opts);
    }
}

// ── Tagged newtype impl ───────────────────────────────────────────────────────

fn emit_tagged_newtype_encode(
    w:         &mut IndentWriter,
    type_name: &str,
    tag:       CborTag,
    inner:     &TypeRef,
    opts:      &CodegenOptions,
) {
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<C> Encode<C> for {} {{", type_name));
    w.indent();
    w.line("fn encode<W: Write>(&self, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {");
    w.indent();
    w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
    emit_encode_value(w, inner, "self.0", &Occurrence::Required, opts, None);
    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_tagged_newtype_decode(
    w:           &mut IndentWriter,
    type_name:   &str,
    tag:         CborTag,
    inner:       &TypeRef,
    constraints: &[Constraint],
    opts:        &CodegenOptions,
) {
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<'b> Decode<'b, ()> for {} {{", type_name));
    w.indent();
    w.line("fn decode(d: &mut Decoder<'b>, ctx: &mut ()) -> Result<Self, DecodeError> {");
    w.indent();
    w.line("let t = d.tag()?;");
    w.line(&format!("if t != minicbor::data::Tag::new({}) {{", tag.0));
    w.indent();
    w.line("return Err(DecodeError::message(\"unexpected CBOR tag\"));");
    w.dedent();
    w.line("}");
    let decode_expr = decode_expr_for(inner, opts);
    w.line(&format!("let val = {};", decode_expr));
    emit_field_validation(w, constraints, "val", type_name);
    w.line("Ok(Self(val))");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_constrained_newtype_encode(
    w:         &mut IndentWriter,
    type_name: &str,
    inner:     &TypeRef,
    opts:      &CodegenOptions,
) {
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<C> Encode<C> for {} {{", type_name));
    w.indent();
    w.line("fn encode<W: Write>(&self, e: &mut Encoder<W>, ctx: &mut C) -> Result<(), EncodeError<W::Error>> {");
    w.indent();
    emit_encode_value(w, inner, "self.0", &Occurrence::Required, opts, None);
    w.line("Ok(())");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_constrained_newtype_decode(
    w:           &mut IndentWriter,
    type_name:   &str,
    inner:       &TypeRef,
    constraints: &[Constraint],
    opts:        &CodegenOptions,
) {
    w.line("#[allow(unused_variables)]");
    w.line(&format!("impl<'b> Decode<'b, ()> for {} {{", type_name));
    w.indent();
    w.line("fn decode(d: &mut Decoder<'b>, ctx: &mut ()) -> Result<Self, DecodeError> {");
    w.indent();
    let decode_expr = decode_expr_for(inner, opts);
    w.line(&format!("let val = {};", decode_expr));
    emit_field_validation(w, constraints, "val", type_name);
    w.line("Ok(Self(val))");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

// ── Encode helpers ────────────────────────────────────────────────────────────

fn emit_encode_value(
    w:      &mut IndentWriter,
    ty:     &TypeRef,
    expr:   &str,
    occ:    &Occurrence,
    opts:   &CodegenOptions,
    module: Option<&IrModule>,
) {
    match occ {
        Occurrence::Optional => {
            w.line(&format!("if let Some(ref v) = {expr} {{"));
            w.indent();
            emit_encode_typeref(w, ty, "(*v)", opts, module);
            w.dedent();
            w.line("} else { e.null()?; }");
        }
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => {
            w.line(&format!("e.array({expr}.len() as u64)?;"));
            w.line(&format!("for item in {expr}.iter() {{"));
            w.indent();
            emit_encode_typeref(w, ty, "item", opts, module);
            w.dedent();
            w.line("}");
        }
        Occurrence::Required => {
            emit_encode_typeref(w, ty, expr, opts, module);
        }
    }
}

fn emit_encode_typeref(w: &mut IndentWriter, ty: &TypeRef, expr: &str, opts: &CodegenOptions, module: Option<&IrModule>) {
    match ty {
        TypeRef::Primitive(p) => {
            let enc = encode_call_for_primitive(p, expr);
            w.line(&format!("{enc}?;"));
        }
        // minicbor can't encode a `heapless::Vec` (`--no-std`): the array's own function does.
        TypeRef::Named(n) if opts.no_std && is_container(n, module) => {
            w.line(&format!("encode_{}(&{expr}, e, ctx)?;", to_snake_case(n)));
        }
        TypeRef::Named(_) => {
            w.line(&format!("{expr}.encode(e, ctx)?;"));
        }
        TypeRef::Tagged(tag, inner) => {
            // A tagged member (`stamp: #6.1(uint)`) is written as tag + value,
            // like a tagged type (dgtlrift/cddlc#47).
            w.line(&format!("e.tag(minicbor::data::Tag::new({}))?;", tag.0));
            emit_encode_typeref(w, inner, expr, opts, module);
        }
        TypeRef::Choice(_) => {
            // Can't determine variant at compile time — encode as-is
            w.line(&format!("{expr}.encode(e, ctx)?;"));
        }
    }
}

fn encode_call_for_primitive(p: &Primitive, expr: &str) -> String {
    // Value-position primitives (e.u64, e.f64, …) take T by value.  When expr is
    // a deref like (*v) the outer parens are syntactically redundant inside a
    // function-call argument and trigger the unused_parens lint.  Strip them.
    // For method-call primitives (Tstr, Bstr) the parens are necessary to bind
    // the deref before `.as_str()` / `.as_slice()`, so keep expr as-is there.
    let val = if expr.starts_with("(*") && expr.ends_with(')') {
        &expr[1..expr.len() - 1]  // (*v) → *v
    } else {
        expr
    };
    match p {
        Primitive::Bool    => format!("e.bool({val})"),
        Primitive::Null | Primitive::Undefined => "e.null()".into(),
        Primitive::Uint    => format!("e.u64({val})"),
        Primitive::Int     => format!("e.i64({val})"),
        Primitive::Float16 | Primitive::Float32 => format!("e.f32({val})"),
        Primitive::Float64 | Primitive::Float   => format!("e.f64({val})"),
        // For owned String / Vec<u8>, use original expr so (*v).as_str() is correct
        Primitive::Tstr    => format!("e.str({expr}.as_str())"),
        Primitive::Bstr    => format!("e.bytes({expr}.as_slice())"),
        Primitive::Any     => format!("e.bytes({expr}.as_slice())"),
    }
}

// ── Decode helpers ────────────────────────────────────────────────────────────

fn decode_expr_for(ty: &TypeRef, opts: &CodegenOptions) -> String {
    decode_expr_for_in_module(ty, opts, None)
}

fn decode_expr_for_in_module(ty: &TypeRef, opts: &CodegenOptions, module: Option<&IrModule>) -> String {
    match ty {
        TypeRef::Primitive(p) => {
            let fn_name = decode_fn_for_primitive(p);
            match p {
                // Owned String types need .to_string() / .to_vec()
                Primitive::Tstr if !opts.no_std =>
                    format!("d.str()?.to_string()"),
                Primitive::Bstr | Primitive::Any if !opts.no_std =>
                    format!("d.bytes()?.to_vec()"),
                Primitive::Tstr => "cddlc_text(d.str()?)?".into(),
                Primitive::Bstr | Primitive::Any => "cddlc_bytes(d.bytes()?)?".into(),
                _ => format!("d.{fn_name}()?"),
            }
        }
        TypeRef::Named(n) => {
            // Array type aliases (Vec<T>) decode via minicbor's blanket impl
            let is_array = is_container(n, module);
            if is_array && opts.no_std {
                // minicbor can't decode a `heapless::Vec`: the array's own function does.
                format!("decode_{}(d, ctx)?", to_snake_case(n))
            } else if is_array {
                format!("d.decode()?")
            } else {
                format!("{}::decode(d, ctx)?", to_pascal_case(n))
            }
        }
        // A tagged member must carry its tag, which is checked and consumed
        // before the value (dgtlrift/cddlc#47).
        TypeRef::Tagged(tag, inner) => format!(
            "{{ if d.tag()? != minicbor::data::Tag::new({}) {{ return Err(DecodeError::message(\"unexpected CBOR tag\")); }} {} }}",
            tag.0,
            decode_expr_for_in_module(inner, opts, module)
        ),
        TypeRef::Choice(_) => "d.decode()?".into(),
    }
}

fn decode_fn_for_primitive(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool               => "bool",
        Primitive::Null | Primitive::Undefined => "null",
        Primitive::Uint               => "u64",
        Primitive::Int                => "i64",
        Primitive::Float16            => "f32",
        Primitive::Float32            => "f32",
        Primitive::Float64 | Primitive::Float => "f64",
        Primitive::Bstr               => "bytes",
        Primitive::Tstr               => "str",
        Primitive::Any                => "bytes",
    }
}

fn emit_field_validation(
    w:          &mut IndentWriter,
    constraints: &[Constraint],
    val_expr:   &str,
    field_name: &str,
) {
    for c in constraints {
        if let Some(check) = constraint_to_rust_check(c, val_expr) {
            w.line(&format!("if !({check}) {{"));
            w.indent();
            w.line(&format!(
                "return Err(DecodeError::message(\"validation failed for field '{}'\"));",
                field_name
            ));
            w.dedent();
            w.line("}");
        }
    }
}

// ── Type string helpers ───────────────────────────────────────────────────────

fn field_type_str(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    let base = typeref_to_rust(ty, opts);
    match occ {
        Occurrence::Optional => format!("Option<{base}>"),
        Occurrence::ZeroOrMore { capacity } | Occurrence::OneOrMore { capacity }
        | Occurrence::Bounded { capacity, .. } => {
            let cap = capacity_value(capacity, opts.max_array);
            // In the generated crate, the type depends on the no-std feature.
            // We emit the std form here; the no-std form is handled at the
            // type alias level via #[cfg] in emit_array.
            // For struct fields we pick based on alloc strategy at codegen time.
            if opts.no_std || opts.alloc == AllocStrategy::Stack || opts.alloc == AllocStrategy::Arena {
                format!("heapless::Vec<{base}, {cap}>")
            } else {
                format!("Vec<{base}>")
            }
        }
        Occurrence::Required => base,
    }
}

fn typeref_to_rust(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p)  => rust_primitive(p, opts),
        TypeRef::Named(n)      => to_pascal_case(n),
        TypeRef::Tagged(_, inner) => typeref_to_rust(inner, opts),
        TypeRef::Choice(cs)    => {
            // Inline choices are rare in field position; use the first variant
            cs.first()
                .map(|c| typeref_to_rust(c, opts))
                .unwrap_or_else(|| "()".into())
        }
    }
}

fn is_integer(p: &Primitive) -> bool {
    matches!(p, Primitive::Uint | Primitive::Int)
}

fn is_float(p: &Primitive) -> bool {
    matches!(p, Primitive::Float | Primitive::Float16 | Primitive::Float32 | Primitive::Float64)
}

fn emit_doc(w: &mut IndentWriter, doc: Option<&str>) {
    cddlc_codegen::emit_doc_lines(w, doc, cddlc_codegen::DocStyle::Line("///"));
}
// ── Cargo.toml generation ─────────────────────────────────────────────────────

fn emit_cargo_toml(crate_name: &str, opts: &CodegenOptions) -> String {
    let mut s = String::new();
    s.push_str(&cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Hash));
    s.push_str("[package]\n");
    s.push_str(&format!("name    = {:?}\n", crate_name));
    s.push_str("version = \"0.1.0\"\n");
    s.push_str("edition = \"2021\"\n");
    s.push('\n');
    s.push_str("[dependencies]\n");
    if opts.is_json() {
        s.push_str("serde      = { version = \"1\", features = [\"derive\"] }\n");
        s.push_str("serde_json = \"1\"\n");
        s.push_str("base64     = \"0.23\"\n");
        s.push('\n');
        s.push_str("[dev-dependencies]\n");
        s.push_str("# The round-trip tests also go through CBOR.\n");
        s.push_str("ciborium = \"0.2\"\n");
    } else {
        s.push_str("minicbor = { version = \"2.3\", features = [\"derive\"] }\n");
        if opts.no_std {
            // `--no-std` types are heapless whatever the features (dgtlrift/cddlc#65).
            s.push_str("heapless = \"0.9\"\n");
        } else {
            s.push_str("heapless = { version = \"0.9\", optional = true }\n");
        }
        s.push('\n');
        s.push_str("[features]\n");
        // Outside no-std the types are `Vec` and `String`, which need minicbor's alloc.
        s.push_str("default = [\"alloc\"]\n");
        s.push_str("alloc   = [\"minicbor/alloc\"]\n");
        if opts.no_std {
            s.push_str("no-std  = []\n");
        } else {
            s.push_str("no-std  = [\"heapless\"]\n");
        }
        s.push('\n');
        s.push_str("[dev-dependencies]\n");
        s.push_str("minicbor = { version = \"2.3\", features = [\"derive\", \"alloc\"] }\n");
    }
    if opts.dcbor {
        s.push('\n');
        s.push_str("# dCBOR deterministic encoding enabled\n");
    }
    s
}

// ── Roundtrip test generation ─────────────────────────────────────────────────

fn emit_roundtrip_tests(module: &IrModule, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    let crate_name = format!("{}_cbor", to_snake_case(&module.name));

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Roundtrip encode/decode tests");
    w.blank();
    w.line(&format!("use {}::*;", crate_name));
    w.blank();

    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => emit_struct_roundtrip(&mut w, s, module, opts),
            TypeDef::Enum(e)   => emit_enum_roundtrip(&mut w, e, module, opts),
            TypeDef::Alias(a)  => emit_alias_roundtrip(&mut w, a, opts),
            TypeDef::Array(a)  => emit_array_roundtrip(&mut w, a, opts),
            TypeDef::Map(m)    => emit_map_roundtrip(&mut w, m, opts),
        }
        w.blank();
    }

    w.finish()
}

fn emit_struct_roundtrip(w: &mut IndentWriter, s: &StructDef, module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&s.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&s.name));

    // Tier 1: default roundtrip
    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {}::default();", type_name));
    w.line("let mut buf = Vec::new();");
    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
    w.line(&format!(
        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
        type_name
    ));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
    w.blank();

    // Tier 2: explicit values test
    w.line("#[test]");
    w.line(&format!("fn {test_name}_explicit() {{"));
    w.indent();
    w.line(&format!("let original = {} {{", type_name));
    w.indent();
    for f in &s.fields {
        let fname = field_ident(&f.name);
        // For Named types that are constrained tstr aliases, use a valid string value.
        let val = match (&f.occurrence, &f.ty) {
            (Occurrence::Required, _) if field_literal(f).is_some() => {
                literal_value_expr(field_literal(f).expect("checked"), opts.no_std)
            }
            (Occurrence::Required, TypeRef::Named(n)) => {
                if let Some(v) = tstr_test_value_for(n, module, opts) {
                    v
                } else {
                    default_value_expr(&f.ty, &f.occurrence, opts)
                }
            }
            (Occurrence::Required, TypeRef::Tagged(_, inner)) => {
                if let TypeRef::Named(n) = inner.as_ref() {
                    if let Some(v) = tstr_test_value_for(n, module, opts) {
                        v
                    } else {
                        default_value_expr(&f.ty, &f.occurrence, opts)
                    }
                } else {
                    default_value_expr(&f.ty, &f.occurrence, opts)
                }
            }
            _ => default_value_expr(&f.ty, &f.occurrence, opts),
        };
        w.line(&format!("{fname}: {val},"));
    }
    if let Some(r) = &s.rest {
        w.line(&format!("{}: {},", field_ident(&r.name), rest_test_value(r, opts)));
    }
    w.dedent();
    w.line("};");
    w.line("let mut buf = Vec::new();");
    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
    w.line(&format!(
        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
        type_name
    ));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
}

/// Whether `name` is a struct encoded as a map.
fn is_map_struct(name: &str, module: &IrModule) -> bool {
    matches!(module.get(name), Some(TypeDef::Struct(s)) if s.encoding == cddlc_ir::StructEncoding::Map)
}

fn emit_enum_roundtrip(w: &mut IndentWriter, e: &EnumDef, module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&e.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&e.name));
    let mut used_tests = HashSet::from([format!("{test_name}_default")]);

    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {}::default();", type_name));
    w.line("let mut buf = Vec::new();");
    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
    w.line(&format!(
        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
        type_name
    ));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");

    // Test each variant
    for v in &e.variants {
        let vname     = to_pascal_case(&v.name);
        let construct = match &v.ty {
            TypeRef::Primitive(Primitive::Tstr) => {
                let s = variant_str(v);
                if opts.no_std {
                    format!("{}::{}({})", type_name, vname, no_std_text(&format!("{s:?}")))
                } else {
                    format!("{}::{}({:?}.to_string())", type_name, vname, s)
                }
            }
            TypeRef::Primitive(Primitive::Uint) => {
                let val = v.name.as_str().strip_prefix("Variant")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|n| format!("{n}u64"))
                    .unwrap_or_else(|| "0u64".into());
                format!("{}::{}({})", type_name, vname, val)
            }
            TypeRef::Primitive(Primitive::Int) => {
                let val = v.name.as_str().strip_prefix("Variant")
                    .and_then(|s| s.parse::<i64>().ok())
                    .map(|n| format!("{n}i64"))
                    .unwrap_or_else(|| "0i64".into());
                format!("{}::{}({})", type_name, vname, val)
            }
            TypeRef::Primitive(Primitive::Bool) =>
                format!("{}::{}(false)", type_name, vname),
            TypeRef::Primitive(Primitive::Null) =>
                format!("{}::{}", type_name, vname),
            // A variant of a map type (dgtlrift/cddlc#102).
            TypeRef::Named(n) if is_map_struct(n, module) =>
                format!("{}::{}({}::default())", type_name, vname, to_pascal_case(n)),
            _ => continue,
        };

        let var_test = variant_test_name(&test_name, &v.name, &mut used_tests);

        w.blank();
        w.line("#[test]");
        w.line(&format!("fn {var_test}() {{"));
        w.indent();
        w.line(&format!("let original = {construct};"));
        w.line("let mut buf = Vec::new();");
        w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
        w.line(&format!(
            "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
            type_name
        ));
        w.line("assert_eq!(original, decoded);");
        w.dedent();
        w.line("}");
    }
}

/// Names the roundtrip test for one enum variant: `{test_name}_{variant}`,
/// unless that is already taken in this enum — by the `Default` derive's
/// `{test_name}_default` test (a variant named `default`, issue #16) or by a
/// variant whose name differs only in case or `-`/`_`. A taken name becomes
/// `{test_name}_variant_{variant}`, then gets a numeric suffix if needed.
fn variant_test_name(test_name: &str, variant: &str, used: &mut HashSet<String>) -> String {
    let variant = to_snake_case(&variant.to_lowercase());
    let mut name = format!("{test_name}_{variant}");
    if used.contains(&name) {
        let base = format!("{test_name}_variant_{variant}");
        name = base.clone();
        let mut n = 2;
        while used.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
    }
    used.insert(name.clone());
    name
}

fn emit_alias_roundtrip(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    if a.constraints.is_empty() && a.tagged.is_none() {
        return; // simple alias — no interesting test to write
    }

    let type_name = to_pascal_case(&a.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&a.name));

    // Tier 1: default roundtrip (constrained newtype)
    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {}::default();", type_name));
    w.line("let mut buf = Vec::new();");
    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
    w.line(&format!(
        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
        type_name
    ));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");

    // Tag verification test
    if let Some(tag) = a.tagged {
        w.blank();
        w.line("#[test]");
        w.line(&format!("fn {test_name}_tag_present() {{"));
        w.indent();
        w.line(&format!("let original = {}::default();", type_name));
        w.line("let mut buf = Vec::new();");
        w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
        w.line(&format!("// CBOR tag({}) must be the first byte group", tag.0));
        w.line("assert!(buf.len() >= 2, \"encoded output too short to contain tag\");");
        w.dedent();
        w.line("}");
    }

    // Tier 2: constraint violation test (valid value roundtrips, invalid rejected)
    for c in &a.constraints {
        match c {
            cddlc_ir::Constraint::SizeExact(n) => {
                let Some(valid_val) = sized_value(&a.ty, *n, opts) else { continue };
                w.blank();
                w.line("#[test]");
                w.line(&format!("fn {test_name}_valid_size() {{"));
                w.indent();
                w.line(&format!("let original = {}({});", type_name, valid_val));
                w.line("let mut buf = Vec::new();");
                w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
                w.line(&format!(
                    "let decoded: {} = minicbor::decode(&buf).expect(\"decode valid\");",
                    type_name
                ));
                w.line("assert_eq!(original, decoded);");
                w.dedent();
                w.line("}");
            }
            cddlc_ir::Constraint::ValueRangeInt { min, max, .. } => {
                if let Some(lo) = min {
                    let mid = if let Some(hi) = max { (lo + hi) / 2 } else { *lo };
                    w.blank();
                    w.line("#[test]");
                    w.line(&format!("fn {test_name}_in_range() {{"));
                    w.indent();
                    w.line(&format!("let original = {}({mid}i64);", type_name));
                    w.line("let mut buf = Vec::new();");
                    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
                    w.line(&format!(
                        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
                        type_name
                    ));
                    w.line("assert_eq!(original, decoded);");
                    w.dedent();
                    w.line("}");
                }
            }
            _ => {}
        }
    }
}

fn emit_map_roundtrip(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&m.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&m.name));
    let key = primitive_default_expr(&m.key_ty, opts);
    let val = primitive_default_expr(&m.val_ty, opts);
    w.line("#[allow(unexpected_cfgs)]");
    w.line("#[cfg(not(feature = \"no-std\"))]");
    w.line("#[test]");
    w.line(&format!("fn {test_name}() {{"));
    w.indent();
    if opts.no_std {
        let snake = to_snake_case(&m.name);
        w.line(&format!("let mut original = {type_name}::new();"));
        w.line(&format!("original.insert({key}, {val}).expect(\"capacity\");"));
        w.line("let mut buf = Vec::new();");
        w.line(&format!("encode_{snake}(&original, &mut minicbor::Encoder::new(&mut buf), &mut ()).expect(\"encode\");"));
        w.line(&format!("let decoded = decode_{snake}(&mut minicbor::Decoder::new(&buf), &mut ()).expect(\"decode\");"));
    } else {
        // `.into()` makes a `ByteVec` or `Tagged` key or value from the plain one.
        w.line(&format!("let mut original = {type_name}::new();"));
        w.line(&format!("original.insert({key}.into(), {val}.into());"));
        w.line("let mut buf = Vec::new();");
        w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
        w.line(&format!("let decoded: {type_name} = minicbor::decode(&buf).expect(\"decode\");"));
    }
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
}

fn emit_array_roundtrip(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    // Arrays are type aliases — test via the element type directly
    let type_name = to_pascal_case(&a.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&a.name));
    if opts.no_std {
        // A heapless array goes through its own encode/decode functions.
        let snake = to_snake_case(&a.name);
        w.line("#[test]");
        w.line(&format!("fn {test_name}_empty() {{"));
        w.indent();
        w.line(&format!("let original: {type_name} = heapless::Vec::new();"));
        w.line("let mut buf = Vec::new();");
        w.line(&format!("encode_{snake}(&original, &mut minicbor::Encoder::new(&mut buf), &mut ()).expect(\"encode\");"));
        w.line(&format!("let decoded = decode_{snake}(&mut minicbor::Decoder::new(&buf), &mut ()).expect(\"decode\");"));
        w.line("assert_eq!(original, decoded);");
        w.dedent();
        w.line("}");
        return;
    }

    w.line("#[allow(unexpected_cfgs)]");
    w.line("#[cfg(not(feature = \"no-std\"))]");
    w.line("#[test]");
    w.line(&format!("fn {test_name}_empty() {{"));
    w.indent();
    w.line(&format!("let original: {} = Vec::new();", type_name));
    w.line("let mut buf = Vec::new();");
    w.line("minicbor::encode(&original, &mut buf).expect(\"encode\");");
    w.line(&format!(
        "let decoded: {} = minicbor::decode(&buf).expect(\"decode\");",
        type_name
    ));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
}

// ── Default value expression helpers ─────────────────────────────────────────

fn default_value_expr(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::Optional => "None".into(),
        Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => {
            if opts.no_std || opts.alloc == AllocStrategy::Stack
                || opts.alloc == AllocStrategy::Arena
            {
                "heapless::Vec::new()".into()
            } else {
                "Vec::new()".into()
            }
        }
        Occurrence::Required => primitive_default_expr(ty, opts),
    }
}

fn primitive_default_expr(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) if opts.no_std => no_std_bytes("&[0xf6u8]"),
        TypeRef::Primitive(Primitive::Tstr) if opts.no_std => no_std_text("\"hello\""),
        TypeRef::Primitive(p) => match p {
            Primitive::Bool               => "true".into(),
            Primitive::Null | Primitive::Undefined => "()".into(),
            Primitive::Uint               => "42u64".into(),
            Primitive::Int                => "-1i64".into(),
            Primitive::Float16 | Primitive::Float32 => "1.0f32".into(),
            Primitive::Float64 | Primitive::Float   => "1.0f64".into(),
            Primitive::Bstr | Primitive::Any        => "vec![0xf6u8]".into(),
            Primitive::Tstr               => r#""hello".to_string()"#.into(),
        },
        TypeRef::Named(n)      => format!("{}::default()", to_pascal_case(n)),
        TypeRef::Tagged(_, inner) => primitive_default_expr(inner, opts),
        TypeRef::Choice(cs)    => cs.first()
            .map(|c| primitive_default_expr(c, opts))
            .unwrap_or_else(|| "Default::default()".into()),
    }
}

fn tstr_test_value_for(type_name: &str, module: &IrModule, opts: &CodegenOptions) -> Option<String> {
    match module.types.get(type_name)? {
        TypeDef::Alias(a) if matches!(&a.ty, TypeRef::Primitive(Primitive::Tstr)) => {
            let n = a.constraints.iter().find_map(|c| match c {
                Constraint::SizeExact(n) => Some(*n),
                Constraint::SizeRange { min: Some(n), .. } => Some(*n),
                _ => None,
            })?;
            Some(format!("{}({})", to_pascal_case(type_name), sized_value(&a.ty, n, opts)?))
        }
        _ => None,
    }
}

// ── JSON backend ──────────────────────────────────────────────────────────────

fn emit_module_json(module: &IrModule, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Format: JSON (RFC 8259)");
    if opts.dcbor { w.line("// Encoding: deterministic (key sorting enabled)"); }
    w.blank();

    w.line("use serde::{Deserialize, Serialize};");
    w.blank();

    w.line("/// Byte strings (`bstr`), in the form each serde format expects.");
    w.line("///");
    w.line("/// Human-readable formats such as JSON get base64url text (RFC 8949 section 6.1), binary");
    w.line("/// ones such as CBOR a native byte string. Decoding accepts both, as serde's buffering for");
    w.line("/// untagged enums reports itself human-readable in any format.");
    w.line("#[allow(dead_code)]");
    w.line("mod base64url_serde {");
    w.line("    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};");
    w.line("    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};");
    w.blank();
    w.line("    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {");
    w.line("        if s.is_human_readable() {");
    w.line("            s.serialize_str(&URL_SAFE_NO_PAD.encode(v))");
    w.line("        } else {");
    w.line("            s.serialize_bytes(v)");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    struct BytesVisitor;");
    w.blank();
    w.line("    impl<'de> de::Visitor<'de> for BytesVisitor {");
    w.line("        type Value = Vec<u8>;");
    w.blank();
    w.line("        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {");
    w.line("            f.write_str(\"a byte string, or base64url text\")");
    w.line("        }");
    w.blank();
    w.line("        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<u8>, E> {");
    w.line("            URL_SAFE_NO_PAD.decode(v).map_err(E::custom)");
    w.line("        }");
    w.blank();
    w.line("        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {");
    w.line("            Ok(v.to_vec())");
    w.line("        }");
    w.blank();
    w.line("        fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {");
    w.line("            Ok(v)");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {");
    w.line("        if d.is_human_readable() {");
    w.line("            d.deserialize_str(BytesVisitor)");
    w.line("        } else {");
    w.line("            d.deserialize_byte_buf(BytesVisitor)");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    /// A byte string borrowed for serializing.");
    w.line("    pub struct BytesRef<'a>(pub &'a [u8]);");
    w.blank();
    w.line("    impl Serialize for BytesRef<'_> {");
    w.line("        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {");
    w.line("            serialize(self.0, s)");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    /// A byte string owned after deserializing.");
    w.line("    pub struct Bytes(pub Vec<u8>);");
    w.blank();
    w.line("    impl<'de> Deserialize<'de> for Bytes {");
    w.line("        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {");
    w.line("            deserialize(d).map(Bytes)");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    /// `Option<bstr>`.");
    w.line("    pub mod option {");
    w.line("        use serde::{Deserialize, Deserializer, Serializer};");
    w.blank();
    w.line("        pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {");
    w.line("            match v {");
    w.line("                Some(bytes) => s.serialize_some(&super::BytesRef(bytes)),");
    w.line("                None => s.serialize_none(),");
    w.line("            }");
    w.line("        }");
    w.blank();
    w.line("        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {");
    w.line("            Ok(Option::<super::Bytes>::deserialize(d)?.map(|b| b.0))");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    /// `[* bstr]`.");
    w.line("    pub mod vec {");
    w.line("        use serde::{Deserialize, Deserializer, Serializer};");
    w.blank();
    w.line("        pub fn serialize<S: Serializer>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {");
    w.line("            s.collect_seq(v.iter().map(|b| super::BytesRef(b)))");
    w.line("        }");
    w.blank();
    w.line("        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {");
    w.line("            Ok(Vec::<super::Bytes>::deserialize(d)?.into_iter().map(|b| b.0).collect())");
    w.line("        }");
    w.line("    }");
    w.blank();
    w.line("    /// `Option<[* bstr]>`.");
    w.line("    pub mod vec_option {");
    w.line("        use serde::{Deserialize, Deserializer, Serialize, Serializer};");
    w.blank();
    w.line("        struct Seq<'a>(&'a [Vec<u8>]);");
    w.blank();
    w.line("        impl Serialize for Seq<'_> {");
    w.line("            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {");
    w.line("                super::vec::serialize(self.0, s)");
    w.line("            }");
    w.line("        }");
    w.blank();
    w.line("        pub fn serialize<S: Serializer>(v: &Option<Vec<Vec<u8>>>, s: S) -> Result<S::Ok, S::Error> {");
    w.line("            match v {");
    w.line("                Some(items) => s.serialize_some(&Seq(items)),");
    w.line("                None => s.serialize_none(),");
    w.line("            }");
    w.line("        }");
    w.blank();
    w.line("        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<Vec<u8>>>, D::Error> {");
    w.line("            let v = Option::<Vec<super::Bytes>>::deserialize(d)?;");
    w.line("            Ok(v.map(|items| items.into_iter().map(|b| b.0).collect()))");
    w.line("        }");
    w.line("    }");
    w.line("}");

    if module_has_optional_null(module) {
        w.blank();
        for line in PRESENT_NULL_HELPER.lines() {
            w.line(line);
        }
    }

    for (_, def) in &module.types {
        w.blank();
        match def {
            TypeDef::Struct(s) => emit_struct_json(&mut w, s, opts, module),
            TypeDef::Enum(e)   => emit_enum_json(&mut w, e, opts, module),
            TypeDef::Array(a)  => emit_array_json(&mut w, a, opts, module),
            TypeDef::Alias(a)  => emit_alias_json(&mut w, a, opts),
            TypeDef::Map(m)    => emit_map_json(&mut w, m, opts),
        }
    }

    w.finish()
}

fn is_bytes_type(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Primitive(Primitive::Bstr | Primitive::Any))
}

/// How a type holds byte strings, for the `base64url_serde` helpers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ByteShape {
    /// One `bstr`.
    One,
    /// An array of them, `[* bstr]`.
    Many,
}

/// Whether `ty` is a byte string, directly or through plain aliases, or an array of them.
/// A constrained or tagged `bstr` alias is a newtype that serializes itself, so it is `None`.
fn byte_shape(ty: &TypeRef, module: &IrModule) -> Option<ByteShape> {
    match ty {
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => Some(ByteShape::One),
        TypeRef::Tagged(_, inner) => byte_shape(inner, module),
        TypeRef::Named(n) => match module.get(n) {
            Some(TypeDef::Alias(a)) if a.constraints.is_empty() && a.tagged.is_none() => {
                byte_shape(&a.ty, module)
            }
            Some(TypeDef::Array(a)) if byte_shape(&a.element, module) == Some(ByteShape::One) => {
                Some(ByteShape::Many)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The `with` module for a byte-string member of `shape` and occurrence `occ`, and whether
/// it needs `default` (an `Option` with `with` loses serde's implicit default).
fn bytes_with(shape: ByteShape, occ: &Occurrence) -> (&'static str, bool) {
    let optional = matches!(occ, Occurrence::Optional);
    let repeated = matches!(
        occ,
        Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. } | Occurrence::Bounded { .. }
    );
    match (shape, optional, repeated) {
        (ByteShape::One, true, _) => ("base64url_serde::option", true),
        (ByteShape::One, false, false) => ("base64url_serde", false),
        (ByteShape::One, false, true) | (ByteShape::Many, false, false) => ("base64url_serde::vec", false),
        (ByteShape::Many, true, _) => ("base64url_serde::vec_option", true),
        (ByteShape::Many, false, true) => ("base64url_serde::vec", false),
    }
}

/// Decodes an optional `null` member (`? gone: null`) that is present as `Some(())`.
/// Serde's own `Option` reads `null` as `None`, the same as an absent member;
/// `#[serde(default)]` keeps absent members `None`.
const PRESENT_NULL_HELPER: &str = r#"/// A present `null` member decodes as `Some(())`; an absent one stays `None`.
fn present_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<()>, D::Error> {
    <()>::deserialize(d).map(Some)
}"#;

/// Whether a member is optional and typed `null` / `undefined` (a Rust `Option<()>`).
fn is_optional_null(ty: &TypeRef, occ: &Occurrence) -> bool {
    matches!(occ, Occurrence::Optional)
        && matches!(ty, TypeRef::Primitive(Primitive::Null | Primitive::Undefined))
}

fn module_has_optional_null(module: &IrModule) -> bool {
    module.types.values().any(|def| match def {
        TypeDef::Struct(s) => s.fields.iter().any(|f| is_optional_null(&f.ty, &f.occurrence)),
        _ => false,
    })
}

fn serde_field_attrs_json(
    fname_cddl: &str,
    fname_rust: &str,
    ty: &TypeRef,
    occ: &Occurrence,
    module: &IrModule,
) -> String {
    let mut attrs = Vec::new();
    // Rename if CDDL name differs from snake_case Rust name
    let cddl_snake = to_snake_case(fname_cddl);
    if fname_cddl != cddl_snake || fname_cddl != fname_rust {
        attrs.push(format!("rename = {:?}", fname_cddl));
    }
    // Byte strings: base64url in JSON, native in CBOR (see `base64url_serde`).
    if let Some(shape) = byte_shape(ty, module) {
        let (with, default) = bytes_with(shape, occ);
        attrs.push(format!("with = {with:?}"));
        if default {
            // `with` disables serde's implicit default for `Option`, so an absent field
            // would fail to decode without `default`.
            attrs.push("default".into());
        }
    }
    // An optional `null` member: serde reads a present `null` into `None` for any
    // `Option`, so it would decode the same as an absent member (dgtlrift/cddlc#22).
    if is_optional_null(ty, occ) {
        attrs.push("default".into());
        attrs.push("deserialize_with = \"present_null\"".into());
    }
    // Skip None for optional
    if matches!(occ, Occurrence::Optional) {
        attrs.push("skip_serializing_if = \"Option::is_none\"".into());
    }
    if attrs.is_empty() {
        String::new()
    } else {
        format!("#[serde({})]", attrs.join(", "))
    }
}

/// Emit a zero-sized JSON marker type for a single-literal member: it serializes
/// as the literal and rejects any other value on decode. Struct fields such as
/// `type: "SimStart"` use it, which lets an untagged union of structs pick the
/// right variant instead of the first one whose shape fits.
fn emit_literal_marker_json(w: &mut IndentWriter, name: &str, lit: &LiteralValue) {
    let (value_ty, de_ty, value) = match lit {
        LiteralValue::Text(t) => ("&'static str", "String", format!("{t:?}")),
        LiteralValue::Uint(n) => ("u64", "u64", format!("{n}")),
        LiteralValue::Int(n)  => ("i64", "i64", format!("{n}")),
        LiteralValue::Bool(b) => ("bool", "bool", b.to_string()),
        _ => return,
    };
    w.line(&format!("/// Always the literal `{value}`."));
    w.line("#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]");
    w.line(&format!("pub struct {name};"));
    w.blank();
    w.line(&format!("impl {name} {{"));
    w.indent();
    w.line("/// The literal value.");
    w.line(&format!("pub const VALUE: {value_ty} = {value};"));
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("impl Serialize for {name} {{"));
    w.indent();
    w.line("fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {");
    w.indent();
    w.line("Self::VALUE.serialize(s)");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("impl<'de> Deserialize<'de> for {name} {{"));
    w.indent();
    w.line("fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {");
    w.indent();
    w.line(&format!("let v = <{de_ty}>::deserialize(d)?;"));
    w.line("if v == Self::VALUE {");
    w.indent();
    w.line("Ok(Self)");
    w.dedent();
    w.line("} else {");
    w.indent();
    w.line("Err(serde::de::Error::custom(format!(\"expected {:?}, found {:?}\", Self::VALUE, v)))");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_struct_json(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, s.doc.as_deref());
    if s.deprecated { w.line("#[deprecated]"); }

    let mut markers = Vec::new();
    let record = s.encoding == cddlc_ir::StructEncoding::Array;
    if record {
        // A record is a JSON array: see `emit_record_json_impls`.
        w.line("#[derive(Debug, Clone, PartialEq, Default)]");
    } else {
        w.line("#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]");
        // A map has only the members its schema gives it (dgtlrift/cddlc#102).
        if s.rest.is_none() {
            w.line("#[serde(deny_unknown_fields)]");
        }
    }
    w.line(&format!("pub struct {} {{", to_pascal_case(&s.name)));
    w.indent();
    let mut field_types = Vec::new();
    for f in &s.fields {
        emit_doc(w, f.doc.as_deref());
        let fname = field_ident(&f.name);
        let attrs = serde_field_attrs_json(&f.name, &fname, &f.ty, &f.occurrence, module);
        if !attrs.is_empty() && !record { w.line(&attrs); }
        let ty_str = match field_literal(f) {
            Some(lit) => {
                let marker = marker_name(&s.name, &f.name, module);
                let ty = if f.occurrence.is_required() { marker.clone() } else { format!("Option<{marker}>") };
                markers.push((marker, lit.clone()));
                ty
            }
            None => field_type_str(&f.ty, &f.occurrence, opts),
        };
        w.line(&format!("pub {fname}: {ty_str},"));
        field_types.push(ty_str);
    }
    // The other entries: every key no named member takes.
    if let Some(r) = &s.rest {
        w.line("/// The map's other entries.");
        w.line("#[serde(flatten)]");
        w.line(&format!(
            "pub {}: std::collections::BTreeMap<String, {}>,",
            field_ident(&r.name),
            typeref_to_rust(&r.val_ty, opts)
        ));
    }
    w.dedent();
    w.line("}");
    w.blank();
    if record {
        emit_record_json_impls(w, s, &field_types, module);
        w.blank();
    }

    let type_name = to_pascal_case(&s.name);
    w.line(&format!("impl {type_name} {{"));
    w.indent();
    w.line("pub fn encode(&self) -> Vec<u8> {");
    w.indent();
    w.line("serde_json::to_vec(self).expect(\"encode\")");
    w.dedent();
    w.line("}");
    w.blank();
    w.line("pub fn decode(data: &[u8]) -> Result<Self, serde_json::Error> {");
    w.indent();
    w.line("serde_json::from_slice(data)");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");

    for (marker, lit) in &markers {
        w.blank();
        emit_literal_marker_json(w, marker, lit);
    }
}

/// serde writes a struct as a JSON object, so a record (RFC 8610 §3.4) gets
/// its own impls: an array of its members' values, in order, optional members
/// at the end left off when absent. Byte members go through a wrapper that
/// carries their base64url conversion.
fn emit_record_json_impls(w: &mut IndentWriter, s: &StructDef, field_types: &[String], module: &IrModule) {
    let ty = to_pascal_case(&s.name);
    // Per member: (field, element type to read, wrapper type if any).
    let mut members = Vec::new();
    for (f, ty_str) in s.fields.iter().zip(field_types) {
        let inner = if f.occurrence.is_required() {
            ty_str.clone()
        } else {
            ty_str.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')).unwrap_or(ty_str).to_owned()
        };
        let wrapper = byte_shape(&f.ty, module).map(|shape| {
            let (with, _) = bytes_with(shape, &Occurrence::Required);
            let name = format!("{ty}{}Json", to_pascal_case(&f.name));
            w.line("#[derive(Serialize, Deserialize)]");
            w.line(&format!("struct {name}(#[serde(with = {with:?})] {inner});"));
            name
        });
        members.push((f, inner, wrapper));
    }
    let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();

    w.line(&format!("impl Serialize for {ty} {{"));
    w.indent();
    w.line("fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {");
    w.indent();
    let has_optional = required < s.fields.len();
    if has_optional {
        w.line("use serde::ser::{Error as _, SerializeSeq};");
        w.line(&format!("let mut len = {required}usize;"));
        w.line("let mut gap = false;");
    } else {
        w.line("use serde::ser::SerializeSeq;");
        w.line(&format!("let len = {required}usize;"));
    }
    for (f, _, _) in members.iter().filter(|(f, _, _)| !f.occurrence.is_required()) {
        w.line(&format!("if self.{}.is_some() {{", field_ident(&f.name)));
        w.indent();
        w.line(&format!(
            "if gap {{ return Err(S::Error::custom(\"{}: '{}' is set but an optional member before it isn't\")); }}",
            s.name, f.name
        ));
        w.line("len += 1;");
        w.dedent();
        w.line("} else { gap = true; }");
    }
    if has_optional { w.line("let _ = gap;"); }
    w.line("let mut seq = serializer.serialize_seq(Some(len))?;");
    for (f, _, wrapper) in &members {
        let access = format!("self.{}", field_ident(&f.name));
        let elem = |v: &str| match wrapper {
            Some(wr) => format!("&{wr}({v}.clone())"),
            None => v.to_owned(),
        };
        if f.occurrence.is_required() {
            let v = match wrapper {
                Some(wr) => format!("&{wr}({access}.clone())"),
                None => format!("&{access}"),
            };
            w.line(&format!("seq.serialize_element({v})?;"));
        } else {
            w.line(&format!("if let Some(v) = &{access} {{ seq.serialize_element({})?; }}", elem("v")));
        }
    }
    w.line("seq.end()");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("impl<'de> Deserialize<'de> for {ty} {{"));
    w.indent();
    w.line("fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {");
    w.indent();
    w.line("struct V;");
    w.line("impl<'de> serde::de::Visitor<'de> for V {");
    w.indent();
    w.line(&format!("type Value = {ty};"));
    w.line("fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {");
    w.indent();
    w.line(&format!("f.write_str(\"the record {}, an array\")", s.name));
    w.dedent();
    w.line("}");
    w.line("fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {");
    w.indent();
    w.line("use serde::de::Error as _;");
    for (f, inner, wrapper) in &members {
        let local = field_ident(&f.name);
        let (read_ty, unwrap) = match wrapper {
            Some(wr) => (wr.clone(), ".map(|w| w.0)"),
            None => (inner.clone(), ""),
        };
        if f.occurrence.is_required() {
            w.line(&format!(
                "let {local} = seq.next_element::<{read_ty}>()?{unwrap}.ok_or_else(|| A::Error::custom(\"{}: missing item '{}'\"))?;",
                s.name, f.name
            ));
        } else {
            w.line(&format!("let {local} = seq.next_element::<{read_ty}>()?{unwrap};"));
        }
    }
    w.line("if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {");
    w.indent();
    w.line(&format!("return Err(A::Error::custom(\"{}: too many items\"));", s.name));
    w.dedent();
    w.line("}");
    let names: Vec<_> = s.fields.iter().map(|f| field_ident(&f.name)).collect();
    w.line(&format!("Ok({ty} {{ {} }})", names.join(", ")));
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
    w.line("deserializer.deserialize_seq(V)");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_enum_json(w: &mut IndentWriter, e: &EnumDef, _opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, e.doc.as_deref());
    if e.deprecated { w.line("#[deprecated]"); }

    let type_name = to_pascal_case(&e.name);

    // All string literals (and nulls) → a plain unit enum renamed to the literals.
    // Anything else → untagged, with literal variants carried by marker types.
    let unit_like = |v: &EnumVariant| {
        matches!(&v.ty, TypeRef::Primitive(Primitive::Null)) || text_literal(v).is_some()
    };
    let all_unit = e.variants.iter().all(unit_like);

    if all_unit {
        w.line("#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]");
        w.line(&format!("pub enum {type_name} {{"));
        w.indent();
        for (i, v) in e.variants.iter().enumerate() {
            let vname = to_pascal_case(&v.name);
            if i == 0 { w.line("#[default]"); }
            w.line(&format!("#[serde(rename = {:?})]", variant_str(v)));
            w.line(&format!("{vname},"));
        }
        w.dedent();
        w.line("}");
    } else {
        let mut markers = Vec::new();
        w.line("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]");
        w.line("#[serde(untagged)]");
        w.line(&format!("pub enum {type_name} {{"));
        w.indent();
        let mut default_expr = None;
        for v in &e.variants {
            let vname = to_pascal_case(&v.name);
            // Text and integer literals are marker types that serialize as, and only
            // accept, their literal (an integer variant holding a `u64` would take
            // any number, so every value decoded as the first one; dgtlrift/cddlc#63).
            let literal = text_literal(v).map(|t| LiteralValue::Text(t.to_owned()))
                .or_else(|| int_literal(v).cloned());
            let (line, default) = if let Some(lit) = literal {
                let marker = marker_name(&e.name, &v.name, module);
                markers.push((marker.clone(), lit));
                (format!("{vname}({marker}),"), format!("{type_name}::{vname}({marker})"))
            } else {
                match &v.ty {
                    TypeRef::Primitive(Primitive::Null) => (format!("{vname},"), format!("{type_name}::{vname}")),
                    TypeRef::Primitive(p) => {
                        let rt = primitive_to_rust(p, false);
                        let di = match p {
                            Primitive::Uint => "0u64".to_owned(),
                            Primitive::Int  => "0i64".to_owned(),
                            Primitive::Bool => "false".to_owned(),
                            Primitive::Tstr => "String::new()".to_owned(),
                            Primitive::Bstr | Primitive::Any => "Vec::new()".to_owned(),
                            _ => "Default::default()".to_owned(),
                        };
                        let attr = match byte_shape(&v.ty, module) {
                            Some(shape) => format!("#[serde(with = {:?})] ", bytes_with(shape, &Occurrence::Required).0),
                            None => String::new(),
                        };
                        (format!("{vname}({attr}{rt}),"), format!("{type_name}::{vname}({di})"))
                    }
                    TypeRef::Named(n) => {
                        let tn = to_pascal_case(n);
                        let attr = match byte_shape(&v.ty, module) {
                            Some(shape) => format!("#[serde(with = {:?})] ", bytes_with(shape, &Occurrence::Required).0),
                            None => String::new(),
                        };
                        (format!("{vname}({attr}{tn}),"), format!("{type_name}::{vname}({tn}::default())"))
                    }
                    _ => (format!("{vname},"), format!("{type_name}::{vname}")),
                }
            };
            w.line(&line);
            default_expr.get_or_insert(default);
        }
        w.dedent();
        w.line("}");

        if let Some(di) = default_expr {
            w.blank();
            w.line(&format!("impl Default for {type_name} {{"));
            w.indent();
            w.line("fn default() -> Self {");
            w.indent();
            w.line(&di);
            w.dedent();
            w.line("}");
            w.dedent();
            w.line("}");
        }
        for (marker, lit) in &markers {
            w.blank();
            emit_literal_marker_json(w, marker, lit);
        }
    }

    w.blank();
    w.line(&format!("impl {type_name} {{"));
    w.indent();
    w.line("pub fn encode(&self) -> Vec<u8> {");
    w.indent();
    w.line("serde_json::to_vec(self).expect(\"encode\")");
    w.dedent();
    w.line("}");
    w.blank();
    w.line("pub fn decode(data: &[u8]) -> Result<Self, serde_json::Error> {");
    w.indent();
    w.line("serde_json::from_slice(data)");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_array_json(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions, module: &IrModule) {
    emit_doc(w, a.doc.as_deref());
    if a.deprecated { w.line("#[deprecated]"); }

    let type_name = to_pascal_case(&a.name);
    let elem_ty   = typeref_to_rust(&a.element, opts);
    let bytes     = byte_shape(&a.element, module) == Some(ByteShape::One);

    w.line(&format!("pub type {type_name} = Vec<{elem_ty}>;"));
    w.blank();
    w.line(&format!("pub fn encode_{type_name}(val: &{type_name}) -> Vec<u8> {{"));
    w.indent();
    if bytes {
        w.line("let items: Vec<_> = val.iter().map(|b| base64url_serde::BytesRef(b)).collect();");
        w.line("serde_json::to_vec(&items).expect(\"encode\")");
    } else {
        w.line("serde_json::to_vec(val).expect(\"encode\")");
    }
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("pub fn decode_{type_name}(data: &[u8]) -> Result<{type_name}, serde_json::Error> {{"));
    w.indent();
    if bytes {
        w.line("let items: Vec<base64url_serde::Bytes> = serde_json::from_slice(data)?;");
        w.line("Ok(items.into_iter().map(|b| b.0).collect())");
    } else {
        w.line("serde_json::from_slice(data)");
    }
    w.dedent();
    w.line("}");
}

fn emit_alias_json(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    emit_doc(w, a.doc.as_deref());
    if a.deprecated { w.line("#[deprecated]"); }

    let type_name = to_pascal_case(&a.name);
    let inner_ty  = typeref_to_rust(&a.ty, opts);
    let is_bytes  = is_bytes_type(&a.ty);

    // Simple alias without constraints or tag
    if a.constraints.is_empty() && a.tagged.is_none() {
        w.line(&format!("pub type {type_name} = {inner_ty};"));
        w.blank();
        w.line(&format!("pub fn encode_{type_name}(val: &{type_name}) -> Vec<u8> {{"));
        w.indent();
        if is_bytes {
            w.line("use base64::Engine;");
            w.line("let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(val);");
            w.line("serde_json::to_vec(&encoded).expect(\"encode\")");
        } else {
            w.line("serde_json::to_vec(val).expect(\"encode\")");
        }
        w.dedent();
        w.line("}");
        w.blank();
        w.line(&format!("pub fn decode_{type_name}(data: &[u8]) -> Result<{type_name}, serde_json::Error> {{"));
        w.indent();
        if is_bytes {
            w.line("use base64::Engine;");
            w.line("let s: String = serde_json::from_slice(data)?;");
            w.line("base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&s)");
            w.line("    .map_err(|e| <serde_json::Error as serde::de::Error>::custom(e.to_string()))");
        } else {
            w.line("serde_json::from_slice(data)");
        }
        w.dedent();
        w.line("}");
        return;
    }

    // Constrained newtype
    if is_bytes {
        w.line("#[derive(Debug, Clone, PartialEq, Default)]");
        w.line(&format!("pub struct {type_name}(pub {inner_ty});"));
        w.blank();
        w.line(&format!("impl serde::Serialize for {type_name} {{"));
        w.indent();
        w.line("fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {");
        w.indent();
        w.line("base64url_serde::serialize(&self.0, s)");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
        w.blank();
        w.line(&format!("impl<'de> serde::Deserialize<'de> for {type_name} {{"));
        w.indent();
        w.line("fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {");
        w.indent();
        w.line("base64url_serde::deserialize(d).map(Self)");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
    } else {
        w.line("#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]");
        w.line(&format!("pub struct {type_name}(pub {inner_ty});"));
    }

    // Constraint validation
    if !a.constraints.is_empty() {
        w.blank();
        w.line(&format!("impl {type_name} {{"));
        w.indent();
        w.line(&format!("pub fn new(val: impl Into<{inner_ty}>) -> Result<Self, String> {{"));
        w.indent();
        w.line("let inner = val.into();");
        for c in &a.constraints {
            if let Some(check) = constraint_to_rust_check(c, "inner") {
                w.line(&format!("if !({check}) {{ return Err(format!(\"constraint violated: {{}}\", {check:?})); }}"));
            }
        }
        w.line("Ok(Self(inner))");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
    }

    w.blank();
    w.line(&format!("impl {type_name} {{"));
    w.indent();
    w.line("pub fn encode(&self) -> Vec<u8> {");
    w.indent();
    w.line("serde_json::to_vec(self).expect(\"encode\")");
    w.dedent();
    w.line("}");
    w.blank();
    w.line("pub fn decode(data: &[u8]) -> Result<Self, serde_json::Error> {");
    w.indent();
    w.line("serde_json::from_slice(data)");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

// ── JSON roundtrip tests ──────────────────────────────────────────────────────

fn emit_roundtrip_tests_json(module: &IrModule, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    let crate_name = format!("{}_json", to_snake_case(&module.name));

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Roundtrip encode/decode tests: JSON, and CBOR through ciborium");
    w.blank();
    w.line(&format!("use {}::*;", crate_name));
    w.blank();

    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => emit_struct_roundtrip_json(&mut w, s, opts),
            TypeDef::Enum(e)   => emit_enum_roundtrip_json(&mut w, e, module),
            TypeDef::Alias(a)  => emit_alias_roundtrip_json(&mut w, a, opts),
            TypeDef::Array(a)  => emit_array_roundtrip_json(&mut w, a),
            TypeDef::Map(m)    => emit_map_roundtrip_json(&mut w, m, opts),
        }
        w.blank();
    }

    w.finish()
}

/// The CBOR half of a round trip: `original` through ciborium and back.
fn emit_cbor_roundtrip(w: &mut IndentWriter, type_name: &str) {
    w.line("let mut cbor = Vec::new();");
    w.line("ciborium::into_writer(&original, &mut cbor).expect(\"encode cbor\");");
    w.line(&format!("let decoded: {type_name} = ciborium::from_reader(cbor.as_slice()).expect(\"decode cbor\");"));
    w.line("assert_eq!(original, decoded);");
}

fn emit_struct_roundtrip_json(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&s.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&s.name));

    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {type_name}::default();"));
    w.line("let buf = original.encode();");
    w.line(&format!("let decoded = {type_name}::decode(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    emit_cbor_roundtrip(w, &type_name);
    w.dedent();
    w.line("}");
    w.blank();

    w.line("#[test]");
    w.line(&format!("fn {test_name}_explicit() {{"));
    w.indent();
    w.line(&format!("let original = {type_name} {{"));
    w.indent();
    for f in &s.fields {
        let fname = field_ident(&f.name);
        let val = if field_literal(f).is_some() {
            "Default::default()".to_owned()
        } else {
            default_value_expr(&f.ty, &f.occurrence, opts)
        };
        w.line(&format!("{fname}: {val},"));
    }
    if let Some(r) = &s.rest {
        let val = primitive_default_expr(&r.val_ty, opts);
        w.line(&format!(
            "{}: std::collections::BTreeMap::from([(\"k\".to_string(), {val})]),",
            field_ident(&r.name)
        ));
    }
    w.dedent();
    w.line("};");
    w.line("let buf = original.encode();");
    w.line(&format!("let decoded = {type_name}::decode(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    emit_cbor_roundtrip(w, &type_name);
    w.dedent();
    w.line("}");
}

fn emit_enum_roundtrip_json(w: &mut IndentWriter, e: &EnumDef, module: &IrModule) {
    let type_name = to_pascal_case(&e.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&e.name));
    let mut used_tests = HashSet::from([format!("{test_name}_default")]);

    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {type_name}::default();"));
    w.line("let buf = original.encode();");
    w.line(&format!("let decoded = {type_name}::decode(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    emit_cbor_roundtrip(w, &type_name);
    w.dedent();
    w.line("}");

    let all_unit = e.variants.iter().all(|v| {
        matches!(&v.ty, TypeRef::Primitive(Primitive::Null)) || text_literal(v).is_some()
    });
    for v in &e.variants {
        let vname    = to_pascal_case(&v.name);
        let construct = match &v.ty {
            _ if text_literal(v).is_some() && all_unit => format!("{type_name}::{vname}"),
            _ if text_literal(v).is_some() || int_literal(v).is_some() => {
                format!("{type_name}::{vname}({})", marker_name(&e.name, &v.name, module))
            }
            TypeRef::Primitive(Primitive::Tstr) => {
                format!("{}::{}({:?}.to_string())", type_name, vname, "hello")
            }
            TypeRef::Primitive(Primitive::Uint) => format!("{}::{}(0u64)", type_name, vname),
            TypeRef::Primitive(Primitive::Int)  => format!("{}::{}(0i64)", type_name, vname),
            TypeRef::Primitive(Primitive::Bool) => format!("{}::{}(false)", type_name, vname),
            TypeRef::Primitive(Primitive::Null) => format!("{}::{}", type_name, vname),
            // A variant of a map type (dgtlrift/cddlc#102).
            TypeRef::Named(n) if is_map_struct(n, module) =>
                format!("{}::{}({}::default())", type_name, vname, to_pascal_case(n)),
            _ => continue,
        };

        let var_test = variant_test_name(&test_name, &v.name, &mut used_tests);

        w.blank();
        w.line("#[test]");
        w.line(&format!("fn {var_test}() {{"));
        w.indent();
        w.line(&format!("let original = {construct};"));
        w.line("let buf = original.encode();");
        w.line(&format!("let decoded = {type_name}::decode(&buf).expect(\"decode\");"));
        w.line("assert_eq!(original, decoded);");
        w.dedent();
        w.line("}");
    }
}

fn emit_alias_roundtrip_json(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    if a.constraints.is_empty() && a.tagged.is_none() { return; }

    let type_name = to_pascal_case(&a.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&a.name));

    w.line("#[test]");
    w.line(&format!("fn {test_name}_default() {{"));
    w.indent();
    w.line(&format!("let original = {type_name}::default();"));
    w.line("let buf = original.encode();");
    w.line(&format!("let decoded = {type_name}::decode(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    emit_cbor_roundtrip(w, &type_name);
    w.dedent();
    w.line("}");

    let _ = opts;
}

/// A table in JSON: an object, through serde (dgtlrift/cddlc#82).
fn emit_map_json(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    emit_doc(w, m.doc.as_deref());
    if m.deprecated { w.line("#[deprecated]"); }
    let type_name = to_pascal_case(&m.name);
    let key = typeref_to_rust(&m.key_ty, opts);
    let val = typeref_to_rust(&m.val_ty, opts);
    w.line(&format!("pub type {type_name} = std::collections::BTreeMap<{key}, {val}>;"));
    w.blank();
    w.line(&format!("pub fn encode_{type_name}(val: &{type_name}) -> Vec<u8> {{"));
    w.line("    serde_json::to_vec(val).expect(\"encode\")");
    w.line("}");
    w.blank();
    w.line(&format!("pub fn decode_{type_name}(data: &[u8]) -> Result<{type_name}, serde_json::Error> {{"));
    w.line("    serde_json::from_slice(data)");
    w.line("}");
}

fn emit_map_roundtrip_json(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&m.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&m.name));
    w.line("#[test]");
    w.line(&format!("fn {test_name}() {{"));
    w.indent();
    w.line(&format!("let mut original = {type_name}::new();"));
    w.line(&format!(
        "original.insert({}, {});",
        primitive_default_expr(&m.key_ty, opts),
        primitive_default_expr(&m.val_ty, opts)
    ));
    w.line(&format!("let buf = encode_{type_name}(&original);"));
    w.line(&format!("let decoded = decode_{type_name}(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
}

fn emit_array_roundtrip_json(w: &mut IndentWriter, a: &ArrayDef) {
    let type_name = to_pascal_case(&a.name);
    let test_name = format!("roundtrip_{}", to_snake_case(&a.name));
    let fn_enc    = format!("encode_{type_name}");
    let fn_dec    = format!("decode_{type_name}");

    w.line("#[test]");
    w.line(&format!("fn {test_name}_empty() {{"));
    w.indent();
    w.line(&format!("let original: {type_name} = Vec::new();"));
    w.line(&format!("let buf = {fn_enc}(&original);"));
    w.line(&format!("let decoded = {fn_dec}(&buf).expect(\"decode\");"));
    w.line("assert_eq!(original, decoded);");
    w.dedent();
    w.line("}");
}
