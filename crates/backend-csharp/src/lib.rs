/// C# / .NET Core backend for cddlc.
///
/// Emits a .NET 8 library project:
///   <module>-cbor/
///     <module>-cbor.csproj   ← net8.0, System.Formats.Cbor, xUnit test ref
///     src/<Module>.cs        ← types + Encode/Decode
///     tests/RoundtripTests.cs ← xUnit roundtrip tests
use cddlc_codegen::{
    to_pascal_case, to_snake_case, Backend, CodegenError, CodegenOptions, CodegenOutput,
    GeneratedFile, IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, IrModule, LiteralValue, MapDef,
    Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

pub struct CSharpBackend;

impl Backend for CSharpBackend {
    fn language_name(&self) -> &str {
        "C#"
    }
    fn file_extension(&self) -> &str {
        "cs"
    }

    fn generate(
        &self,
        module: &IrModule,
        opts: &CodegenOptions,
    ) -> Result<CodegenOutput, CodegenError> {
        cddlc_codegen::check_rest_keys(module, opts, "C#", true)?;
        // A rule whose class would clash with a member of its JSON converter
        // (`write` → `Write`) is `<module>-<rule>` (dgtlrift/cddlc#191).
        let renamed =
            cddlc_codegen::rename_types(module, &cddlc_codegen::csharp_reserved_renames(module));
        let module = &renamed;
        // The variants that hold their enum again (dgtlrift/cddlc#224).
        RECURSIVE_VARIANTS.with(|r| {
            *r.borrow_mut() = module
                .types
                .iter()
                .filter_map(|(n, d)| match d {
                    TypeDef::Enum(e) => Some((n, e)),
                    _ => None,
                })
                .flat_map(|(n, e)| {
                    e.variants
                        .iter()
                        .filter(|v| cddlc_codegen::is_indirect(n, &v.ty, module))
                        .map(move |v| (n.to_string(), v.name.to_string()))
                })
                .collect()
        });
        let name = module.name.as_str();
        let fmt_tag = if opts.is_json() { "json" } else { "cbor" };
        let crate_dir = format!("{name}-{fmt_tag}");
        let ns = to_pascal_case(name);

        Ok(CodegenOutput {
            files: vec![
                GeneratedFile {
                    path: format!("{crate_dir}/{name}-{fmt_tag}.csproj"),
                    content: emit_csproj(name, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/src/{ns}.cs"),
                    content: emit_source(module, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/tests/RoundtripTests.cs"),
                    content: emit_tests(module, opts),
                },
            ],
        })
    }
}

// ── .csproj ───────────────────────────────────────────────────────────────────

fn emit_csproj(name: &str, opts: &CodegenOptions) -> String {
    let fmt_tag = if opts.is_json() { "json" } else { "cbor" };
    let ns = to_pascal_case(name);
    let cbor_ref = if opts.is_json() {
        "".into()
    } else {
        "  <ItemGroup>\n    <!-- CBOR runtime ships in-box with .NET 5+ -->\n    <PackageReference Include=\"System.Formats.Cbor\" Version=\"8.*\" />\n  </ItemGroup>\n\n".to_owned()
    };
    let header = cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Xml);
    format!(
        r#"{header}<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <AllowUnsafeBlocks>false</AllowUnsafeBlocks>
    <RootNamespace>{ns}</RootNamespace>
    <AssemblyName>{name}-{fmt_tag}</AssemblyName>
  </PropertyGroup>

{cbor_ref}  <ItemGroup Label="Test dependencies (only used by test project)">
    <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.*" />
    <PackageReference Include="xunit"                  Version="2.*" />
    <PackageReference Include="xunit.runner.visualstudio" Version="2.*" />
  </ItemGroup>
</Project>
"#
    )
}

// ── Source file ───────────────────────────────────────────────────────────────

fn emit_source(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = to_pascal_case(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    let fmt_label = if opts.is_json() {
        "JSON (RFC 8259)"
    } else {
        "CBOR (RFC 8949)"
    };
    w.line(&format!("// Format: {fmt_label}"));
    if opts.dcbor {
        w.line("// Encoding: deterministic (dCBOR)");
    }
    w.blank();
    w.line("using System;");
    w.line("using System.Collections.Generic;");
    if opts.is_json() {
        w.line("using System.Text.Json;");
        w.line("using System.Text.Json.Serialization;");
    } else {
        w.line("using System.Formats.Cbor;");
    }
    w.blank();
    w.line(&format!("namespace {ns};"));
    w.blank();

    // Emit error / helper type
    w.line("/// <summary>Field-level validation exception.</summary>");
    let exc_name = if opts.is_json() {
        "JsonValidationException"
    } else {
        "CborValidationException"
    };
    w.line(&format!("public sealed class {exc_name} : Exception"));
    w.line("{");
    w.indent();
    w.line("public string FieldName { get; }");
    w.line(&format!(
        "public {exc_name}(string fieldName, string message)"
    ));
    w.indent();
    w.line(": base($\"Validation failed for '{fieldName}': {message}\")");
    w.dedent();
    w.line("=> FieldName = fieldName;");
    w.dedent();
    w.line("}");
    w.blank();

    // JSON: emit base64url helpers
    if opts.is_json() {
        w.line("internal static class Base64Url");
        w.line("{");
        w.indent();
        w.line("public static string Encode(byte[] data)");
        w.indent();
        w.line("=> Convert.ToBase64String(data).Replace('+', '-').Replace('/', '_').TrimEnd('=');");
        w.dedent();
        w.blank();
        w.line("public static byte[] Decode(string s)");
        w.line("{");
        w.indent();
        w.line("s = s.Replace('-', '+').Replace('_', '/');");
        w.line("switch (s.Length % 4) { case 2: s += \"==\"; break; case 3: s += \"=\"; break; }");
        w.line("return Convert.FromBase64String(s);");
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
        w.blank();
    }

    if opts.is_json() {
        emit_json_helpers(&mut w);
    } else {
        emit_cbor_helpers(&mut w);
    }

    // Emit each type
    for (_, def) in &module.types {
        emit_type(&mut w, def, module, &ns, opts);
        w.blank();
    }

    w.finish()
}

fn emit_type(
    w: &mut IndentWriter,
    def: &TypeDef,
    module: &IrModule,
    ns: &str,
    opts: &CodegenOptions,
) {
    match def {
        TypeDef::Struct(s) => emit_struct(w, s, opts),
        TypeDef::Enum(e) => emit_enum(w, e, ns, opts),
        TypeDef::Array(a) => match flat_record(a, module) {
            Some(rec) => emit_flat_array(w, a, rec, opts),
            None => emit_array(w, a, opts),
        },
        TypeDef::Alias(a) => emit_alias(w, a, opts),
        TypeDef::Map(m) => emit_map(w, m, opts),
    }
}

// ── Class (struct) ────────────────────────────────────────────────────────────

fn emit_struct(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    emit_doc_comment(w, s.doc.as_deref());
    if s.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&s.name);

    let record = s.encoding == cddlc_ir::StructEncoding::Array;
    if record && opts.is_json() {
        w.line(&format!(
            "[JsonConverter(typeof({type_name}JsonConverter))]"
        ));
    }
    // A map has only the members its schema gives it (dgtlrift/cddlc#102).
    if !record && opts.is_json() && s.rest.is_none() {
        w.line("[JsonUnmappedMemberHandling(JsonUnmappedMemberHandling.Disallow)]");
    }
    // JSON's other entries convert around the serializer (dgtlrift/cddlc#82).
    if opts.is_json() && s.rest.is_some() {
        w.line(&format!(
            "public sealed class {type_name} : IJsonOnSerializing, IJsonOnDeserialized"
        ));
    } else {
        w.line(&format!("public sealed class {type_name}"));
    }
    w.line("{");
    w.indent();

    // Properties
    for f in &s.fields {
        emit_doc_comment(w, f.doc.as_deref());
        if f.deprecated {
            w.line("[Obsolete]");
        }
        if opts.is_json() && !record {
            // JsonPropertyName attribute
            w.line(&format!(
                "[JsonPropertyName({:?})]",
                cddlc_codegen::field_key_json(&f.key)
            ));
            // Skip null for optional fields
            if matches!(f.occurrence, Occurrence::Optional) {
                w.line("[JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]");
            }
            // A required member must be there: the serializer would leave it
            // at its default.
            if matches!(f.occurrence, Occurrence::Required) {
                w.line("[JsonRequired]");
            }
            if matches!(f.occurrence, Occurrence::Optional | Occurrence::Required) {
                if matches!(cs_base(&f.ty), TypeRef::Primitive(Primitive::Bstr)) {
                    w.line("[JsonConverter(typeof(Base64UrlJsonConverter))]");
                } else if cs_typeref(&f.ty, opts) == "object" {
                    w.line(&format!(
                        "[JsonConverter(typeof({type_name}{}Json))]",
                        to_pascal_case(&f.name)
                    ));
                }
            }
        }
        let ty = cs_field_type(&f.ty, &f.occurrence, opts);
        let pname = to_pascal_case(&f.name);
        // A required member that's a reference type starts at its type's
        // default, so `new T()` encodes.
        let init = if matches!(f.occurrence, Occurrence::Optional) {
            String::new()
        } else if matches!(f.occurrence, Occurrence::Required) && !f.constraints.is_empty() {
            format!(
                " = {};",
                cs_valid_constrained_value(&f.constraints, &f.ty, opts)
            )
        } else if cs_is_value_type(&f.ty, opts) && matches!(f.occurrence, Occurrence::Required) {
            String::new()
        } else {
            format!(
                " = {};",
                cs_default_value(&f.ty, &f.occurrence, opts).trim()
            )
        };
        w.line(&format!("public {ty} {pname} {{ get; set; }}{init}"));
    }
    // The map's other entries (dgtlrift/cddlc#82), typed as in CBOR. In JSON
    // the serializer's extension data carries them as JSON elements, which a
    // non-public property converts to and from the values' type.
    if let Some(r) = &s.rest {
        w.line("/// <summary>The map's other entries.</summary>");
        let pname = to_pascal_case(&r.name);
        if opts.is_json() {
            let vt = cs_typeref(&r.val_ty, opts);
            w.line("[JsonIgnore]");
            w.line(&format!(
                "public Dictionary<string, {vt}> {pname} {{ get; set; }} = new();"
            ));
            w.line("[JsonExtensionData]");
            w.line("[JsonInclude]");
            w.line(&format!(
                "internal Dictionary<string, JsonElement>? {pname}Json {{ get; set; }}"
            ));
            w.line("void IJsonOnSerializing.OnSerializing()");
            w.line("{");
            w.indent();
            // A named member's key isn't one of the other entries: it would be
            // written twice.
            let named: Vec<String> = s
                .fields
                .iter()
                .map(|f| format!("{:?}", cddlc_codegen::field_key_json(&f.key)))
                .collect();
            if !named.is_empty() {
                w.line(&format!("foreach (var k in {pname}.Keys)"));
                w.line(&format!(
                    "    if (k is {}) throw new JsonException($\"{type_name}: other entry '{{k}}' is a named member's key\");",
                    named.join(" or ")
                ));
            }
            w.line(&format!(
                "{pname}Json = {pname}.ToDictionary(kv => kv.Key, kv => JsonDocument.Parse({}).RootElement.Clone());",
                cs_json_to_bytes(&r.val_ty, "kv.Value", opts)
            ));
            w.dedent();
            w.line("}");
            w.line("void IJsonOnDeserialized.OnDeserialized()");
            w.line("{");
            w.indent();
            w.line(&format!(
                "{pname} = ({pname}Json ?? new()).ToDictionary(kv => kv.Key, kv => {});",
                cs_json_from_element(&r.val_ty, "kv.Value", opts)
            ));
            w.line(&format!("{pname}Json = null;"));
            w.dedent();
            w.line("}");
        } else {
            w.line(&format!(
                "public Dictionary<{}, {}> {pname} {{ get; set; }} = new();",
                rest_key_cs(r, opts),
                cs_typeref(&r.val_ty, opts)
            ));
        }
    }
    w.blank();
    w.line(&format!("public static {type_name} Default => new();"));
    w.blank();

    if record && !opts.is_json() {
        emit_record_cbor(w, s, &type_name, opts);
        w.dedent();
        w.line("}");
        return;
    }

    // Encode method
    emit_struct_encode(w, s, opts);
    w.blank();

    // Decode static method
    emit_struct_decode(w, s, opts);

    w.dedent();
    w.line("}");
    if record {
        w.blank();
        emit_record_json_converter(w, s, &type_name, opts);
    } else if opts.is_json() {
        for f in &s.fields {
            if matches!(f.occurrence, Occurrence::Optional | Occurrence::Required)
                && cs_typeref(&f.ty, opts) == "object"
            {
                w.blank();
                emit_choice_member_converter(
                    w,
                    &format!("{type_name}{}Json", to_pascal_case(&f.name)),
                    &f.ty,
                    opts,
                );
            }
        }
    }
}

/// A record's Encode / Decode (RFC 8610 §3.4): an array of the members' values,
/// in order. Optional members come last: those present are written up to the
/// first absent one, and one set after an absent one is an error.
/// Whether a map's other entries have plain `tstr` keys (otherwise they hold
/// their key type; dgtlrift/cddlc#136).
fn text_rest(r: &cddlc_ir::MapRest) -> bool {
    matches!(r.key_ty, TypeRef::Primitive(Primitive::Tstr))
}

/// The C# type of a map's other entries' keys.
fn rest_key_cs(r: &cddlc_ir::MapRest, opts: &CodegenOptions) -> String {
    if text_rest(r) {
        "string".into()
    } else {
        cs_typeref(&r.key_ty, opts)
    }
}

/// Writes a map member's key: its text, or an integer key as an integer
/// (dgtlrift/cddlc#176).
fn cs_key_write(key: &cddlc_ir::FieldKey) -> String {
    match key {
        cddlc_ir::FieldKey::Text(t) => format!("writer.WriteTextString({:?});", t.as_str()),
        cddlc_ir::FieldKey::Uint(n) => format!("writer.WriteUInt64({n}UL);"),
        cddlc_ir::FieldKey::Int(n) if *n >= 0 => format!("writer.WriteUInt64({n}UL);"),
        cddlc_ir::FieldKey::Int(n) => format!("writer.WriteInt64({n}L);"),
    }
}

fn emit_record_cbor(w: &mut IndentWriter, s: &StructDef, type_name: &str, opts: &CodegenOptions) {
    if let Some(ri) = cddlc_codegen::record_repeated(s) {
        emit_record_cbor_repeated(w, s, ri, type_name, opts);
        return;
    }
    let optional: Vec<_> = s
        .fields
        .iter()
        .filter(|f| matches!(f.occurrence, Occurrence::Optional))
        .collect();
    let required = s.fields.len() - optional.len();
    w.line("public void Encode(CborWriter writer)");
    w.line("{");
    w.indent();
    for pair in optional.windows(2) {
        w.line(&format!(
            "if (this.{} is not null && this.{} is null) throw new InvalidOperationException({:?});",
            to_pascal_case(&pair[1].name), to_pascal_case(&pair[0].name),
            format!("{type_name}.{}: set, but an optional member before it isn't", pair[1].name)
        ));
    }
    if let Some(tag) = s.tagged {
        w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
    }
    let present: String = optional
        .iter()
        .map(|f| format!(" + (this.{} is not null ? 1 : 0)", to_pascal_case(&f.name)))
        .collect();
    w.line(&format!("writer.WriteStartArray({required}{present});"));
    for f in &s.fields {
        let expr = format!("this.{}", to_pascal_case(&f.name));
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("if ({expr} is not null)"));
            w.line("{");
            w.indent();
            emit_cs_encode_typeref(w, &f.ty, &cs_present_expr(&f.ty, &expr, opts), opts);
            w.dedent();
            w.line("}");
        } else {
            emit_cs_encode_value(w, &f.ty, &expr, &f.occurrence, opts);
        }
    }
    w.line("writer.WriteEndArray();");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!(
        "public static {type_name} Decode(CborReader reader)"
    ));
    w.line("{");
    w.indent();
    if let Some(tag) = s.tagged {
        w.line("var tag = reader.ReadTag();");
        w.line(&format!(
            "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
            tag.0
        ));
    }
    for f in &s.fields {
        let ty = cs_field_type(&f.ty, &f.occurrence, opts);
        let local = cs_local(&f.name);
        let def = cs_default_value(&f.ty, &f.occurrence, opts);
        w.line(&format!("{ty} {local} = {def};"));
    }
    // One optional member before required ones: present when the array holds
    // every member, so count the items first, on a copy of the array's
    // encoding (dgtlrift/cddlc#134).
    let before = cddlc_codegen::record_optionals_before(s);
    if before.is_some() {
        w.line("var encoded = reader.ReadEncodedValue();");
        w.line("var probe = new CborReader(encoded, reader.ConformanceMode);");
        w.line("probe.ReadStartArray();");
        w.line("int count = 0;");
        w.line(
            "while (probe.PeekState() != CborReaderState.EndArray) { probe.SkipValue(); count++; }",
        );
        w.line("reader = new CborReader(encoded, reader.ConformanceMode);");
    }
    w.line("var length = reader.ReadStartArray();");
    w.line("int read = 0;");
    w.line("bool More() => length is int n ? read < n : reader.PeekState() != CborReaderState.EndArray;");
    for (i, f) in s.fields.iter().enumerate() {
        let local = cs_local(&f.name);
        if let (Occurrence::Optional, Some(b)) = (&f.occurrence, &before) {
            w.line(&format!("if (count > {})", required + b[i]));
        } else if matches!(f.occurrence, Occurrence::Optional) {
            w.line("if (More())");
            w.line("{");
            w.indent();
            emit_cs_decode_field(
                w,
                &f.ty,
                &local,
                &f.constraints,
                &f.name,
                &f.occurrence,
                opts,
            );
            w.line("read++;");
            w.dedent();
            w.line("}");
        } else {
            w.line(&format!(
                "if (!More()) throw new CborContentException({:?});",
                format!("{type_name}: missing item '{}'", f.name)
            ));
            emit_cs_decode_field(
                w,
                &f.ty,
                &local,
                &f.constraints,
                &f.name,
                &f.occurrence,
                opts,
            );
            w.line("read++;");
        }
    }
    w.line(&format!(
        "if (More()) throw new CborContentException({:?});",
        format!("{type_name}: too many items")
    ));
    w.line("reader.ReadEndArray();");
    w.line(&format!("return new {type_name}"));
    w.line("{");
    w.indent();
    for f in &s.fields {
        w.line(&format!(
            "{} = {},",
            to_pascal_case(&f.name),
            cs_local(&f.name)
        ));
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
}

/// Throws (`exc`) unless a repeated member's item count, `n`, is within its
/// bounds (dgtlrift/cddlc#134).
fn emit_cs_repeat_check(
    w: &mut IndentWriter,
    type_name: &str,
    f: &cddlc_ir::FieldDef,
    n: &str,
    exc: &str,
) {
    let (min, max) = cddlc_codegen::repeat_bounds(&f.occurrence);
    if min > 0 {
        w.line(&format!(
            "if ({n} < {min}) throw new {exc}({:?});",
            format!("{type_name}.{}: needs at least {min} items", f.name)
        ));
    }
    if let Some(max) = max {
        w.line(&format!(
            "if ({n} > {max}) throw new {exc}({:?});",
            format!("{type_name}.{}: has more than {max} items", f.name)
        ));
    }
}

/// A record with a repeated member (`[primary, * extension, payload]`), in
/// CBOR: the member's items go inline, and the array's length says how many
/// there are (dgtlrift/cddlc#134).
fn emit_record_cbor_repeated(
    w: &mut IndentWriter,
    s: &StructDef,
    ri: usize,
    type_name: &str,
    opts: &CodegenOptions,
) {
    let rf = &s.fields[ri];
    let rp = to_pascal_case(&rf.name);
    let fixed = s.fields.len() - 1;
    w.line("public void Encode(CborWriter writer)");
    w.line("{");
    w.indent();
    emit_cs_repeat_check(
        w,
        type_name,
        rf,
        &format!("this.{rp}.Count"),
        "InvalidOperationException",
    );
    if let Some(tag) = s.tagged {
        w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
    }
    w.line(&format!(
        "writer.WriteStartArray({fixed} + this.{rp}.Count);"
    ));
    for f in &s.fields {
        let expr = format!("this.{}", to_pascal_case(&f.name));
        if f.occurrence.is_repeated() {
            w.line(&format!("foreach (var item in {expr})"));
            w.line("{");
            w.indent();
            emit_cs_encode_typeref(w, &f.ty, "item", opts);
            w.dedent();
            w.line("}");
        } else {
            emit_cs_encode_value(w, &f.ty, &expr, &f.occurrence, opts);
        }
    }
    w.line("writer.WriteEndArray();");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!(
        "public static {type_name} Decode(CborReader reader)"
    ));
    w.line("{");
    w.indent();
    if let Some(tag) = s.tagged {
        w.line("var tag = reader.ReadTag();");
        w.line(&format!(
            "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
            tag.0
        ));
    }
    for f in &s.fields {
        let ty = cs_field_type(&f.ty, &f.occurrence, opts);
        let local = cs_local(&f.name);
        let def = cs_default_value(&f.ty, &f.occurrence, opts);
        w.line(&format!("{ty} {local} = {def};"));
    }
    // Count the items first, on a copy of the array's encoding.
    w.line("var encoded = reader.ReadEncodedValue();");
    w.line("var probe = new CborReader(encoded, reader.ConformanceMode);");
    w.line("probe.ReadStartArray();");
    w.line("int count = 0;");
    w.line("while (probe.PeekState() != CborReaderState.EndArray) { probe.SkipValue(); count++; }");
    w.line("reader = new CborReader(encoded, reader.ConformanceMode);");
    w.line(&format!(
        "if (count < {fixed}) throw new CborContentException({:?});",
        format!("{type_name}: too few items")
    ));
    w.line(&format!("int reps = count - {fixed};"));
    emit_cs_repeat_check(w, type_name, rf, "reps", "CborContentException");
    w.line("reader.ReadStartArray();");
    for f in &s.fields {
        let local = cs_local(&f.name);
        if f.occurrence.is_repeated() {
            w.line(&format!(
                "for (int k = 0; k < reps; k++) {local}.Add({});",
                cs_decode_expr(&f.ty, opts)
            ));
        } else {
            emit_cs_decode_field(
                w,
                &f.ty,
                &local,
                &f.constraints,
                &f.name,
                &f.occurrence,
                opts,
            );
        }
    }
    w.line("reader.ReadEndArray();");
    w.line(&format!("return new {type_name}"));
    w.line("{");
    w.indent();
    for f in &s.fields {
        w.line(&format!(
            "{} = {},",
            to_pascal_case(&f.name),
            cs_local(&f.name)
        ));
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
}

/// A record with a repeated member, in JSON: its items go inline, and the
/// array's length says how many there are (dgtlrift/cddlc#134).
fn emit_record_json_repeated(
    w: &mut IndentWriter,
    s: &StructDef,
    ri: usize,
    type_name: &str,
    opts: &CodegenOptions,
) {
    let rf = &s.fields[ri];
    let fixed = s.fields.len() - 1;
    w.line(&format!(
        "public sealed class {type_name}JsonConverter : JsonConverter<{type_name}>"
    ));
    w.line("{");
    w.indent();
    w.line(&format!(
        "public override {type_name} Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)"
    ));
    w.line("{");
    w.indent();
    w.line("using var doc = JsonDocument.ParseValue(ref reader);");
    w.line("var a = doc.RootElement;");
    w.line(&format!(
        "if (a.ValueKind != JsonValueKind.Array || a.GetArrayLength() < {fixed})"
    ));
    w.line(&format!(
        "    throw new JsonException({:?});",
        format!("{type_name}: expected an array of at least {fixed} items")
    ));
    w.line(&format!("int reps = a.GetArrayLength() - {fixed};"));
    emit_cs_repeat_check(w, type_name, rf, "reps", "JsonException");
    let local = cs_local(&rf.name);
    w.line(&format!(
        "var {local} = {};",
        cs_default_value(&rf.ty, &rf.occurrence, opts).trim()
    ));
    w.line(&format!(
        "for (int k = 0; k < reps; k++) {local}.Add({});",
        cs_json_from_element(&rf.ty, &format!("a[{ri} + k]"), opts)
    ));
    w.line(&format!("return new {type_name}"));
    w.line("{");
    w.indent();
    for (i, f) in s.fields.iter().enumerate() {
        let pname = to_pascal_case(&f.name);
        if i == ri {
            w.line(&format!("{pname} = {local},"));
        } else {
            let el = if i < ri {
                format!("a[{i}]")
            } else {
                format!("a[{} + reps]", i - 1)
            };
            w.line(&format!(
                "{pname} = {},",
                cs_json_from_element(&f.ty, &el, opts)
            ));
        }
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!(
        "public override void Write(Utf8JsonWriter writer, {type_name} value, JsonSerializerOptions options)"
    ));
    w.line("{");
    w.indent();
    emit_cs_repeat_check(
        w,
        type_name,
        rf,
        &format!("value.{}.Count", to_pascal_case(&rf.name)),
        "JsonException",
    );
    w.line("writer.WriteStartArray();");
    for f in &s.fields {
        let expr = format!("value.{}", to_pascal_case(&f.name));
        if f.occurrence.is_repeated() {
            w.line(&format!(
                "foreach (var item in {expr}) {}",
                cs_json_write_stmt(&f.ty, "item", opts)
            ));
        } else {
            w.line(&cs_json_write_stmt(&f.ty, &expr, opts));
        }
    }
    w.line("writer.WriteEndArray();");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

/// System.Text.Json writes a class as an object, so a record gets a converter:
/// a JSON array of its members' values, in order, optional ones at the end
/// left off when absent.
fn emit_record_json_converter(
    w: &mut IndentWriter,
    s: &StructDef,
    type_name: &str,
    opts: &CodegenOptions,
) {
    if let Some(ri) = cddlc_codegen::record_repeated(s) {
        emit_record_json_repeated(w, s, ri, type_name, opts);
        return;
    }
    let optional: Vec<_> = s
        .fields
        .iter()
        .filter(|f| matches!(f.occurrence, Occurrence::Optional))
        .collect();
    let required = s.fields.len() - optional.len();
    let total = s.fields.len();
    w.line(&format!(
        "public sealed class {type_name}JsonConverter : JsonConverter<{type_name}>"
    ));
    w.line("{");
    w.indent();
    w.line(&format!(
        "public override {type_name} Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)"
    ));
    w.line("{");
    w.indent();
    w.line("using var doc = JsonDocument.ParseValue(ref reader);");
    w.line("var a = doc.RootElement;");
    w.line(&format!(
        "if (a.ValueKind != JsonValueKind.Array || a.GetArrayLength() < {required} || a.GetArrayLength() > {total})"
    ));
    w.line(&format!(
        "    throw new JsonException({:?});",
        format!("{type_name}: expected an array of {required} to {total} items")
    ));
    w.line("int n = a.GetArrayLength();");
    w.line(&format!("return new {type_name}"));
    w.line("{");
    w.indent();
    // One optional member before required ones: present when the array holds
    // every member; those after it move down one when it's absent
    // (dgtlrift/cddlc#134).
    let before = cddlc_codegen::record_optionals_before(s);
    for (i, f) in s.fields.iter().enumerate() {
        let ty = cs_field_type(&f.ty, &f.occurrence, opts);
        let pname = to_pascal_case(&f.name);
        let el = match &before {
            Some(b) if b[i] > 0 => {
                let x = required + b[i];
                format!("a[n >= {x} ? {i} : {i} - ({x} - n)]")
            }
            _ => format!("a[{i}]"),
        };
        let present = match &before {
            Some(b) => format!("n > {}", required + b[i]),
            None => format!("n > {i}"),
        };
        match f.occurrence {
            Occurrence::Optional => w.line(&format!(
                "{pname} = {present} ? ({ty}){} : null,",
                cs_json_from_element(&f.ty, &el, opts)
            )),
            Occurrence::Required => w.line(&format!(
                "{pname} = {},",
                cs_json_from_element(&f.ty, &el, opts)
            )),
            _ => w.line(&format!("{pname} = {el}.Deserialize<{ty}>(options)!,")),
        }
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!(
        "public override void Write(Utf8JsonWriter writer, {type_name} value, JsonSerializerOptions options)"
    ));
    w.line("{");
    w.indent();
    for pair in optional.windows(2) {
        w.line(&format!(
            "if (value.{} is not null && value.{} is null) throw new JsonException({:?});",
            to_pascal_case(&pair[1].name),
            to_pascal_case(&pair[0].name),
            format!(
                "{type_name}.{}: set, but an optional member before it isn't",
                pair[1].name
            )
        ));
    }
    w.line("writer.WriteStartArray();");
    for f in &s.fields {
        let pname = to_pascal_case(&f.name);
        let expr = format!("value.{pname}");
        match f.occurrence {
            Occurrence::Optional => w.line(&format!(
                "if ({expr} is not null) {}",
                cs_json_write_stmt(&f.ty, &cs_present_expr(&f.ty, &expr, opts), opts)
            )),
            Occurrence::Required => w.line(&cs_json_write_stmt(&f.ty, &expr, opts)),
            _ => w.line(&format!(
                "JsonSerializer.Serialize(writer, {expr}, options);"
            )),
        }
    }
    w.line("writer.WriteEndArray();");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
}

fn emit_struct_encode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    if opts.is_json() {
        w.line("public byte[] ToJson()");
        w.indent();
        w.line("=> JsonSerializer.SerializeToUtf8Bytes(this);");
        w.dedent();
        return;
    }

    let required = s
        .fields
        .iter()
        .filter(|f| !matches!(f.occurrence, Occurrence::Optional))
        .count();
    let present: String = s
        .fields
        .iter()
        .filter(|f| matches!(f.occurrence, Occurrence::Optional))
        .map(|f| format!(" + (this.{} is not null ? 1 : 0)", to_pascal_case(&f.name)))
        .collect();

    w.line("public void Encode(CborWriter writer)");
    w.line("{");
    w.indent();

    // Tag
    if let Some(tag) = s.tagged {
        w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
    }

    // dCBOR: sort fields by key
    let fields: Vec<_> = if opts.dcbor {
        let mut fs: Vec<_> = s.fields.iter().collect();
        fs.sort_by(|a, b| cddlc_codegen::dcbor_key_order(a, b));
        fs
    } else {
        s.fields.iter().collect()
    };

    let rest_count = s
        .rest
        .as_ref()
        .map(|r| format!(" + this.{}.Count", to_pascal_case(&r.name)))
        .unwrap_or_default();
    w.line(&format!(
        "writer.WriteStartMap({required}{present}{rest_count});"
    ));
    for f in &fields {
        let expr = format!("this.{}", to_pascal_case(&f.name));
        // An absent optional member is left out of the map.
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("if ({expr} is not null)"));
            w.line("{");
            w.indent();
            w.line(&cs_key_write(&f.key));
            emit_cs_encode_typeref(w, &f.ty, &cs_present_expr(&f.ty, &expr, opts), opts);
            w.dedent();
            w.line("}");
        } else {
            w.line(&cs_key_write(&f.key));
            emit_cs_encode_value(w, &f.ty, &expr, &f.occurrence, opts);
        }
    }
    if let Some(r) = &s.rest {
        let named: Vec<String> = s
            .fields
            .iter()
            .filter_map(|f| match &f.key {
                cddlc_ir::FieldKey::Text(t) => Some(format!("{:?}", t.as_str())),
                _ => None,
            })
            .collect();
        w.line(&format!(
            "foreach (var kv in this.{})",
            to_pascal_case(&r.name)
        ));
        w.line("{");
        w.indent();
        if !named.is_empty() && text_rest(r) {
            w.line(&format!(
                "if (Array.IndexOf(new[] {{ {} }}, kv.Key) >= 0) throw new CborContentException($\"Other entry {{kv.Key}} is a named member's key\");",
                named.join(", ")
            ));
        }
        if text_rest(r) {
            w.line("writer.WriteTextString(kv.Key);");
        } else {
            emit_cs_encode_value(w, &r.key_ty, "kv.Key", &Occurrence::Required, opts);
        }
        emit_cs_encode_value(w, &r.val_ty, "kv.Value", &Occurrence::Required, opts);
        w.dedent();
        w.line("}");
    }
    w.line("writer.WriteEndMap();");

    w.dedent();
    w.line("}");
}

fn emit_struct_decode(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&s.name);

    if opts.is_json() {
        // A member pinned to a literal (`type: "javascript"`) must hold it, so
        // a choice of maps tells its alternatives apart (dgtlrift/cddlc#157).
        let pinned: Vec<(String, String)> = s
            .fields
            .iter()
            .filter(|f| matches!(f.occurrence, Occurrence::Required))
            .filter_map(|f| {
                f.constraints.iter().find_map(|c| match c {
                    Constraint::Eq(cddlc_ir::LiteralValue::Text(t)) => {
                        Some((to_pascal_case(&f.name), format!("{:?}", t.as_str())))
                    }
                    Constraint::Eq(cddlc_ir::LiteralValue::Uint(n)) => {
                        Some((to_pascal_case(&f.name), format!("{n}UL")))
                    }
                    Constraint::Eq(cddlc_ir::LiteralValue::Int(n)) => {
                        Some((to_pascal_case(&f.name), format!("{n}L")))
                    }
                    _ => None,
                })
            })
            .collect();
        // Members with constraints to check, as CBOR checks them on decode.
        let checked: Vec<&cddlc_ir::FieldDef> = s
            .fields
            .iter()
            .filter(|f| matches!(f.occurrence, Occurrence::Required | Occurrence::Optional))
            .filter(|f| {
                f.constraints
                    .iter()
                    .any(|c| cs_constraint_check(c, "x", &f.ty).is_some())
            })
            .collect();
        w.line(&format!("public static {type_name} FromJson(byte[] data)"));
        if pinned.is_empty() && checked.is_empty() {
            w.indent();
            w.line(&format!(
                "=> JsonSerializer.Deserialize<{type_name}>(data)!;"
            ));
            w.dedent();
            return;
        }
        w.line("{");
        w.indent();
        w.line(&format!(
            "var v = JsonSerializer.Deserialize<{type_name}>(data)!;"
        ));
        for (prop, lit) in pinned {
            w.line(&format!(
                "if (!Equals(v.{prop}, {lit})) throw new JsonException({:?});",
                format!("{type_name}.{prop}: must be {lit}")
            ));
        }
        for f in checked {
            let prop = format!("v.{}", to_pascal_case(&f.name));
            if matches!(f.occurrence, Occurrence::Optional) {
                w.line(&format!("if ({prop} is not null)"));
                w.line("{");
                w.indent();
                emit_cs_validation(
                    w,
                    &f.constraints,
                    &cs_present_expr(&f.ty, &prop, opts),
                    &f.name,
                    &f.ty,
                    opts,
                );
                w.dedent();
                w.line("}");
            } else {
                emit_cs_validation(w, &f.constraints, &prop, &f.name, &f.ty, opts);
            }
        }
        w.line("return v;");
        w.dedent();
        w.line("}");
        return;
    }

    w.line(&format!(
        "public static {type_name} Decode(CborReader reader)"
    ));
    w.line("{");
    w.indent();

    // Tag verification
    if let Some(tag) = s.tagged {
        w.line(&format!("var tag = reader.ReadTag();"));
        w.line(&format!(
            "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
            tag.0
        ));
    }

    // Declare locals
    for f in &s.fields {
        let ty = cs_field_type(&f.ty, &f.occurrence, opts);
        let local = cs_local(&f.name);
        let def = cs_default_value(&f.ty, &f.occurrence, opts);
        w.line(&format!("{ty} {local} = {def};"));
    }
    // Whether each required member was read: a map without one is invalid,
    // not one with a default value.
    let required: Vec<&cddlc_ir::FieldDef> = s
        .fields
        .iter()
        .filter(|f| matches!(f.occurrence, Occurrence::Required))
        .collect();
    for (i, _) in required.iter().enumerate() {
        w.line(&format!("bool have{i} = false;"));
    }
    if let Some(r) = &s.rest {
        w.line(&format!(
            "var rest = new Dictionary<{}, {}>();",
            rest_key_cs(r, opts),
            cs_typeref(&r.val_ty, opts)
        ));
    }
    w.blank();

    w.line("var length = reader.ReadStartMap();");
    w.line("var count  = length ?? int.MaxValue;");
    w.line("for (int i = 0; i < count; i++)");
    w.line("{");
    w.indent();
    w.line("if (length == null && reader.PeekState() == CborReaderState.EndMap) break;");
    // A map with integer keys (`1 => tstr`) reads each key as text or an
    // integer (dgtlrift/cddlc#176).
    // Other entries with typed keys: the key's encoding is read once, matched
    // against the members, and decoded as the key type if none has it
    // (dgtlrift/cddlc#136).
    let typed_rest = s.rest.as_ref().filter(|r| !text_rest(r));
    let int_keys = cddlc_codegen::has_int_keys(s) || typed_rest.is_some();
    if typed_rest.is_some() {
        w.line("var keyBytes = reader.ReadEncodedValue();");
        w.line("var keyReader = new CborReader(keyBytes, reader.ConformanceMode);");
        w.line("object key = keyReader.PeekState() == CborReaderState.TextString ? keyReader.ReadTextString() : keyReader.ReadInt64();");
    } else if int_keys {
        w.line("object key = reader.PeekState() == CborReaderState.TextString ? reader.ReadTextString() : reader.ReadInt64();");
    } else {
        w.line("var key = reader.ReadTextString();");
    }
    w.line("switch (key)");
    w.line("{");
    w.indent();

    for f in &s.fields {
        let local = cs_local(&f.name);
        match &f.key {
            cddlc_ir::FieldKey::Text(t) => w.line(&format!("case {:?}:", t.as_str())),
            cddlc_ir::FieldKey::Int(n) => w.line(&format!("case {n}L:")),
            cddlc_ir::FieldKey::Uint(n) => w.line(&format!("case {}L:", (*n).min(i64::MAX as u64))),
        }
        w.indent();
        emit_cs_decode_field(
            w,
            &f.ty,
            &local,
            &f.constraints,
            &f.name,
            &f.occurrence,
            opts,
        );
        if let Some(i) = required.iter().position(|r| std::ptr::eq(*r, f)) {
            w.line(&format!("have{i} = true;"));
        }
        w.line("break;");
        w.dedent();
    }

    // Other keys: the map's other entries, or unknown fields to skip.
    w.line("default:");
    w.indent();
    match &s.rest {
        Some(r) if typed_rest.is_some() => {
            w.line(&format!(
                "static {} DecodeKey(CborReader reader) => {};",
                rest_key_cs(r, opts), cs_decode_expr(&r.key_ty, opts)
            ));
            w.line(&format!(
                "rest[DecodeKey(new CborReader(keyBytes, reader.ConformanceMode))] = {};",
                cs_decode_expr(&r.val_ty, opts)
            ));
        }
        Some(r) if int_keys => w.line(&format!(
            "if (key is string textKey) rest[textKey] = {}; else throw new CborContentException($\"{}: unknown member {{key}}\");",
            cs_decode_expr(&r.val_ty, opts), to_pascal_case(&s.name)
        )),
        Some(r) => w.line(&format!("rest[key] = {};", cs_decode_expr(&r.val_ty, opts))),
        // A map has only the members its schema gives it (dgtlrift/cddlc#102).
        None => w.line(&format!("throw new CborContentException($\"{}: unknown member {{key}}\");", to_pascal_case(&s.name))),
    }
    w.line("break;");
    w.dedent();

    w.dedent();
    w.line("}"); // switch
    w.dedent();
    w.line("}"); // for
    w.line("reader.ReadEndMap();");
    for (i, f) in required.iter().enumerate() {
        w.line(&format!(
            "if (!have{i}) throw new CborContentException(\"{type_name}: missing member {}\");",
            f.name
        ));
    }
    w.blank();

    w.line(&format!("return new {type_name}"));
    w.line("{");
    w.indent();
    for f in &s.fields {
        let pname = to_pascal_case(&f.name);
        let local = cs_local(&f.name);
        w.line(&format!("{pname} = {local},"));
    }
    if let Some(r) = &s.rest {
        w.line(&format!("{} = rest,", to_pascal_case(&r.name)));
    }
    w.dedent();
    w.line("};");

    w.dedent();
    w.line("}");
}

// ── Enum (sealed class hierarchy) ─────────────────────────────────────────────

/// A variant that is one literal value (`"ok"`, `1`, `true`, `nil`): its
/// literal as a C# expression, or `None` when the variant holds a value.
fn enum_literal(v: &cddlc_ir::EnumVariant) -> Option<String> {
    if matches!(v.ty, TypeRef::Primitive(Primitive::Null)) {
        return Some("null".into());
    }
    let TypeRef::Primitive(p) = &v.ty else {
        return None;
    };
    v.constraints.iter().find_map(|c| match c {
        Constraint::Eq(LiteralValue::Bytes(_)) => None,
        Constraint::Eq(LiteralValue::Uint(n)) if matches!(p, Primitive::Int) => {
            Some(format!("{n}L"))
        }
        Constraint::Eq(l) => Some(cs_literal(l)),
        _ => None,
    })
}

fn emit_enum(w: &mut IndentWriter, e: &EnumDef, ns: &str, opts: &CodegenOptions) {
    if opts.is_json() {
        emit_enum_json(w, e, ns, opts);
        return;
    }
    emit_doc_comment(w, e.doc.as_deref());
    if e.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&e.name);

    w.line(&format!("public abstract class {type_name}"));
    w.line("{");
    w.indent();

    // Sealed subclasses: a literal variant is a singleton that writes its
    // literal; any other holds a value of its type.
    for v in &e.variants {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        w.line(&format!("public sealed class {vname} : {type_name}"));
        w.line("{");
        w.indent();
        match enum_literal(v) {
            Some(lit) => {
                w.line(&format!("public static readonly {vname} Instance = new();"));
                let TypeRef::Primitive(p) = &v.ty else {
                    unreachable!()
                };
                w.line(&format!(
                    "public override void Encode(CborWriter writer) => {};",
                    cs_encode_call(p, &lit, opts)
                ));
            }
            None => {
                let ct = qualified(cs_typeref(&v.ty, opts), &v.ty, ns);
                w.line(&format!("public {ct} Value {{ get; }}"));
                w.line(&format!("public {vname}({ct} value) => Value = value;"));
                w.line("public override void Encode(CborWriter writer)");
                w.line("{");
                w.indent();
                emit_cs_encode_typeref(w, &v.ty, "Value", opts);
                w.dedent();
                w.line("}");
            }
        }
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.line(&format!(
        "public static {type_name} Default => {};",
        enum_variant_default(e, ns, opts)
    ));
    w.blank();

    w.line("public abstract void Encode(CborWriter writer);");
    w.blank();

    // Decode: the first variant, in the CDDL's order, that the item matches.
    w.line(&format!(
        "public static {type_name} Decode(CborReader reader)"
    ));
    w.line("{");
    w.indent();
    if let Some(tag) = e.tagged {
        w.line("var tag = reader.ReadTag();");
        w.line(&format!(
            "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
            tag.0
        ));
    }
    w.line(&format!("return ({type_name})Cddl_.Choice(reader,"));
    w.indent();
    let n = e.variants.len();
    for (i, v) in e.variants.iter().enumerate() {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        let sep = if i + 1 == n { ");" } else { "," };
        match enum_literal(v) {
            Some(_) if matches!(v.ty, TypeRef::Primitive(Primitive::Null)) =>
                w.line(&format!("r => Cddl_.ReadNull(r) ?? {vname}.Instance{sep}")),
            Some(lit) => w.line(&format!(
                "r => {} == {lit} ? {vname}.Instance : throw new CborContentException(\"not {vname}\"){sep}",
                cs_decode_expr_from(&v.ty, "r", opts)
            )),
            None => w.line(&format!("r => new {vname}({}){sep}", qualified(cs_decode_expr_from(&v.ty, "r", opts), &v.ty, ns))),
        }
    }
    if n == 0 {
        w.line(");");
    }
    w.dedent();
    w.dedent();
    w.line("}");

    w.dedent();
    w.line("}");
}

/// `expr`, a type or an expression starting with one, with a named type
/// qualified by its namespace: inside an enum, a variant class shares the
/// type's name (`Shape.Circle` holds a `Circle`) and hides it (dgtlrift/cddlc#102).
fn qualified(expr: String, ty: &TypeRef, ns: &str) -> String {
    let TypeRef::Named(n) = cs_base(ty) else {
        return expr;
    };
    let t = to_pascal_case(n);
    if expr == t || expr == format!("{t}?") {
        return format!("global::{ns}.{expr}");
    }
    if let Some(rest) = expr.strip_prefix(&format!("{t}.")) {
        return format!("global::{ns}.{t}.{rest}");
    }
    if let Some(rest) = expr.strip_prefix(&format!("new {t}(")) {
        return format!("new global::{ns}.{t}({rest}");
    }
    expr
}

// The (enum, variant) pairs whose variant holds the enum again, set for each
// `generate`.
thread_local! {
    static RECURSIVE_VARIANTS: std::cell::RefCell<std::collections::HashSet<(String, String)>> = Default::default();
}

/// An enum's `Default`: its first variant that doesn't hold the enum again,
/// whose default would build itself without end (`link = next / null`;
/// dgtlrift/cddlc#224); its first if they all do.
fn enum_variant_default(e: &EnumDef, ns: &str, opts: &CodegenOptions) -> String {
    let type_name = to_pascal_case(&e.name);
    let recursive = |v: &&EnumVariant| {
        RECURSIVE_VARIANTS.with(|r| {
            r.borrow()
                .contains(&(e.name.to_string(), v.name.to_string()))
        })
    };
    let Some(v) = e
        .variants
        .iter()
        .find(|v| !recursive(v))
        .or(e.variants.first())
    else {
        return "null!".into();
    };
    let vname = cddlc_codegen::csharp_variant_name(&v.name);
    match enum_literal(v) {
        Some(_) => format!("{type_name}.{vname}.Instance"),
        None => format!(
            "new {type_name}.{vname}({})",
            qualified(cs_type_default(&v.ty, opts), &v.ty, ns)
        ),
    }
}

fn emit_enum_json(w: &mut IndentWriter, e: &EnumDef, ns: &str, opts: &CodegenOptions) {
    emit_doc_comment(w, e.doc.as_deref());
    if e.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&e.name);

    w.line(&format!(
        "[JsonConverter(typeof({type_name}JsonConverter))]"
    ));
    w.line(&format!("public abstract class {type_name}"));
    w.line("{");
    w.indent();

    for v in &e.variants {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        w.line(&format!("public sealed class {vname} : {type_name}"));
        w.line("{");
        w.indent();
        match enum_literal(v) {
            Some(lit) => {
                w.line(&format!("public static readonly {vname} Instance = new();"));
                w.line(&format!("public override byte[] ToJson() => JsonSerializer.SerializeToUtf8Bytes<{}>({lit});",
                    cs_typeref(&v.ty, opts)));
            }
            None => {
                let ct = qualified(cs_typeref(&v.ty, opts), &v.ty, ns);
                w.line(&format!("public {ct} Value {{ get; }}"));
                w.line(&format!("public {vname}({ct} value) => Value = value;"));
                w.line(&format!(
                    "public override byte[] ToJson() => {};",
                    cs_json_to_bytes(&v.ty, "Value", opts)
                ));
            }
        }
        w.dedent();
        w.line("}");
        w.blank();
    }

    // Factory methods
    for v in &e.variants {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        match enum_literal(v) {
            Some(_) => w.line(&format!(
                "public static {vname} Create{vname}() => {vname}.Instance;"
            )),
            None => w.line(&format!(
                "public static {vname} Create{vname}({} value) => new(value);",
                qualified(cs_typeref(&v.ty, opts), &v.ty, ns)
            )),
        }
    }
    w.blank();

    w.line(&format!(
        "public static {type_name} Default => {};",
        enum_variant_default(e, ns, opts)
    ));
    w.blank();

    w.line("public abstract byte[] ToJson();");
    w.blank();

    // FromJson: the first variant, in the CDDL's order, that the value matches.
    w.line(&format!("public static {type_name} FromJson(byte[] data)"));
    w.line("{");
    w.indent();
    w.line("using var doc = JsonDocument.Parse(data);");
    w.line(&format!(
        "return ({type_name})Cddl_.Choice(doc.RootElement,"
    ));
    w.indent();
    let n = e.variants.len();
    for (i, v) in e.variants.iter().enumerate() {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        let sep = if i + 1 == n { ");" } else { "," };
        match enum_literal(v) {
            Some(_) if matches!(v.ty, TypeRef::Primitive(Primitive::Null)) => w.line(&format!(
                "j => j.ValueKind == JsonValueKind.Null ? {vname}.Instance : throw new JsonException(\"not {vname}\"){sep}"
            )),
            Some(lit) => w.line(&format!(
                "j => {} == {lit} ? {vname}.Instance : throw new JsonException(\"not {vname}\"){sep}",
                cs_json_from_element(&v.ty, "j", opts)
            )),
            // A named variant isn't JSON null, which System.Text.Json reads as
            // a null object rather than failing (dgtlrift/cddlc#224).
            None if matches!(v.ty, TypeRef::Named(_)) => w.line(&format!(
                "j => j.ValueKind == JsonValueKind.Null ? throw new JsonException(\"null\") : new {vname}({}){sep}",
                qualified(cs_json_from_element(&v.ty, "j", opts), &v.ty, ns)
            )),
            None => w.line(&format!("j => new {vname}({}){sep}", qualified(cs_json_from_element(&v.ty, "j", opts), &v.ty, ns))),
        }
    }
    if n == 0 {
        w.line(");");
    }
    w.dedent();
    w.dedent();
    w.line("}");

    w.dedent();
    w.line("}");
    w.blank();
    emit_delegating_json_converter(w, &type_name);
}

/// JSON bytes for `expr`, a value of type `ty`.
fn cs_json_to_bytes(ty: &TypeRef, expr: &str, opts: &CodegenOptions) -> String {
    match cs_base(ty) {
        TypeRef::Named(_) => format!("{expr}.ToJson()"),
        TypeRef::Primitive(Primitive::Bstr) => {
            format!("JsonSerializer.SerializeToUtf8Bytes(Base64Url.Encode({expr}))")
        }
        TypeRef::Primitive(Primitive::Any) => {
            format!("JsonSerializer.SerializeToUtf8Bytes({expr})")
        }
        _ => format!(
            "JsonSerializer.SerializeToUtf8Bytes<{}>({expr})",
            cs_typeref(ty, opts)
        ),
    }
}

/// An expression reading a value of type `ty` from the JsonElement `el`;
/// throws when the element isn't one.
fn cs_json_from_element(ty: &TypeRef, el: &str, opts: &CodegenOptions) -> String {
    match cs_base(ty) {
        TypeRef::Named(n) => format!(
            "{}.FromJson(System.Text.Encoding.UTF8.GetBytes({el}.GetRawText()))", to_pascal_case(n)
        ),
        TypeRef::Primitive(Primitive::Bstr) => format!("Base64Url.Decode({el}.GetString()!)"),
        // Its own copy, which outlives the document it was read from.
        TypeRef::Primitive(Primitive::Any) => format!("{el}.Clone()"),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) =>
            format!("({el}.ValueKind == JsonValueKind.Null ? null : throw new JsonException(\"expected null\"))"),
        TypeRef::Choice(alts) => {
            let e2 = format!("{}1", el.replace(|c: char| !c.is_ascii_alphanumeric(), "_"));
            let tries: Vec<String> = alts.iter()
                .map(|a| format!("{e2} => (object?){}", cs_json_from_element(a, &e2, opts)))
                .collect();
            let call = format!("Cddl_.Choice({el}, {})", tries.join(", "));
            match cs_typeref(ty, opts).as_str() {
                "object" => call,
                t => format!("({t}){call}"),
            }
        }
        _ => format!("{el}.Deserialize<{}>()!", cs_typeref(ty, opts)),
    }
}

/// The JSON counterpart of `emit_cbor_helpers`: trying a choice's alternatives.
fn emit_json_helpers(w: &mut IndentWriter) {
    for l in [
        "internal static class Cddl_",
        "{",
        "    public static object Choice(JsonElement element, params Func<JsonElement, object?>[] alternatives)",
        "    {",
        "        foreach (var alternative in alternatives)",
        "        {",
        "            try { return alternative(element)!; }",
        "            catch (Exception e) when (e is JsonException or InvalidOperationException or FormatException",
        "                                      or OverflowException or JsonValidationException) { }",
        "        }",
        "        throw new JsonException(\"The value is none of the choice's alternatives\");",
        "    }",
        "",
        "    /// <summary>A key's text, from its JSON: a string's own text, else the raw JSON (`1`).</summary>",
        "    public static string KeyText(byte[] json)",
        "    {",
        "        using var doc = JsonDocument.Parse(json);",
        "        return doc.RootElement.ValueKind == JsonValueKind.String ? doc.RootElement.GetString()! : doc.RootElement.GetRawText();",
        "    }",
        "    /// <summary>A key's JSON, from its text: an integer's digits stay a number, anything else is a string.</summary>",
        "    public static byte[] KeyJson(string name) =>",
        "        long.TryParse(name, System.Globalization.NumberStyles.AllowLeadingSign, System.Globalization.CultureInfo.InvariantCulture, out _)",
        "            ? System.Text.Encoding.UTF8.GetBytes(name)",
        "            : JsonSerializer.SerializeToUtf8Bytes(name);",
        "    public static byte[] Json(Action<Utf8JsonWriter> write)",
        "    {",
        "        using var stream = new System.IO.MemoryStream();",
        "        using (var writer = new Utf8JsonWriter(stream)) write(writer);",
        "        return stream.ToArray();",
        "    }",
        "",
        "    /// <summary>Writes a value of one of a choice's alternatives.</summary>",
        "    public static void WriteChoice(Utf8JsonWriter writer, object? value, JsonSerializerOptions options)",
        "    {",
        "        if (value is byte[] b) writer.WriteStringValue(Base64Url.Encode(b));",
        "        else if (value is null) writer.WriteNullValue();",
        "        else JsonSerializer.Serialize(writer, value, value.GetType(), options);",
        "    }",
        "}",
        "",
        "/// <summary>A byte string is a base64url string in JSON.</summary>",
        "public sealed class Base64UrlJsonConverter : JsonConverter<byte[]>",
        "{",
        "    public override byte[] Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)",
        "        => Base64Url.Decode(reader.GetString() ?? throw new JsonException(\"expected a base64url string\"));",
        "",
        "    public override void Write(Utf8JsonWriter writer, byte[] value, JsonSerializerOptions options)",
        "        => writer.WriteStringValue(Base64Url.Encode(value));",
        "}",
    ] {
        if l.is_empty() { w.blank(); } else { w.line(l); }
    }
    w.blank();
}

/// A JSON converter for a generated type that isn't a plain class (an enum,
/// an alias, an array, a map): it goes through the type's ToJson / FromJson,
/// so the type can be a member of a class.
fn emit_delegating_json_converter(w: &mut IndentWriter, type_name: &str) {
    w.line(&format!(
        "public sealed class {type_name}JsonConverter : JsonConverter<{type_name}>"
    ));
    w.line("{");
    w.indent();
    w.line(&format!("public override {type_name} Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)"));
    w.line("{");
    w.indent();
    w.line("using var doc = JsonDocument.ParseValue(ref reader);");
    w.line(&format!("return {type_name}.FromJson(System.Text.Encoding.UTF8.GetBytes(doc.RootElement.GetRawText()));"));
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("public override void Write(Utf8JsonWriter writer, {type_name} value, JsonSerializerOptions options)"));
    w.indent();
    w.line("=> writer.WriteRawValue(value.ToJson());");
    w.dedent();
    w.dedent();
    w.line("}");
}

/// A statement writing `expr`, a value of type `ty`, to `writer`.
fn cs_json_write_stmt(ty: &TypeRef, expr: &str, opts: &CodegenOptions) -> String {
    match cs_base(ty) {
        TypeRef::Primitive(Primitive::Bstr) => {
            format!("writer.WriteStringValue(Base64Url.Encode({expr}));")
        }
        TypeRef::Primitive(Primitive::Any) => format!("{expr}.WriteTo(writer);"),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => {
            "writer.WriteNullValue();".into()
        }
        TypeRef::Named(_) => format!("writer.WriteRawValue({expr}.ToJson());"),
        _ if cs_typeref(ty, opts) == "object" => {
            format!("Cddl_.WriteChoice(writer, {expr}, options);")
        }
        _ => format!("JsonSerializer.Serialize(writer, {expr}, options);"),
    }
}

/// A per-member converter for a member whose type is a choice of C# types.
fn emit_choice_member_converter(
    w: &mut IndentWriter,
    name: &str,
    ty: &TypeRef,
    opts: &CodegenOptions,
) {
    w.line(&format!(
        "public sealed class {name} : JsonConverter<object>"
    ));
    w.line("{");
    w.indent();
    w.line("public override object Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)");
    w.line("{");
    w.indent();
    w.line("using var doc = JsonDocument.ParseValue(ref reader);");
    w.line(&format!(
        "return {};",
        cs_json_from_element(ty, "doc.RootElement", opts)
    ));
    w.dedent();
    w.line("}");
    w.blank();
    w.line("public override void Write(Utf8JsonWriter writer, object value, JsonSerializerOptions options)");
    w.indent();
    w.line("=> Cddl_.WriteChoice(writer, value, options);");
    w.dedent();
    w.dedent();
    w.line("}");
}

// ── Array ─────────────────────────────────────────────────────────────────────

/// The record an array of a repeated group holds, whose members sit inline
/// in the array (dgtlrift/cddlc#90).
fn flat_record<'m>(a: &ArrayDef, module: &'m IrModule) -> Option<&'m StructDef> {
    match (&a.element, a.flat) {
        (TypeRef::Named(n), true) => match module.get(n) {
            Some(TypeDef::Struct(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

/// An array of a repeated group (`[* (a: uint, b: tstr)]`): a list of its
/// records, whose members are written inline, one group after another
/// (`[1, "x", 2, "y"]`).
fn emit_flat_array(w: &mut IndentWriter, a: &ArrayDef, rec: &StructDef, opts: &CodegenOptions) {
    emit_doc_comment(w, a.doc.as_deref());
    if a.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&a.name);
    let elem_ty = cs_typeref(&a.element, opts);
    let width = rec.fields.len();

    if opts.is_json() {
        w.line(&format!(
            "[JsonConverter(typeof({type_name}JsonConverter))]"
        ));
    }
    w.line(&format!("public sealed class {type_name}"));
    w.line("{");
    w.indent();
    w.line(&format!("public List<{elem_ty}> Items {{ get; }} = new();"));
    w.blank();
    w.line(&format!("public static {type_name} Default => new();"));
    w.blank();

    if opts.is_json() {
        w.line("public byte[] ToJson() => Cddl_.Json(writer =>");
        w.line("{");
        w.indent();
        w.line("var options = JsonSerializerOptions.Default;");
        w.line("writer.WriteStartArray();");
        w.line("foreach (var item in Items)");
        w.line("{");
        w.indent();
        for f in &rec.fields {
            w.line(&cs_json_write_stmt(
                &f.ty,
                &format!("item.{}", to_pascal_case(&f.name)),
                opts,
            ));
        }
        w.dedent();
        w.line("}");
        w.line("writer.WriteEndArray();");
        w.dedent();
        w.line("});");
        w.blank();
        w.line(&format!("public static {type_name} FromJson(byte[] data)"));
        w.line("{");
        w.indent();
        w.line("using var doc = JsonDocument.Parse(data);");
        w.line("var a = doc.RootElement;");
        w.line(&format!(
            "if (a.ValueKind != JsonValueKind.Array || a.GetArrayLength() % {width} != 0) throw new JsonException(\"{type_name}: expected an array of groups of {width}\");"
        ));
        w.line(&format!("var result = new {type_name}();"));
        w.line(&format!(
            "for (int i = 0; i < a.GetArrayLength(); i += {width})"
        ));
        w.line("{");
        w.indent();
        w.line(&format!("result.Items.Add(new {elem_ty}"));
        w.line("{");
        w.indent();
        for (j, f) in rec.fields.iter().enumerate() {
            w.line(&format!(
                "{} = {},",
                to_pascal_case(&f.name),
                cs_json_from_element(&f.ty, &format!("a[i + {j}]"), opts)
            ));
        }
        w.dedent();
        w.line("});");
        w.dedent();
        w.line("}");
        w.line("return result;");
        w.dedent();
        w.line("}");
    } else {
        w.line("public void Encode(CborWriter writer)");
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
        }
        w.line(&format!("writer.WriteStartArray(Items.Count * {width});"));
        w.line("foreach (var item in Items)");
        w.line("{");
        w.indent();
        for f in &rec.fields {
            emit_cs_encode_value(
                w,
                &f.ty,
                &format!("item.{}", to_pascal_case(&f.name)),
                &Occurrence::Required,
                opts,
            );
        }
        w.dedent();
        w.line("}");
        w.line("writer.WriteEndArray();");
        w.dedent();
        w.line("}");
        w.blank();

        w.line(&format!(
            "public static {type_name} Decode(CborReader reader)"
        ));
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line("var tag = reader.ReadTag();");
            w.line(&format!(
                "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
                tag.0
            ));
        }
        w.line(&format!("var result = new {type_name}();"));
        w.line("var length = reader.ReadStartArray();");
        w.line(&format!(
            "if (length is int n && n % {width} != 0) throw new CborContentException(\"{type_name}: a partial group\");"
        ));
        w.line("var count  = length ?? int.MaxValue;");
        w.line(&format!("for (int i = 0; i < count; i += {width})"));
        w.line("{");
        w.indent();
        w.line("if (length == null && reader.PeekState() == CborReaderState.EndArray) break;");
        for f in &rec.fields {
            let ty = cs_field_type(&f.ty, &Occurrence::Required, opts);
            let local = format!("m_{}", to_snake_case(&f.name));
            let def = cs_default_value(&f.ty, &Occurrence::Required, opts);
            w.line(&format!("{ty} {local} = {def};"));
            emit_cs_decode_field(
                w,
                &f.ty,
                &local,
                &f.constraints,
                &f.name,
                &Occurrence::Required,
                opts,
            );
        }
        w.line(&format!("result.Items.Add(new {elem_ty}"));
        w.line("{");
        w.indent();
        for f in &rec.fields {
            w.line(&format!(
                "{} = m_{},",
                to_pascal_case(&f.name),
                to_snake_case(&f.name)
            ));
        }
        w.dedent();
        w.line("});");
        w.dedent();
        w.line("}");
        w.line("reader.ReadEndArray();");
        w.line("return result;");
        w.dedent();
        w.line("}");
    }

    w.dedent();
    w.line("}");
    if opts.is_json() {
        w.blank();
        emit_delegating_json_converter(w, &type_name);
    }
}

fn emit_array(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    emit_doc_comment(w, a.doc.as_deref());
    if a.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&a.name);
    let elem_ty = cs_typeref(&a.element, opts);

    if opts.is_json() {
        w.line(&format!(
            "[JsonConverter(typeof({type_name}JsonConverter))]"
        ));
    }
    w.line(&format!("public sealed class {type_name}"));
    w.line("{");
    w.indent();

    w.line(&format!("public List<{elem_ty}> Items {{ get; }} = new();"));
    w.blank();
    w.line(&format!("public static {type_name} Default => new();"));
    w.blank();

    if opts.is_json() {
        w.line("public byte[] ToJson() => Cddl_.Json(writer =>");
        w.line("{");
        w.indent();
        w.line("var options = JsonSerializerOptions.Default;");
        w.line("writer.WriteStartArray();");
        w.line(&format!(
            "foreach (var item in Items) {}",
            cs_json_write_stmt(&a.element, "item", opts)
        ));
        w.line("writer.WriteEndArray();");
        w.dedent();
        w.line("});");
        w.blank();
        w.line(&format!("public static {type_name} FromJson(byte[] data)"));
        w.line("{");
        w.indent();
        w.line("using var doc = JsonDocument.Parse(data);");
        w.line(&format!("if (doc.RootElement.ValueKind != JsonValueKind.Array) throw new JsonException(\"{type_name}: expected an array\");"));
        w.line(&format!("var result = new {type_name}();"));
        w.line(&format!(
            "foreach (var item in doc.RootElement.EnumerateArray()) result.Items.Add({});",
            cs_json_from_element(&a.element, "item", opts)
        ));
        w.line("return result;");
        w.dedent();
        w.line("}");
    } else {
        // Encode
        w.line("public void Encode(CborWriter writer)");
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
        }
        w.line("writer.WriteStartArray(Items.Count);");
        w.line("foreach (var item in Items)");
        w.line("{");
        w.indent();
        emit_cs_encode_value(w, &a.element, "item", &Occurrence::Required, opts);
        w.dedent();
        w.line("}");
        w.line("writer.WriteEndArray();");
        w.dedent();
        w.line("}");
        w.blank();

        // Decode
        w.line(&format!(
            "public static {type_name} Decode(CborReader reader)"
        ));
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line("var tag = reader.ReadTag();");
            w.line(&format!(
                "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
                tag.0
            ));
        }
        w.line(&format!("var result = new {type_name}();"));
        w.line("var length = reader.ReadStartArray();");
        w.line("var count  = length ?? int.MaxValue;");
        w.line("for (int i = 0; i < count; i++)");
        w.line("{");
        w.indent();
        w.line("if (length == null && reader.PeekState() == CborReaderState.EndArray) break;");
        let decode_expr = cs_decode_expr(&a.element, opts);
        w.line(&format!("result.Items.Add({decode_expr});"));
        w.dedent();
        w.line("}");
        w.line("reader.ReadEndArray();");
        w.line("return result;");
        w.dedent();
        w.line("}");
    }

    w.dedent();
    w.line("}");
    if opts.is_json() {
        w.blank();
        emit_delegating_json_converter(w, &type_name);
    }
}

// ── Map ───────────────────────────────────────────────────────────────────────

/// A table (`{ * tstr => uint }`): a class holding a `Dictionary` (dgtlrift/cddlc#82).
fn emit_map(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    emit_doc_comment(w, m.doc.as_deref());
    if m.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&m.name);
    let key_ty = cs_typeref(&m.key_ty, opts);
    let val_ty = cs_typeref(&m.val_ty, opts);

    if opts.is_json() {
        w.line(&format!(
            "[JsonConverter(typeof({type_name}JsonConverter))]"
        ));
    }
    w.line(&format!("public sealed class {type_name}"));
    w.line("{");
    w.indent();

    w.line(&format!(
        "public Dictionary<{key_ty}, {val_ty}> Entries {{ get; }} = new();"
    ));
    w.blank();
    w.line(&format!("public static {type_name} Default => new();"));
    w.blank();

    if opts.is_json() {
        // JSON object member names are strings; other keys are written as text.
        let (key_out, key_in) = if key_ty == "string" {
            ("kv.Key".to_owned(), "p.Name".to_owned())
        } else if matches!(m.key_ty, TypeRef::Named(_)) {
            // A named key (`Claim-Label = int / text`, RFC 9597) goes through
            // its own JSON: an int key is its digits.
            (
                "Cddl_.KeyText(kv.Key.ToJson())".to_owned(),
                format!("{key_ty}.FromJson(Cddl_.KeyJson(p.Name))"),
            )
        } else {
            (
                "System.Convert.ToString(kv.Key, System.Globalization.CultureInfo.InvariantCulture)!".to_owned(),
                format!("{key_ty}.Parse(p.Name, System.Globalization.CultureInfo.InvariantCulture)"),
            )
        };
        w.line("public byte[] ToJson() => Cddl_.Json(writer =>");
        w.line("{");
        w.indent();
        w.line("var options = JsonSerializerOptions.Default;");
        w.line("writer.WriteStartObject();");
        w.line("foreach (var kv in Entries)");
        w.line("{");
        w.indent();
        w.line(&format!("writer.WritePropertyName({key_out});"));
        w.line(&cs_json_write_stmt(&m.val_ty, "kv.Value", opts));
        w.dedent();
        w.line("}");
        w.line("writer.WriteEndObject();");
        w.dedent();
        w.line("});");
        w.blank();
        w.line(&format!("public static {type_name} FromJson(byte[] data)"));
        w.line("{");
        w.indent();
        w.line("using var doc = JsonDocument.Parse(data);");
        w.line(&format!("if (doc.RootElement.ValueKind != JsonValueKind.Object) throw new JsonException(\"{type_name}: expected an object\");"));
        w.line(&format!("var result = new {type_name}();"));
        w.line("foreach (var p in doc.RootElement.EnumerateObject())");
        w.line(&format!(
            "    if (!result.Entries.TryAdd({key_in}, {})) throw new JsonException(\"Duplicate map key\");",
            cs_json_from_element(&m.val_ty, "p.Value", opts)
        ));
        w.line("return result;");
        w.dedent();
        w.line("}");
    } else {
        w.line("public void Encode(CborWriter writer)");
        w.line("{");
        w.indent();
        if let Some(tag) = m.tagged {
            w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
        }
        w.line("writer.WriteStartMap(Entries.Count);");
        w.line("foreach (var kv in Entries)");
        w.line("{");
        w.indent();
        emit_cs_encode_value(w, &m.key_ty, "kv.Key", &Occurrence::Required, opts);
        emit_cs_encode_value(w, &m.val_ty, "kv.Value", &Occurrence::Required, opts);
        w.dedent();
        w.line("}");
        w.line("writer.WriteEndMap();");
        w.dedent();
        w.line("}");
        w.blank();

        w.line(&format!(
            "public static {type_name} Decode(CborReader reader)"
        ));
        w.line("{");
        w.indent();
        if let Some(tag) = m.tagged {
            w.line("var tag = reader.ReadTag();");
            w.line(&format!(
                "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
                tag.0
            ));
        }
        w.line(&format!("var result = new {type_name}();"));
        w.line("reader.ReadStartMap();");
        w.line("while (reader.PeekState() != CborReaderState.EndMap)");
        w.line("{");
        w.indent();
        w.line(&format!("var key = {};", cs_decode_expr(&m.key_ty, opts)));
        w.line(&format!("var value = {};", cs_decode_expr(&m.val_ty, opts)));
        w.line("if (!result.Entries.TryAdd(key, value)) throw new CborContentException(\"Duplicate map key\");");
        w.dedent();
        w.line("}");
        w.line("reader.ReadEndMap();");
        w.line("return result;");
        w.dedent();
        w.line("}");
    }

    w.dedent();
    w.line("}");
    if opts.is_json() {
        w.blank();
        emit_delegating_json_converter(w, &type_name);
    }
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    emit_doc_comment(w, a.doc.as_deref());
    if a.deprecated {
        w.line("[Obsolete]");
    }

    let type_name = to_pascal_case(&a.name);
    let inner = cs_typeref(&a.ty, opts);
    let exc_name = if opts.is_json() {
        "JsonValidationException"
    } else {
        "CborValidationException"
    };

    if opts.is_json() {
        w.line(&format!(
            "[JsonConverter(typeof({type_name}JsonConverter))]"
        ));
    }
    if a.constraints.is_empty() && a.tagged.is_none() {
        w.line(&format!("public sealed class {type_name}"));
        w.line("{");
        w.indent();
        w.line(&format!(
            "public {inner} Value {{ get; set; }} = {};",
            cs_type_default(&a.ty, opts)
        ));
        w.blank();
        w.line(&format!("public static {type_name} Default => new();"));
        w.blank();
        if opts.is_json() {
            w.line(&format!(
                "public byte[] ToJson() => {};",
                cs_json_to_bytes(&a.ty, "Value", opts)
            ));
            w.blank();
            w.line(&format!("public static {type_name} FromJson(byte[] data)"));
            w.line("{");
            w.indent();
            w.line("using var doc = JsonDocument.Parse(data);");
            w.line(&format!(
                "return new {type_name} {{ Value = {} }};",
                cs_json_from_element(&a.ty, "doc.RootElement", opts)
            ));
            w.dedent();
            w.line("}");
        } else {
            w.line("public void Encode(CborWriter writer)");
            w.line("{");
            w.indent();
            emit_cs_encode_value(w, &a.ty, "this.Value", &Occurrence::Required, opts);
            w.dedent();
            w.line("}");
            w.blank();
            w.line(&format!(
                "public static {type_name} Decode(CborReader reader)"
            ));
            w.line("{");
            w.indent();
            let decode_expr = cs_decode_expr(&a.ty, opts);
            w.line(&format!(
                "return new {type_name} {{ Value = {decode_expr} }};"
            ));
            w.dedent();
            w.line("}");
        }
        w.dedent();
        w.line("}");
        if opts.is_json() {
            w.blank();
            emit_delegating_json_converter(w, &type_name);
        }
        return;
    }

    // Constrained or tagged alias
    w.line(&format!("public sealed class {type_name}"));
    w.line("{");
    w.indent();

    w.line(&format!("private readonly {inner} _value;"));
    w.blank();
    w.line(&format!("public {inner} Value => _value;"));
    w.blank();
    w.line(&format!(
        "public static {type_name} Default => new({});",
        cs_valid_constrained_value(&a.constraints, &a.ty, opts)
    ));
    w.blank();

    // Constructor with validation
    w.line(&format!("public {type_name}({inner} value)"));
    w.line("{");
    w.indent();
    for c in &a.constraints {
        if let Some(check) = cs_constraint_check(c, "value", &a.ty) {
            let msg = constraint_description(c);
            w.line(&format!("if (!({check}))"));
            w.indent();
            w.line(&format!(
                "throw new {exc_name}(nameof({type_name}), {msg:?});"
            ));
            w.dedent();
        }
    }
    w.line("_value = value;");
    w.dedent();
    w.line("}");
    w.blank();

    if opts.is_json() {
        w.line(&format!(
            "public byte[] ToJson() => {};",
            cs_json_to_bytes(&a.ty, "_value", opts)
        ));
        w.blank();
        w.line(&format!("public static {type_name} FromJson(byte[] data)"));
        w.line("{");
        w.indent();
        w.line("using var doc = JsonDocument.Parse(data);");
        w.line(&format!(
            "return new {type_name}({});",
            cs_json_from_element(&a.ty, "doc.RootElement", opts)
        ));
        w.dedent();
        w.line("}");
    } else {
        w.line("public void Encode(CborWriter writer)");
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
        }
        emit_cs_encode_value(w, &a.ty, "this._value", &Occurrence::Required, opts);
        w.dedent();
        w.line("}");
        w.blank();

        w.line(&format!(
            "public static {type_name} Decode(CborReader reader)"
        ));
        w.line("{");
        w.indent();
        if let Some(tag) = a.tagged {
            w.line("var tag = reader.ReadTag();");
            w.line(&format!(
                "if ((ulong)tag != {}) throw new CborContentException(\"Unexpected CBOR tag\");",
                tag.0
            ));
        }
        let decode_expr = cs_decode_expr(&a.ty, opts);
        w.line(&format!("var raw = {decode_expr};"));
        w.line(&format!("return new {type_name}(raw);"));
        w.dedent();
        w.line("}");
    }

    w.dedent();
    w.line("}");
    if opts.is_json() {
        w.blank();
        emit_delegating_json_converter(w, &type_name);
    }
}

// ── Encode helpers ────────────────────────────────────────────────────────────

/// Is this C# type a value type (so `T?` is `Nullable<T>`, read through `.Value`)?
fn cs_is_value_type(ty: &TypeRef, opts: &CodegenOptions) -> bool {
    matches!(
        cs_typeref(ty, opts).as_str(),
        "bool" | "ulong" | "long" | "float" | "double"
    )
}

/// The expression for a present optional member's value.
fn cs_present_expr(ty: &TypeRef, expr: &str, opts: &CodegenOptions) -> String {
    if cs_is_value_type(ty, opts) {
        format!("{expr}.Value")
    } else {
        expr.to_owned()
    }
}

fn emit_cs_encode_value(
    w: &mut IndentWriter,
    ty: &TypeRef,
    expr: &str,
    occ: &Occurrence,
    opts: &CodegenOptions,
) {
    match occ {
        Occurrence::Optional => {
            w.line(&format!("if ({expr} is not null)"));
            w.line("{");
            w.indent();
            emit_cs_encode_typeref(w, ty, &cs_present_expr(ty, expr, opts), opts);
            w.dedent();
            w.line("} else { writer.WriteNull(); }");
        }
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => {
            w.line(&format!("writer.WriteStartArray({expr}.Count);"));
            w.line(&format!("foreach (var item in {expr})"));
            w.line("{");
            w.indent();
            emit_cs_encode_typeref(w, ty, "item", opts);
            w.dedent();
            w.line("}");
            w.line("writer.WriteEndArray();");
        }
        Occurrence::Required => {
            emit_cs_encode_typeref(w, ty, expr, opts);
        }
    }
}

fn emit_cs_encode_typeref(w: &mut IndentWriter, ty: &TypeRef, expr: &str, opts: &CodegenOptions) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = cs_encode_call(p, expr, opts);
            w.line(&format!("{call};"));
        }
        TypeRef::Named(_) => {
            w.line(&format!("{expr}.Encode(writer);"));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line(&format!("writer.WriteTag((CborTag){});", tag.0));
            emit_cs_encode_typeref(w, inner, expr, opts);
        }
        TypeRef::Choice(alts) => {
            if cs_typeref(ty, opts) != "object" {
                // Every alternative is the same C# type: write it as the first.
                if let Some(first) = alts.first() {
                    emit_cs_encode_typeref(w, first, expr, opts);
                }
                return;
            }
            // Write the alternative the value holds.
            // Named after the expression, so a nested switch gets its own.
            let var: String = format!(
                "alt_{}",
                expr.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
            );
            let mut seen = Vec::new();
            // A nested choice's alternatives are this one's (`number /
            // script.SpecialNumber`, WebDriver BiDi): its `case object`
            // would take every value, leaving the rest unreachable.
            fn flatten<'a>(cs: &'a [TypeRef], out: &mut Vec<&'a TypeRef>) {
                for c in cs {
                    match c {
                        TypeRef::Choice(inner) => flatten(inner, out),
                        other => out.push(other),
                    }
                }
            }
            let mut flat = Vec::new();
            flatten(alts, &mut flat);
            w.line(&format!("switch ({expr})"));
            w.line("{");
            w.indent();
            for alt in flat {
                let alt_ty = cs_typeref(alt, opts);
                if seen.contains(&alt_ty) {
                    continue;
                }
                seen.push(alt_ty.clone());
                if matches!(cs_base(alt), TypeRef::Primitive(Primitive::Null)) {
                    w.line("case null:");
                } else {
                    w.line(&format!("case {alt_ty} {var}:"));
                }
                w.indent();
                emit_cs_encode_typeref(w, alt, &var, opts);
                w.line("break;");
                w.dedent();
            }
            w.line("default:");
            w.indent();
            w.line(&format!(
                "throw new InvalidOperationException($\"{{{expr}?.GetType()}} isn't one of the choice's alternatives\");"
            ));
            w.dedent();
            w.dedent();
            w.line("}");
        }
    }
}

/// A type with its tags taken off.
fn cs_base(ty: &TypeRef) -> &TypeRef {
    match ty {
        TypeRef::Tagged(_, inner) => cs_base(inner),
        _ => ty,
    }
}

/// Floats are written at their CDDL width (`float32` as four bytes), as the
/// other backends write them; CborWriter alone would write the shortest
/// form, which only dCBOR asks for.
fn cs_encode_call(p: &Primitive, expr: &str, opts: &CodegenOptions) -> String {
    match p {
        Primitive::Float32 if !opts.dcbor => format!("Cddl_.WriteFloat32(writer, {expr})"),
        Primitive::Float64 | Primitive::Float if !opts.dcbor => {
            format!("Cddl_.WriteFloat64(writer, {expr})")
        }
        Primitive::Bool => format!("writer.WriteBoolean({expr})"),
        Primitive::Null => "writer.WriteNull()".into(),
        Primitive::Undefined => "writer.WriteSimpleValue(CborSimpleValue.Undefined)".into(),
        Primitive::Uint => format!("writer.WriteUInt64({expr})"),
        Primitive::Int => format!("writer.WriteInt64({expr})"),
        Primitive::Float16 => format!("writer.WriteHalf((Half){expr})"),
        Primitive::Float32 => format!("writer.WriteSingle({expr})"),
        Primitive::Float64 | Primitive::Float => format!("writer.WriteDouble({expr})"),
        Primitive::Tstr => format!("writer.WriteTextString({expr})"),
        Primitive::Bstr => format!("writer.WriteByteString({expr})"),
        // The encoded item, written back unchanged (dgtlrift/cddlc#178).
        Primitive::Any => format!("writer.WriteEncodedValue({expr})"),
    }
}

/// Helpers the generated CBOR code calls: reading a null or undefined as a
/// value, a tagged value, and an inline choice (each alternative is tried on
/// the encoded item in turn).
fn emit_cbor_helpers(w: &mut IndentWriter) {
    for l in [
        "internal static class Cddl_",
        "{",
        "    public static object? ReadNull(CborReader reader) { reader.ReadNull(); return null; }",
        "",
        "    public static void WriteFloat32(CborWriter writer, float value)",
        "    {",
        "        var b = new byte[5];",
        "        b[0] = 0xfa;",
        "        System.Buffers.Binary.BinaryPrimitives.WriteSingleBigEndian(b.AsSpan(1), value);",
        "        writer.WriteEncodedValue(b);",
        "    }",
        "",
        "    public static void WriteFloat64(CborWriter writer, double value)",
        "    {",
        "        var b = new byte[9];",
        "        b[0] = 0xfb;",
        "        System.Buffers.Binary.BinaryPrimitives.WriteDoubleBigEndian(b.AsSpan(1), value);",
        "        writer.WriteEncodedValue(b);",
        "    }",
        "",
        "    public static object? ReadUndefined(CborReader reader)",
        "    {",
        "        if (reader.ReadSimpleValue() != CborSimpleValue.Undefined) throw new CborContentException(\"Expected undefined\");",
        "        return null;",
        "    }",
        "",
        "    public static T Tagged<T>(CborReader reader, ulong tag, Func<CborReader, T> read)",
        "    {",
        "        var t = (ulong)reader.ReadTag();",
        "        if (t != tag) throw new CborContentException($\"Expected tag {tag}, got {t}\");",
        "        return read(reader);",
        "    }",
        "",
        "    public static object Choice(CborReader reader, params Func<CborReader, object?>[] alternatives)",
        "    {",
        "        var encoded = reader.ReadEncodedValue();",
        "        foreach (var alternative in alternatives)",
        "        {",
        "            try",
        "            {",
        "                var r = new CborReader(encoded);",
        "                var value = alternative(r);",
        "                if (r.BytesRemaining == 0) return value!;",
        "            }",
        "            catch (Exception e) when (e is CborContentException or InvalidOperationException",
        "                                      or OverflowException or FormatException or CborValidationException) { }",
        "        }",
        "        throw new CborContentException(\"The value is none of the choice's alternatives\");",
        "    }",
        "}",
    ] {
        if l.is_empty() { w.blank(); } else { w.line(l); }
    }
    w.blank();
}

// ── Decode helpers ────────────────────────────────────────────────────────────

fn emit_cs_decode_field(
    w: &mut IndentWriter,
    ty: &TypeRef,
    local: &str,
    constraints: &[Constraint],
    field_name: &str,
    occ: &Occurrence,
    opts: &CodegenOptions,
) {
    match occ {
        Occurrence::Optional => {
            w.line(&format!("if (reader.PeekState() == CborReaderState.Null) {{ reader.ReadNull(); {local} = null; }}"));
            w.line("else");
            w.line("{");
            w.indent();
            let inner_expr = cs_decode_expr(ty, opts);
            w.line(&format!("{local} = {inner_expr};"));
            emit_cs_validation(
                w,
                constraints,
                &cs_present_expr(ty, local, opts),
                field_name,
                ty,
                opts,
            );
            w.dedent();
            w.line("}");
        }
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => {
            w.line(&format!("{local} = new List<{}>();", cs_typeref(ty, opts)));
            w.line("{");
            w.indent();
            w.line("var n = reader.ReadStartArray();");
            w.line(&format!("while (n is int k ? {local}.Count < k : reader.PeekState() != CborReaderState.EndArray)"));
            w.indent();
            w.line(&format!("{local}.Add({});", cs_decode_expr(ty, opts)));
            w.dedent();
            w.line("reader.ReadEndArray();");
            w.dedent();
            w.line("}");
        }
        Occurrence::Required => {
            let decode_expr = cs_decode_expr(ty, opts);
            w.line(&format!("{local} = {decode_expr};"));
            emit_cs_validation(w, constraints, local, field_name, ty, opts);
        }
    }
}

fn emit_cs_validation(
    w: &mut IndentWriter,
    constraints: &[Constraint],
    expr: &str,
    field_name: &str,
    ty: &TypeRef,
    opts: &CodegenOptions,
) {
    let exc_name = if opts.is_json() {
        "JsonValidationException"
    } else {
        "CborValidationException"
    };
    for c in constraints {
        if let Some(check) = cs_constraint_check(c, expr, ty) {
            let msg = constraint_description(c);
            w.line(&format!("if (!({check}))"));
            w.indent();
            w.line(&format!("throw new {exc_name}({:?}, {msg:?});", field_name));
            w.dedent();
        }
    }
}

fn cs_decode_expr(ty: &TypeRef, opts: &CodegenOptions) -> String {
    cs_decode_expr_from(ty, "reader", opts)
}

/// An expression decoding `ty` from the CborReader named `reader`.
fn cs_decode_expr_from(ty: &TypeRef, reader: &str, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => cs_decode_call(p, reader),
        TypeRef::Named(n) => format!("{}.Decode({reader})", to_pascal_case(n)),
        TypeRef::Tagged(tag, inner) => {
            let r = format!("{reader}1");
            format!(
                "Cddl_.Tagged({reader}, {}UL, {r} => {})",
                tag.0,
                cs_decode_expr_from(inner, &r, opts)
            )
        }
        TypeRef::Choice(alts) => {
            let r = format!("{reader}1");
            let tries: Vec<String> = alts
                .iter()
                .map(|a| format!("{r} => (object?){}", cs_decode_expr_from(a, &r, opts)))
                .collect();
            let call = format!("Cddl_.Choice({reader}, {})", tries.join(", "));
            match cs_typeref(ty, opts).as_str() {
                "object" => call,
                t => format!("({t}){call}"),
            }
        }
    }
}

fn cs_decode_call(p: &Primitive, reader: &str) -> String {
    match p {
        Primitive::Bool => format!("{reader}.ReadBoolean()"),
        Primitive::Null => format!("Cddl_.ReadNull({reader})"),
        Primitive::Undefined => format!("Cddl_.ReadUndefined({reader})"),
        Primitive::Uint => format!("{reader}.ReadUInt64()"),
        Primitive::Int => format!("{reader}.ReadInt64()"),
        Primitive::Float16 => format!("(float){reader}.ReadHalf()"),
        Primitive::Float32 => format!("{reader}.ReadSingle()"),
        Primitive::Float64 | Primitive::Float => format!("{reader}.ReadDouble()"),
        Primitive::Tstr => format!("{reader}.ReadTextString()"),
        Primitive::Bstr => format!("{reader}.ReadByteString()"),
        // The whole item's encoding.
        Primitive::Any => format!("{reader}.ReadEncodedValue().ToArray()"),
    }
}

// ── Constraint checks ─────────────────────────────────────────────────────────

/// A C# boolean expression checking `c` on `expr`, a value of type `ty`.
/// The lower bound is always inclusive; `inclusive` is the upper bound's.
fn cs_constraint_check(c: &Constraint, expr: &str, ty: &TypeRef) -> Option<String> {
    let p = match cs_base(ty) {
        TypeRef::Primitive(p) => Some(p.clone()),
        _ => None,
    };
    let is_uint = matches!(p, Some(Primitive::Uint));
    let is_int = matches!(p, Some(Primitive::Int));
    let is_float = matches!(
        p,
        Some(Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float)
    );
    let numeric = is_uint || is_int || is_float;
    let len = match p {
        Some(Primitive::Tstr) => Some(format!("System.Text.Encoding.UTF8.GetByteCount({expr})")),
        Some(Primitive::Bstr) => Some(format!("{expr}.Length")),
        _ => None,
    };
    let join = |parts: Vec<String>| {
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" && "))
        }
    };
    // A bound written as a literal of the value's own type.
    let lit = |n: i128| -> String {
        if is_uint {
            format!("{n}UL")
        } else if is_int {
            format!("{n}L")
        } else {
            format!("{n}.0")
        }
    };
    match c {
        Constraint::SizeExact(n) => len.map(|l| format!("{l} == {n}")),
        Constraint::SizeRange { min, max } => {
            let l = len?;
            let mut parts = Vec::new();
            if let Some(lo) = min {
                parts.push(format!("{l} >= {lo}"));
            }
            if let Some(hi) = max {
                parts.push(format!("{l} <= {hi}"));
            }
            join(parts)
        }
        Constraint::ValueRangeInt {
            min,
            max,
            inclusive,
        } if numeric => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if let Some(lo) = min.filter(|lo| !(is_uint && *lo <= 0)) {
                parts.push(format!("{expr} >= {}", lit(lo as i128)));
            }
            match max {
                Some(hi) if is_uint && *hi < 0 => return Some("false".into()),
                Some(hi) => parts.push(format!("{expr} {le} {}", lit(*hi as i128))),
                None => {}
            }
            join(parts)
        }
        Constraint::ValueRangeUint {
            min,
            max,
            inclusive,
        } if numeric => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if min.is_some_and(|lo| is_int && lo > i64::MAX as u64) {
                return Some("false".into());
            }
            if let Some(lo) = min.filter(|lo| *lo > 0) {
                parts.push(format!("{expr} >= {}", lit(lo as i128)));
            }
            if let Some(hi) = max.filter(|hi| !is_int || *hi <= i64::MAX as u64) {
                parts.push(format!("{expr} {le} {}", lit(hi as i128)));
            }
            join(parts)
        }
        Constraint::ValueRangeF64 {
            min,
            max,
            inclusive,
        } if is_float => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if let Some(lo) = min {
                parts.push(format!("{expr} >= {lo:?}"));
            }
            if let Some(hi) = max {
                parts.push(format!("{expr} {le} {hi:?}"));
            }
            join(parts)
        }
        Constraint::Eq(v) | Constraint::Ne(v) => {
            let l = match (v, p) {
                (LiteralValue::Bool(_), Some(Primitive::Bool))
                | (LiteralValue::Text(_), Some(Primitive::Tstr)) => cs_literal(v),
                (LiteralValue::Uint(n), _) if numeric => lit(*n as i128),
                (LiteralValue::Int(n), _) if is_int || is_float => lit(*n as i128),
                (LiteralValue::Float(f), _) if is_float => format!("{f:?}"),
                _ => return None,
            };
            let op = if matches!(c, Constraint::Eq(_)) {
                "=="
            } else {
                "!="
            };
            Some(format!("{expr} {op} {l}"))
        }
        Constraint::Regexp { hook: Some(h), .. } => Some(format!("{h}({expr})")),
        // `.regexp` without a hook: the whole text matches (.NET's grammar).
        Constraint::Regexp {
            pattern,
            hook: None,
        } if matches!(p, Some(Primitive::Tstr)) => Some(format!(
            "System.Text.RegularExpressions.Regex.IsMatch({expr}, @\"\\A(?:{})\\z\")",
            pattern.replace('"', "\"\"")
        )),
        // `.bits` (dgtlrift/cddlc#107): no bit outside the mask is set.
        Constraint::Bits(bits) if is_uint => Some(format!(
            "({expr} & ~{:#x}UL) == 0",
            cddlc_codegen::bits_mask_u64(bits)
        )),
        Constraint::Bits(bits) if matches!(p, Some(Primitive::Bstr)) => {
            let m = cddlc_codegen::bits_mask_bytes(bits);
            let bytes = m
                .iter()
                .map(|b| format!("{b:#04x}"))
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "System.Linq.Enumerable.All(System.Linq.Enumerable.Select({expr}, (b, i) => (b & ~(i < {n} ? new byte[] {{ {bytes} }}[i] : 0)) == 0), ok => ok)",
                n = m.len(),
            ))
        }
        _ => None,
    }
}

fn constraint_description(c: &Constraint) -> String {
    match c {
        Constraint::SizeExact(n) => format!("length must be {n}"),
        Constraint::SizeRange { min, max } => format!("length out of range ({min:?}..{max:?})"),
        Constraint::ValueRangeInt { min, max, .. } => {
            format!("value out of range ({min:?}..{max:?})")
        }
        Constraint::ValueRangeUint { min, max, .. } => {
            format!("value out of range ({min:?}..{max:?})")
        }
        Constraint::Eq(v) => format!("must equal {}", cs_literal(v)),
        Constraint::Ne(v) => format!("must not equal {}", cs_literal(v)),
        Constraint::Bits(b) => format!("only bits {b:?} may be set"),
        _ => "constraint violated".into(),
    }
}

fn cs_literal(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Bool(b) => b.to_string().to_lowercase(),
        LiteralValue::Null => "null".into(),
        LiteralValue::Uint(n) => format!("{n}UL"),
        LiteralValue::Int(n) => format!("{n}L"),
        LiteralValue::Float(f) => format!("{f:?}"),
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(_) => "null /* bytes */".into(),
    }
}

// ── C# type helpers ───────────────────────────────────────────────────────────

fn cs_field_type(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    let base = cs_typeref(ty, opts);
    match occ {
        Occurrence::Optional => format!("{base}?"),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => format!("List<{base}>"),
        Occurrence::Required => base,
    }
}

fn cs_default_value(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::Optional => "null".into(),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => format!("new List<{}>() ", cs_typeref(ty, opts)),
        Occurrence::Required => cs_type_default(ty, opts),
    }
}

fn cs_type_default(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Bool => "false".into(),
            Primitive::Uint => "0UL".into(),
            Primitive::Int => "0L".into(),
            Primitive::Float16 | Primitive::Float32 => "0.0f".into(),
            Primitive::Float64 | Primitive::Float => "0.0".into(),
            Primitive::Tstr => "string.Empty".into(),
            Primitive::Bstr => "Array.Empty<byte>()".into(),
            // `null`, as an item or a JSON value: an empty one is neither.
            Primitive::Any if opts.is_json() => "JsonDocument.Parse(\"null\").RootElement".into(),
            Primitive::Any => "new byte[] { 0xf6 }".into(),
            Primitive::Null | Primitive::Undefined => "null!".into(),
        },
        TypeRef::Named(n) => format!("{}.Default", to_pascal_case(n)),
        TypeRef::Tagged(_, inner) => cs_type_default(inner, opts),
        TypeRef::Choice(cs) => cs
            .first()
            .map(|c| cs_type_default(c, opts))
            .unwrap_or_else(|| "null!".into()),
    }
}

/// C#'s keywords, which a local can't be named without an `@`.
const CS_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

/// A decoder's local for a member: its snake_case name, with an `@` when
/// that's a C# keyword (`ref`, `protected`, `params`).
fn cs_local(name: &str) -> String {
    let s = to_snake_case(name);
    if CS_KEYWORDS.contains(&s.as_str()) {
        format!("@{s}")
    } else {
        s
    }
}

fn cs_typeref(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        // `any` (dgtlrift/cddlc#178): the item's CBOR encoding (`byte[]`), or
        // any JSON value.
        TypeRef::Primitive(Primitive::Any) if opts.is_json() => "JsonElement".into(),
        TypeRef::Primitive(p) => cs_primitive(p).to_owned(),
        TypeRef::Named(n) => to_pascal_case(n),
        TypeRef::Tagged(_, inner) => cs_typeref(inner, opts),
        // One C# type if every alternative has it, otherwise `object`.
        TypeRef::Choice(cs) => {
            let mut tys: Vec<String> = cs.iter().map(|c| cs_typeref(c, opts)).collect();
            tys.dedup();
            if tys.len() == 1 {
                tys.remove(0)
            } else {
                "object".into()
            }
        }
    }
}

fn cs_primitive(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::Null | Primitive::Undefined => "object",
        Primitive::Uint => "ulong",
        Primitive::Int => "long",
        Primitive::Float16 => "float",
        Primitive::Float32 => "float",
        Primitive::Float64 | Primitive::Float => "double",
        Primitive::Tstr => "string",
        Primitive::Bstr | Primitive::Any => "byte[]",
    }
}

// ── Test file ─────────────────────────────────────────────────────────────────

fn emit_tests(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = to_pascal_case(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// xUnit roundtrip tests");
    w.blank();
    w.line("using System;");
    w.line("using System.Collections.Generic;");
    if opts.is_json() {
        w.line("using System.Text.Json;");
    } else {
        w.line("using System.Formats.Cbor;");
    }
    w.line("using Xunit;");
    w.blank();
    w.line(&format!("namespace {ns}.Tests;"));
    w.blank();
    w.line("public sealed class RoundtripTests");
    w.line("{");
    w.indent();

    for (_, def) in &module.types {
        emit_type_tests(&mut w, def, module, opts);
        w.blank();
    }

    w.dedent();
    w.line("}");
    w.finish()
}

fn emit_type_tests(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct_tests(w, s, opts),
        TypeDef::Enum(e) => emit_enum_tests(w, e, module, opts),
        TypeDef::Alias(a) => emit_alias_tests(w, a, opts),
        TypeDef::Array(a) => emit_array_tests(w, a, opts),
        TypeDef::Map(m) => emit_map_tests(w, m, opts),
    }
}

fn emit_struct_tests(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&s.name);
    let test_pfx = to_pascal_case(&s.name);

    // Tier 1: default roundtrip
    w.line("[Fact]");
    w.line(&format!("public void {test_pfx}_Roundtrip_Default()"));
    w.line("{");
    w.indent();
    w.line(&format!("var original = new {type_name}"));
    w.line("{");
    w.indent();
    for f in &s.fields {
        let pname = to_pascal_case(&f.name);
        let val = match &f.occurrence {
            Occurrence::Optional => "null".into(),
            Occurrence::Required if !f.constraints.is_empty() => {
                cs_valid_constrained_value(&f.constraints, &f.ty, opts)
            }
            occ if cddlc_codegen::repeat_bounds(occ).0 > 0 => cs_needed_items(f, opts),
            occ => cs_default_value(&f.ty, occ, opts).trim().to_owned(),
        };
        w.line(&format!("{pname} = {val},"));
    }
    w.dedent();
    w.line("};");
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.dedent();
    w.line("}");
    w.blank();

    // Tier 2: explicit values
    w.line("[Fact]");
    w.line(&format!("public void {test_pfx}_Roundtrip_Explicit()"));
    w.line("{");
    w.indent();
    w.line(&format!("var original = new {type_name}"));
    w.line("{");
    w.indent();
    for f in &s.fields {
        let pname = to_pascal_case(&f.name);
        let val = match &f.occurrence {
            Occurrence::Required if !f.constraints.is_empty() => {
                cs_valid_constrained_value(&f.constraints, &f.ty, opts)
            }
            occ if cddlc_codegen::repeat_bounds(occ).0 > 0 => cs_needed_items(f, opts),
            occ => cs_explicit_value(&f.ty, occ, opts),
        };
        w.line(&format!("{pname} = {val},"));
    }
    w.dedent();
    w.line("};");
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.dedent();
    w.line("}");

    if !opts.is_json() {
        if let Some(tag) = s.tagged {
            w.blank();
            w.line("[Fact]");
            w.line(&format!("public void {test_pfx}_Encode_HasTag_{}()", tag.0));
            w.line("{");
            w.indent();
            w.line(&format!("var obj = {type_name}.Default;"));
            w.line("var writer = new CborWriter();");
            w.line("obj.Encode(writer);");
            w.line("var bytes = writer.Encode();");
            w.line(&format!("// First bytes should contain tag {}", tag.0));
            w.line("Assert.NotEmpty(bytes);");
            w.dedent();
            w.line("}");
        }
    }
}

fn emit_enum_tests(w: &mut IndentWriter, e: &EnumDef, _module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&e.name);

    for v in &e.variants {
        let vname = cddlc_codegen::csharp_variant_name(&v.name);
        w.line("[Fact]");
        w.line(&format!("public void {type_name}_{vname}_Roundtrip()"));
        w.line("{");
        w.indent();
        let value = cs_explicit_value(&v.ty, &Occurrence::Required, opts);
        let construct = match (enum_literal(v).is_some(), opts.is_json()) {
            (true, false) => format!("{type_name}.{vname}.Instance"),
            (true, true) => format!("{type_name}.Create{vname}()"),
            (false, false) => format!("new {type_name}.{vname}({value})"),
            (false, true) => format!("{type_name}.Create{vname}({value})"),
        };
        w.line(&format!("{type_name} original = {construct};"));
        if opts.is_json() {
            emit_roundtrip_assert_json(w, &type_name);
        } else {
            emit_roundtrip_assert(w, &type_name);
        }
        // A map variant with the same members as an earlier one decodes as
        // that one; the encodings, compared above, are the same
        // (dgtlrift/cddlc#192).
        // Alternatives may overlap (one with the same members as an earlier
        // one, dgtlrift/cddlc#192, or one that takes any map, WebDriver BiDi's
        // `EmptyResult = { Extensible }`): the encodings, compared above, are
        // what must match, not the variant decoded.
        w.dedent();
        w.line("}");
        w.blank();
    }
}

fn emit_alias_tests(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);

    w.line("[Fact]");
    w.line(&format!("public void {type_name}_Roundtrip_Default()"));
    w.line("{");
    w.indent();

    // For constrained aliases, produce a valid value
    let val = if a.constraints.is_empty() {
        cs_type_default(&a.ty, opts)
    } else {
        cs_valid_constrained_value(&a.constraints, &a.ty, opts)
    };

    if a.constraints.is_empty() && a.tagged.is_none() {
        w.line(&format!(
            "var original = new {type_name} {{ Value = {val} }};"
        ));
    } else {
        w.line(&format!("var original = new {type_name}({val});"));
    }
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.dedent();
    w.line("}");

    // Constraint rejection test
    if let Some(bad) = cs_invalid_constrained_value(&a.constraints, &a.ty, opts) {
        w.blank();
        w.line("[Fact]");
        w.line(&format!(
            "public void {type_name}_Invalid_ThrowsValidation()"
        ));
        w.line("{");
        w.indent();
        let exc_name = if opts.is_json() {
            "JsonValidationException"
        } else {
            "CborValidationException"
        };
        w.line(&format!(
            "Assert.Throws<{exc_name}>(() => new {type_name}({bad}));"
        ));
        w.dedent();
        w.line("}");
    }
}

fn emit_array_tests(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);

    w.line("[Fact]");
    w.line(&format!("public void {type_name}_Roundtrip_Empty()"));
    w.line("{");
    w.indent();
    w.line(&format!("var original = new {type_name}();"));
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.dedent();
    w.line("}");
}

fn emit_map_tests(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&m.name);

    w.line("[Fact]");
    w.line(&format!("public void {type_name}_Roundtrip_Empty()"));
    w.line("{");
    w.indent();
    w.line(&format!("var original = new {type_name}();"));
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.line("Assert.Empty(decoded.Entries);");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("[Fact]");
    w.line(&format!("public void {type_name}_Roundtrip_Entry()"));
    w.line("{");
    w.indent();
    w.line(&format!("var original = new {type_name}();"));
    w.line(&format!(
        "original.Entries.Add({}, {});",
        cs_type_default(&m.key_ty, opts),
        cs_type_default(&m.val_ty, opts)
    ));
    if opts.is_json() {
        emit_roundtrip_assert_json(w, &type_name);
    } else {
        emit_roundtrip_assert(w, &type_name);
    }
    w.line("Assert.Single(decoded.Entries);");
    w.dedent();
    w.line("}");
}

/// Encode, decode, and check that the decoded value encodes to the same bytes.
fn emit_roundtrip_assert(w: &mut IndentWriter, type_name: &str) {
    w.line("var writer = new CborWriter();");
    w.line("original.Encode(writer);");
    w.line("var bytes = writer.Encode();");
    w.line("var reader = new CborReader(bytes);");
    w.line(&format!("var decoded = {type_name}.Decode(reader);"));
    w.line("Assert.Equal(0, reader.BytesRemaining);");
    w.line("var again = new CborWriter();");
    w.line("decoded.Encode(again);");
    w.line("Assert.Equal(Convert.ToHexString(bytes), Convert.ToHexString(again.Encode()));");
}

fn emit_roundtrip_assert_json(w: &mut IndentWriter, type_name: &str) {
    w.line("var bytes = original.ToJson();");
    w.line(&format!("var decoded = {type_name}.FromJson(bytes);"));
    w.line("Assert.Equal(System.Text.Encoding.UTF8.GetString(bytes), System.Text.Encoding.UTF8.GetString(decoded.ToJson()));");
}

// ── Test value helpers ────────────────────────────────────────────────────────

/// A repeated member's test value with as many items as it needs (`+`: one;
/// dgtlrift/cddlc#134).
fn cs_needed_items(f: &cddlc_ir::FieldDef, opts: &CodegenOptions) -> String {
    let min = cddlc_codegen::repeat_bounds(&f.occurrence).0;
    let item = cs_explicit_value(&f.ty, &Occurrence::Required, opts);
    let items: Vec<String> = (0..min).map(|_| item.clone()).collect();
    format!(
        "new List<{}> {{ {} }}",
        cs_typeref(&f.ty, opts),
        items.join(", ")
    )
}

fn cs_explicit_value(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::Optional => "null".into(),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => format!("new List<{}>() ", cs_typeref(ty, opts)),
        Occurrence::Required => match ty {
            TypeRef::Primitive(Primitive::Uint) => "42UL".into(),
            TypeRef::Primitive(Primitive::Int) => "-1L".into(),
            TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => "3.14f".into(),
            TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => "3.14".into(),
            TypeRef::Primitive(Primitive::Bool) => "true".into(),
            TypeRef::Primitive(Primitive::Tstr) => "\"hello\"".into(),
            TypeRef::Primitive(Primitive::Bstr) => "new byte[] { 0x01, 0x02 }".into(),
            // An item that isn't a byte string, `[1, "a"]` (dgtlrift/cddlc#178).
            TypeRef::Primitive(Primitive::Any) if opts.is_json() => {
                "JsonDocument.Parse(\"[1,\\\"a\\\"]\").RootElement".into()
            }
            TypeRef::Primitive(Primitive::Any) => "new byte[] { 0x82, 0x01, 0x61, 0x61 }".into(),
            TypeRef::Tagged(_, inner) => cs_explicit_value(inner, occ, opts),
            TypeRef::Choice(cs) if !cs.is_empty() => cs_explicit_value(&cs[0], occ, opts),
            _ => cs_type_default(ty, opts),
        },
    }
}

/// A value of `ty` that satisfies `constraints` (literal, size and range).
fn cs_valid_constrained_value(
    constraints: &[Constraint],
    ty: &TypeRef,
    opts: &CodegenOptions,
) -> String {
    let p = match cs_base(ty) {
        TypeRef::Primitive(p) => p.clone(),
        _ => return cs_type_default(ty, opts),
    };
    if let Some(lit) = constraints.iter().find_map(|c| match c {
        Constraint::Eq(LiteralValue::Bytes(_)) => None,
        Constraint::Eq(LiteralValue::Uint(n)) if matches!(p, Primitive::Int) => {
            Some(format!("{n}L"))
        }
        Constraint::Eq(l) => Some(cs_literal(l)),
        _ => None,
    }) {
        return lit;
    }
    // A `.regexp`: the shortest text it accepts.
    if matches!(p, Primitive::Tstr) {
        if let Some(t) = constraints.iter().find_map(|c| match c {
            Constraint::Regexp { pattern, .. } => cddlc_codegen::regexp_sample(pattern),
            _ => None,
        }) {
            return format!("{t:?}");
        }
    }
    // Sizes: the smallest allowed.
    let size = constraints.iter().find_map(|c| match c {
        Constraint::SizeExact(n) => Some(*n),
        Constraint::SizeRange { min, .. } => Some(min.unwrap_or(0)),
        _ => None,
    });
    match (&p, size) {
        (Primitive::Tstr, Some(n)) => return format!("new string('a', {n})"),
        (Primitive::Bstr, Some(n)) => return format!("new byte[{n}]"),
        _ => {}
    }
    // Ranges: the lower bound, or 0 when it's allowed, or the upper bound.
    for c in constraints {
        let (min, max, inclusive): (Option<i128>, Option<i128>, bool) = match c {
            Constraint::ValueRangeInt {
                min,
                max,
                inclusive,
            } => (min.map(i128::from), max.map(i128::from), *inclusive),
            Constraint::ValueRangeUint {
                min,
                max,
                inclusive,
            } => (min.map(i128::from), max.map(i128::from), *inclusive),
            _ => continue,
        };
        let v = match (min, max) {
            (Some(lo), _) => lo,
            (None, Some(hi)) if hi > 0 || (hi == 0 && inclusive) => 0,
            (None, Some(hi)) => {
                if inclusive {
                    hi
                } else {
                    hi - 1
                }
            }
            (None, None) => 0,
        };
        return match p {
            Primitive::Uint => format!("{}UL", v.max(0)),
            Primitive::Int => format!("{v}L"),
            Primitive::Float32 | Primitive::Float16 => format!("{v}.0f"),
            Primitive::Float64 | Primitive::Float => format!("{v}.0"),
            _ => cs_type_default(ty, opts),
        };
    }
    cs_type_default(ty, opts)
}

/// A value of `ty` that breaks one of `constraints`, if there's one to build.
fn cs_invalid_constrained_value(
    constraints: &[Constraint],
    ty: &TypeRef,
    _opts: &CodegenOptions,
) -> Option<String> {
    let TypeRef::Primitive(p) = cs_base(ty) else {
        return None;
    };
    let num = |v: i128| -> Option<String> {
        match p {
            Primitive::Uint if v >= 0 && v <= u64::MAX as i128 => Some(format!("{v}UL")),
            Primitive::Int if v >= i64::MIN as i128 && v <= i64::MAX as i128 => {
                Some(format!("{v}L"))
            }
            _ => None,
        }
    };
    let sized = |n: usize| -> Option<String> {
        match p {
            Primitive::Tstr => Some(format!("new string('a', {n})")),
            Primitive::Bstr => Some(format!("new byte[{n}]")),
            _ => None,
        }
    };
    constraints.iter().find_map(|c| match c {
        Constraint::SizeExact(n) => sized(n + 1),
        Constraint::SizeRange { min, max } => max
            .and_then(|hi| sized(hi + 1))
            .or_else(|| min.filter(|lo| *lo > 0).and_then(|lo| sized(lo - 1))),
        Constraint::ValueRangeInt {
            min,
            max,
            inclusive,
        } => {
            let (min, max) = (min.map(i128::from), max.map(i128::from));
            max.and_then(|hi| num(if *inclusive { hi + 1 } else { hi }))
                .or_else(|| min.and_then(|lo| num(lo - 1)))
        }
        Constraint::ValueRangeUint {
            min,
            max,
            inclusive,
        } => {
            let (min, max) = (min.map(i128::from), max.map(i128::from));
            max.and_then(|hi| num(if *inclusive { hi + 1 } else { hi }))
                .or_else(|| min.and_then(|lo| num(lo - 1)))
        }
        _ => None,
    })
}

fn emit_doc_comment(w: &mut IndentWriter, doc: Option<&str>) {
    cddlc_codegen::emit_doc_lines(w, doc, cddlc_codegen::DocStyle::CSharpXml);
}
