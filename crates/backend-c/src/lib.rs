/// C backend for cddlc.
///
/// Emits a C library crate:
///   <module>-cbor/
///     CMakeLists.txt
///     include/<module>.h      ← type declarations + callback API
///     src/<module>.c          ← encode/decode implementations
///     test/test_roundtrip.c   ← roundtrip tests (Unity framework)

use cddlc_codegen::{
    capacity_value, constraint_to_c_check, primitive_to_c,
    to_screaming_snake, to_snake_case,
    Backend, CodegenError, CodegenOptions, CodegenOutput, GeneratedFile,
    IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Capacity, Constraint, EnumDef,
    IrModule, MapDef, Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

pub struct CBackend;

impl Backend for CBackend {
    fn language_name(&self) -> &str { "C" }
    fn file_extension(&self) -> &str { "h" }

    fn generate(
        &self,
        module: &IrModule,
        opts:   &CodegenOptions,
    ) -> Result<CodegenOutput, CodegenError> {
        cddlc_codegen::check_rest_keys(module, opts, "C", true)?;
        if let Some(msg) = unsupported_recursion(module) {
            return Err(CodegenError::new(msg));
        }
        // A rule whose `<rule>_t` the C library or the runtime already has
        // (`mode_t`, `time_t`, `cbor_str_t`) is `<module>_<rule>_t`
        // (dgtlrift/cddlc#154).
        let renamed = cddlc_codegen::rename_types(module, &cddlc_codegen::c_reserved_renames(module));
        let module = &renamed;
        let name      = module.name.as_str();
        let fmt_tag   = if opts.is_json() { "json" } else { "cbor" };
        let crate_dir = format!("{name}-{fmt_tag}");

        if opts.is_json() {
            return Ok(CodegenOutput {
                files: vec![
                    GeneratedFile {
                        path:    format!("{crate_dir}/CMakeLists.txt"),
                        content: emit_cmake_json(name, opts),
                    },
                    GeneratedFile {
                        path:    format!("{crate_dir}/include/{name}.h"),
                        content: emit_header_json(module, opts),
                    },
                    GeneratedFile {
                        path:    format!("{crate_dir}/src/{name}.c"),
                        content: emit_source_json(module, opts),
                    },
                    GeneratedFile {
                        path:    format!("{crate_dir}/test/test_roundtrip.c"),
                        content: emit_tests_json(module, opts),
                    },
                ],
            });
        }

        Ok(CodegenOutput {
            files: vec![
                GeneratedFile {
                    path:    format!("{crate_dir}/CMakeLists.txt"),
                    content: emit_cmake(name, opts),
                },
                GeneratedFile {
                    path:    format!("{crate_dir}/include/{name}.h"),
                    content: emit_header(module, opts),
                },
                GeneratedFile {
                    path:    format!("{crate_dir}/compat/endian.h"),
                    content: nanocbor_macos_compat_h(opts),
                },
                GeneratedFile {
                    path:    format!("{crate_dir}/src/{name}.c"),
                    content: emit_source(module, opts),
                },
                GeneratedFile {
                    path:    format!("{crate_dir}/test/test_roundtrip.c"),
                    content: emit_tests(module, opts),
                },
            ],
        })
    }
}

// ── macOS compat header ───────────────────────────────────────────────────────

/// `compat/endian.h`: a stand-in for Linux's `<endian.h>` on macOS.
///
/// nanocbor `#include`s `<endian.h>` directly and uses `be64toh`, `htobe64`,
/// etc., which macOS does not have; upstream leaves supplying it to the
/// downstream project. This maps them to `<libkern/OSByteOrder.h>`. The
/// generated CMakeLists.txt adds `compat/` to the include path on Apple only.
fn nanocbor_macos_compat_h(opts: &CodegenOptions) -> String {
    cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::CBlock) + NANOCBOR_MACOS_COMPAT_H
}

const NANOCBOR_MACOS_COMPAT_H: &str = r#"/* macOS stand-in for Linux's <endian.h>, which nanocbor includes directly.
 * CMakeLists.txt puts this directory first on the include path on Apple only. */
#pragma once
#include <libkern/OSByteOrder.h>
#define be16toh(x) OSSwapBigToHostInt16(x)
#define be32toh(x) OSSwapBigToHostInt32(x)
#define be64toh(x) OSSwapBigToHostInt64(x)
#define htobe16(x) OSSwapHostToBigInt16(x)
#define htobe32(x) OSSwapHostToBigInt32(x)
#define htobe64(x) OSSwapHostToBigInt64(x)
#define le16toh(x) OSSwapLittleToHostInt16(x)
#define le32toh(x) OSSwapLittleToHostInt32(x)
#define le64toh(x) OSSwapLittleToHostInt64(x)
#define htole16(x) OSSwapHostToLittleInt16(x)
#define htole32(x) OSSwapHostToLittleInt32(x)
#define htole64(x) OSSwapHostToLittleInt64(x)
"#;

// ── CMakeLists.txt ────────────────────────────────────────────────────────────

fn emit_cmake(name: &str, opts: &CodegenOptions) -> String {
    let header = cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Hash);
    format!(r#"{header}cmake_minimum_required(VERSION 3.16)
project({name}_cbor C)

set(CMAKE_C_STANDARD 11)

# nanocbor includes <endian.h>, which macOS lacks: put the stand-in in compat/
# first on the include path. Set before FetchContent_MakeAvailable so nanocbor's
# own sources see it too.
if(APPLE)
    include_directories(BEFORE "${{CMAKE_CURRENT_SOURCE_DIR}}/compat")
endif()

include(FetchContent)
# Pinned: nanocbor master changed its byte-order setup without a release
# (dgtlrift/cddlc#20). Update deliberately.
FetchContent_Declare(nanocbor
    GIT_REPOSITORY https://github.com/bergzand/NanoCBOR.git
    GIT_TAG        a9eaf98fef10a0aefccd07e027d63f00e368054f
)
set(NANOCBOR_BUILD_TESTS OFF CACHE BOOL "" FORCE)
FetchContent_MakeAvailable(nanocbor)

file(GLOB NANOCBOR_SOURCES "${{nanocbor_SOURCE_DIR}}/src/*.c")

add_library({name}_cbor
    src/{name}.c
    ${{NANOCBOR_SOURCES}}
)
target_include_directories({name}_cbor PUBLIC include ${{nanocbor_SOURCE_DIR}}/include)

# Tests (requires Unity test framework)
enable_testing()
add_executable(test_roundtrip test/test_roundtrip.c)
target_link_libraries(test_roundtrip {name}_cbor)
add_test(NAME roundtrip COMMAND test_roundtrip)
"#)
}

// ── Header file ───────────────────────────────────────────────────────────────

fn emit_header(module: &IrModule, opts: &CodegenOptions) -> String {
    let name    = module.name.as_str();
    let guard   = format!("{}_H", to_screaming_snake(&name));
    let mut w   = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line(&format!("#ifndef {guard}"));
    w.line(&format!("#define {guard}"));
    w.blank();
    w.line("#include <stdint.h>");
    w.line("#include <stddef.h>");
    w.line("#include <stdbool.h>");
    w.line("#include <nanocbor/nanocbor.h>");
    w.blank();

    // ── Common types ──────────────────────────────────────────────────────────
    w.line("/* Zero-copy string slice (points into decode buffer) */");
    w.line("typedef struct { const char *ptr; size_t len; } cbor_str_t;");
    w.blank();
    w.line("/* Zero-copy byte slice */");
    w.line("typedef struct { const uint8_t *ptr; size_t len; } cbor_bytes_t;");
    w.line("typedef cbor_bytes_t cbor_slice_t;");
    w.line("/* A data item of any type (CDDL `any`): its CBOR encoding, in the decode");
    w.line(" * buffer. Encoding writes it back unchanged (dgtlrift/cddlc#178). */");
    w.line("typedef cbor_bytes_t cbor_any_t;");
    w.blank();
    w.line("/* Writes an encoded item as is, as nanocbor writes its own bytes. */");
    w.line("static inline int cddlc_put_item(nanocbor_encoder_t *enc, const uint8_t *item, size_t len)");
    w.line("{");
    w.line("    enc->len += len;");
    w.line("    if (!enc->fits(enc, enc->context, len)) return NANOCBOR_ERR_END;");
    w.line("    enc->append(enc, enc->context, item, len);");
    w.line("    return (int)len;");
    w.line("}");
    w.blank();

    // ── Error codes ───────────────────────────────────────────────────────────
    w.line("typedef enum {");
    w.indent();
    w.line(&format!("{}_OK = 0,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_TYPE,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_VALUE,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_MISSING,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_OVERFLOW,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_UTF8,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_UNKNOWN_FIELD,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_TAG,", to_screaming_snake(name)));
    w.line(&format!("{}_ERR_VALIDATION,", to_screaming_snake(name)));
    // A recursive member's memory couldn't be had (dgtlrift/cddlc#156).
    w.line(&format!("{}_ERR_ALLOC,", to_screaming_snake(name)));
    w.dedent();
    w.line(&format!("}} {}_err_t;", to_snake_case(name)));
    w.blank();

    // ── Field error ───────────────────────────────────────────────────────────
    w.line("typedef struct {");
    w.indent();
    w.line(&format!("{}_err_t  code;", to_snake_case(name)));
    w.line("const char     *field;");
    w.line("size_t          index;");
    w.dedent();
    w.line(&format!("}} {}_field_err_t;", to_snake_case(name)));
    w.blank();

    // ── Callbacks ─────────────────────────────────────────────────────────────
    w.line("/* Callbacks provided by the caller */");
    w.line("typedef struct {");
    w.indent();
    w.line(&format!(
        "void (*on_error)(void *ctx, {}_field_err_t err);",
        to_snake_case(name)
    ));
    w.line("void (*on_unknown_field)(void *ctx, cbor_str_t key);");
    // A recursive member (`tree = { ? left: tree }`) is a pointer: decoding
    // gets its memory here, and `<type>_release` gives it back
    // (dgtlrift/cddlc#156). Without `alloc`, decoding one fails.
    w.line("void *(*alloc)(void *ctx, size_t size);");
    w.line("void (*release)(void *ctx, void *ptr);");
    w.dedent();
    w.line(&format!("}} {}_callbacks_t;", to_snake_case(name)));
    w.blank();

    // ── Type declarations ─────────────────────────────────────────────────────
    // Types something points at are declared ahead (dgtlrift/cddlc#156).
    let targets = ptr_targets(module);
    for t in &targets {
        w.line(&format!("typedef struct {t}_s {t}_t;"));
    }
    if !targets.is_empty() { w.blank(); }
    for tname in cddlc_codegen::inline_decl_order(module) {
        let Some(def) = module.get(&tname) else { continue };
        let mut sub = IndentWriter::new(4);
        emit_type_decl_h(&mut sub, def, module, opts);
        for line in declare_tagged(sub.finish(), &to_snake_case(&tname), &targets).lines() {
            if line.is_empty() { w.blank() } else { w.line(line) }
        }
        w.blank();
    }

    // ── Function declarations ─────────────────────────────────────────────────
    w.line("/* Encode/decode function declarations */");
    for (_, def) in &module.types {
        emit_fn_decls(&mut w, def, name, opts);
        // What a recursive type's pointers point at is given back by its
        // `_release` (dgtlrift/cddlc#156).
        if needs_release(def.name(), module) {
            let (p, t) = (to_snake_case(name), to_snake_case(def.name()));
            w.line(&format!("void {p}_{t}_release({t}_t *val, const {p}_callbacks_t *cb, void *ctx);"));
        }
    }
    w.blank();

    w.line(&format!("#endif /* {guard} */"));
    w.finish()
}

fn emit_type_decl_h(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct_decl(w, s, module, opts),
        TypeDef::Enum(e)   => emit_enum_decl(w, e, module, opts),
        TypeDef::Array(a)  => emit_array_decl(w, a, opts),
        TypeDef::Alias(a)  => emit_alias_decl(w, a, opts),
        TypeDef::Map(m)    => emit_map_decl(w, m, opts),
    }
}

fn emit_struct_decl(w: &mut IndentWriter, s: &StructDef, module: &IrModule, opts: &CodegenOptions) {
    emit_c_doc(w, s.doc.as_deref());
    let type_name = to_snake_case(&s.name);
    w.line(&format!("typedef struct {{"));
    w.indent();
    for f in &s.fields {
        emit_c_doc(w, f.doc.as_deref());
        // A member that holds this type again is a pointer (dgtlrift/cddlc#156).
        let ty_str = if indirect(&s.name, &f.ty, module) {
            format!("{} *", ptr_type(&f.ty, module))
        } else {
            c_field_type(&f.ty, &f.occurrence, opts)
        };
        let fname  = to_snake_case(&f.name);
        if ty_str.contains("{name}") {
            w.line(&format!("{};", ty_str.replace("{name}", &fname)));
        } else {
            w.line(&format!("{ty_str} {fname};"));
        }
        // bool present flag for optional fields
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("bool {fname}_present;"));
        }
    }
    // The map's other entries, up to their capacity (dgtlrift/cddlc#82).
    if let Some(r) = &s.rest {
        let cap = capacity_value(
            r.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
            opts.max_array,
        );
        let rname = to_snake_case(&r.name);
        w.line("/* The map's other entries. */");
        // Text keys, or integers or a choice of them (dgtlrift/cddlc#136).
        let key = if text_rest(r) { "cbor_str_t".to_owned() } else { typeref_to_c(&r.key_ty, opts) };
        w.line(&format!("struct {{ {key} key; {} value; }} {rname}[{cap}];", typeref_to_c(&r.val_ty, opts)));
        w.line(&format!("size_t {rname}_count;"));
    }
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_enum_decl(w: &mut IndentWriter, e: &EnumDef, module: &IrModule, _opts: &CodegenOptions) {
    emit_c_doc(w, e.doc.as_deref());
    let type_name = to_snake_case(&e.name);

    // Tag enum
    w.line("typedef enum {");
    w.indent();
    for (i, v) in e.variants.iter().enumerate() {
        let vname = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
        if i == e.variants.len() - 1 {
            w.line(&format!("{vname}"));
        } else {
            w.line(&format!("{vname},"));
        }
    }
    w.dedent();
    w.line(&format!("}} {type_name}_tag_t;"));
    w.blank();

    // Union struct
    w.line(&format!("typedef struct {{"));
    w.indent();
    w.line(&format!("{type_name}_tag_t tag;"));
    w.line("union {");
    w.indent();
    for v in &e.variants {
        let vname = to_snake_case(&v.name);
        match &v.ty {
            TypeRef::Primitive(Primitive::Null) => {}
            TypeRef::Primitive(p) => {
                w.line(&format!("{} {};", primitive_to_c(p), vname));
            }
            // An alternative that holds this type again is a pointer
            // (dgtlrift/cddlc#156).
            TypeRef::Named(_) if indirect(&e.name, &v.ty, module) => {
                w.line(&format!("{} *{};", ptr_type(&v.ty, module), vname));
            }
            TypeRef::Named(n) => {
                w.line(&format!("{}_t {};", to_snake_case(n), vname));
            }
            _ => {}
        }
    }
    w.dedent();
    w.line("} value;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_array_decl(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    emit_c_doc(w, a.doc.as_deref());
    let type_name = to_snake_case(&a.name);
    let elem_ty   = typeref_to_c(&a.element, opts);
    let cap       = capacity_value(
        a.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line(&format!("typedef struct {{"));
    w.indent();
    w.line(&format!("{elem_ty} items[{cap}];"));
    w.line("size_t count;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

/// A table (`{ * tstr => uint }`): up to its capacity of key/value entries
/// (dgtlrift/cddlc#82).
fn emit_map_decl(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    emit_c_doc(w, m.doc.as_deref());
    let type_name = to_snake_case(&m.name);
    let cap = capacity_value(
        m.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line("typedef struct {");
    w.indent();
    w.line(&format!(
        "struct {{ {} key; {} value; }} entries[{cap}];",
        typeref_to_c(&m.key_ty, opts),
        typeref_to_c(&m.val_ty, opts)
    ));
    w.line("size_t count;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_alias_decl(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    emit_c_doc(w, a.doc.as_deref());
    let type_name = to_snake_case(&a.name);
    if a.tagged.is_some() || !a.constraints.is_empty() {
        // Newtype struct for tagged/constrained aliases
        let inner = typeref_to_c(&a.ty, opts);
        w.line(&format!("typedef struct {{ {inner} value; }} {type_name}_t;"));
    } else {
        let inner = typeref_to_c(&a.ty, opts);
        w.line(&format!("typedef {inner} {type_name}_t;"));
    }
}

fn emit_fn_decls(w: &mut IndentWriter, def: &TypeDef, mod_name: &str, _opts: &CodegenOptions) {
    let type_name = to_snake_case(def.name());
    let pfx       = to_snake_case(mod_name);

    w.line(&format!(
        "int {pfx}_{type_name}_encode(nanocbor_encoder_t *enc, const {type_name}_t *val);"
    ));
    w.line(&format!(
        "int {pfx}_{type_name}_decode(nanocbor_value_t *dec, {type_name}_t *out, \
         const {pfx}_callbacks_t *cb, void *ctx);"
    ));
}

// ── Source file ───────────────────────────────────────────────────────────────

fn emit_source(module: &IrModule, opts: &CodegenOptions) -> String {
    let name  = module.name.as_str();
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.h\""));
    w.line("#include <string.h>");
    w.blank();
    if cddlc_codegen::module_uses_bits(module) {
        w.line("/* `.bits`: only the bits `m` has set may be set (bit n is bit n % 8 of byte n / 8). */");
        w.line("static bool cddlc_bits_ok(const uint8_t *p, size_t n, const uint8_t *m, size_t mn) {");
        w.indent();
        w.line("size_t i;");
        w.line("for (i = 0; i < n; i++) {");
        w.indent();
        w.line("if (p[i] & (uint8_t)~(i < mn ? m[i] : 0)) return false;");
        w.dedent();
        w.line("}");
        w.line("return true;");
        w.dedent();
        w.line("}");
        w.blank();
    }

    for (_, def) in &module.types {
        w.blank();
        emit_encode_fn(&mut w, def, module, name, opts);
        w.blank();
        emit_decode_fn(&mut w, def, module, name, opts);
        w.blank();
        emit_release_fn(&mut w, def, module, &to_snake_case(name), false);
    }

    w.finish()
}

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

/// Fails (returns -1) unless a repeated member's item count, `n`, is within
/// its bounds (dgtlrift/cddlc#134).
fn emit_c_repeat_check(w: &mut IndentWriter, f: &cddlc_ir::FieldDef, n: &str) {
    let (min, max) = cddlc_codegen::repeat_bounds(&f.occurrence);
    if min > 0 {
        w.line(&format!("if ({n} < {min}) return -1; /* too few '{}' items */", f.name));
    }
    if let Some(max) = max {
        w.line(&format!("if ({n} > {max}) return -1; /* too many '{}' items */", f.name));
    }
}

/// Whether the byte string `val_expr` (a `cbor_slice_t`) equals `b`.
fn c_bytes_eq(val_expr: &str, b: &[u8]) -> String {
    if b.is_empty() {
        return format!("({val_expr}.len == 0)");
    }
    let bytes = b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ");
    format!("({val_expr}.len == {n} && memcmp({val_expr}.ptr, (const uint8_t[]){{{bytes}}}, {n}) == 0)", n = b.len())
}

/// Whether an enum variant is a text literal (`"on"`), not any text (`tstr`).
fn is_text_literal(v: &cddlc_ir::EnumVariant) -> bool {
    v.constraints.iter().any(|c| matches!(c, Constraint::Eq(cddlc_ir::LiteralValue::Text(_))))
}

// ── Recursion (dgtlrift/cddlc#156) ───────────────────────────────────────────

/// Whether a member or variant of `from` holding `ty` is a pointer: `ty`
/// holds `from` again, inline.
fn indirect(from: &str, ty: &TypeRef, module: &IrModule) -> bool {
    cddlc_codegen::is_indirect(from, ty, module)
}

/// The named type `ty` holds, under any tag. A choice that isn't of
/// primitives is held as its first alternative (see `typeref_to_c`).
fn named_of(ty: &TypeRef) -> Option<&str> {
    match ty {
        TypeRef::Named(n) => Some(n.as_str()),
        TypeRef::Tagged(_, inner) => named_of(inner),
        TypeRef::Choice(cs) if prim_choice_c(cs).is_none() => cs.first().and_then(named_of),
        _ => None,
    }
}

/// The type an alias chain ends at.
fn resolve_named<'a>(n: &'a str, module: &'a IrModule) -> &'a str {
    let mut n = n;
    for _ in 0..16 {
        match module.get(n) {
            Some(TypeDef::Alias(a)) => match &a.ty {
                TypeRef::Named(t) => n = t.as_str(),
                _ => break,
            },
            _ => break,
        }
    }
    n
}

/// The C type a pointer member or variant points at: the type its alias
/// chain ends at, declared ahead (`typedef struct <t>_s <t>_t;`).
fn ptr_type(ty: &TypeRef, module: &IrModule) -> String {
    let n = named_of(ty).map(|n| resolve_named(n, module)).unwrap_or("void");
    format!("{}_t", to_snake_case(n))
}

/// The types something points at: declared ahead, so a pointer to one comes
/// before its definition.
fn ptr_targets(module: &IrModule) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (name, def) in &module.types {
        let tys: Vec<&TypeRef> = match def {
            TypeDef::Struct(s) => s.fields.iter()
                .filter(|f| matches!(f.occurrence, Occurrence::Required | Occurrence::Optional))
                .map(|f| &f.ty).collect(),
            TypeDef::Enum(e) => e.variants.iter().map(|v| &v.ty).collect(),
            _ => vec![],
        };
        for ty in tys {
            if indirect(name, ty, module) {
                if let Some(n) = named_of(ty) {
                    out.insert(to_snake_case(resolve_named(n, module)));
                }
            }
        }
    }
    out
}

/// Whether a type holds a pointer, itself or in what it holds inline: it has
/// a `<t>_release` that gives the pointed-at memory back.
fn needs_release(name: &str, module: &IrModule) -> bool {
    fn go(name: &str, module: &IrModule, seen: &mut std::collections::HashSet<String>) -> bool {
        if !seen.insert(name.to_owned()) {
            return false;
        }
        let refs = |ty: &TypeRef| -> Vec<String> { named_of(ty).map(|n| vec![n.to_owned()]).unwrap_or_default() };
        match module.get(name) {
            Some(TypeDef::Struct(s)) => {
                s.fields.iter().any(|f| indirect(name, &f.ty, module))
                    || s.fields.iter().flat_map(|f| refs(&f.ty)).any(|n| go(&n, module, seen))
                    || s.rest.iter().flat_map(|r| refs(&r.val_ty)).any(|n| go(&n, module, seen))
            }
            Some(TypeDef::Enum(e)) => {
                e.variants.iter().any(|v| indirect(name, &v.ty, module))
                    || e.variants.iter().flat_map(|v| refs(&v.ty)).any(|n| go(&n, module, seen))
            }
            Some(TypeDef::Array(a)) => refs(&a.element).iter().any(|n| go(n, module, seen)),
            Some(TypeDef::Map(m)) => refs(&m.val_ty).iter().any(|n| go(n, module, seen)),
            Some(TypeDef::Alias(a)) => refs(&a.ty).iter().any(|n| go(n, module, seen)),
            None => false,
        }
    }
    go(name, module, &mut std::collections::HashSet::new())
}

/// A recursive member C can't hold yet: through a repeated member or other
/// entries (an array of them in place), or of a type that isn't named.
fn unsupported_recursion(module: &IrModule) -> Option<String> {
    for (name, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                for f in &s.fields {
                    if !indirect(name, &f.ty, module) {
                        continue;
                    }
                    if f.occurrence.is_repeated() {
                        return Some(format!("'{name}.{}': C can't hold a repeated member that holds '{name}' again yet; use an array type for it (dgtlrift/cddlc#156)", f.name));
                    }
                    if named_of(&f.ty).is_none() {
                        return Some(format!("'{name}.{}': C can't hold a recursive member of an inline choice yet (dgtlrift/cddlc#156)", f.name));
                    }
                }
                if let Some(r) = &s.rest {
                    if indirect(name, &r.val_ty, module) {
                        return Some(format!("'{name}': C can't hold other entries whose value holds '{name}' again yet (dgtlrift/cddlc#156)"));
                    }
                }
            }
            TypeDef::Enum(e) => {
                for v in &e.variants {
                    if indirect(name, &v.ty, module) && named_of(&v.ty).is_none() {
                        return Some(format!("'{name}': C can't hold a recursive alternative that isn't a named type yet (dgtlrift/cddlc#156)"));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// A type's declaration, tagged when something points at it: `struct <t>_s {
/// ... };`, its `typedef` declared ahead.
fn declare_tagged(decl: String, snake: &str, targets: &std::collections::BTreeSet<String>) -> String {
    if !targets.contains(snake) {
        return decl;
    }
    let open = "typedef struct {";
    let close = format!("}} {snake}_t;");
    match (decl.rfind(open), decl.rfind(&close)) {
        (Some(o), Some(c)) if o < c => format!(
            "{}struct {snake}_s {{{}}};{}",
            &decl[..o], &decl[o + open.len()..c], &decl[c + close.len()..]
        ),
        _ => decl,
    }
}

/// `<t>_release`, for a type that holds a pointer: gives back the memory its
/// pointer members point at, and theirs, through `cb->release` (or `free` in
/// JSON, where decoding used `calloc`), and empties them (dgtlrift/cddlc#156).
fn emit_release_fn(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, pfx: &str, json: bool) {
    let name = def.name();
    if !needs_release(name, module) {
        return;
    }
    let t = to_snake_case(name);
    let sig = if json {
        format!("void {pfx}_{t}_release({t}_t *val)")
    } else {
        format!("void {pfx}_{t}_release({t}_t *val, const {pfx}_callbacks_t *cb, void *ctx)")
    };
    let args = if json { "" } else { ", cb, ctx" };
    let free_ptr = |p: &str| if json { format!("free({p});") } else { format!("if (cb && cb->release) cb->release(ctx, {p});") };
    // Releases what `expr` (a value of `ty`) holds, through a pointer or not.
    let release = |w: &mut IndentWriter, ty: &TypeRef, expr: &str, ptr: bool| {
        let Some(n) = named_of(ty) else { return };
        let target = if ptr { resolve_named(n, module) } else { n };
        let inner = needs_release(target, module);
        if ptr {
            w.line(&format!("if ({expr}) {{"));
            w.indent();
            if inner {
                w.line(&format!("{pfx}_{}_release({expr}{args});", to_snake_case(target)));
            }
            w.line(&free_ptr(expr));
            w.line(&format!("{expr} = NULL;"));
            w.dedent();
            w.line("}");
        } else if inner {
            w.line(&format!("{pfx}_{}_release(&{expr}{args});", to_snake_case(target)));
        }
    };
    w.line(&format!("{sig} {{"));
    w.indent();
    if !json { w.line("(void)cb; (void)ctx;"); }
    w.line("if (!val) return;");
    match def {
        TypeDef::Struct(st) => {
            for f in &st.fields {
                let fname = to_snake_case(&f.name);
                if indirect(&st.name, &f.ty, module) {
                    release(w, &f.ty, &format!("val->{fname}"), true);
                } else if f.occurrence.is_repeated() {
                    if named_of(&f.ty).is_some_and(|n| needs_release(n, module)) {
                        w.line(&format!("for (size_t i = 0; i < val->{fname}_count; i++) {{"));
                        w.indent();
                        release(w, &f.ty, &format!("val->{fname}[i]"), false);
                        w.dedent();
                        w.line("}");
                    }
                } else if matches!(f.occurrence, Occurrence::Optional) {
                    if named_of(&f.ty).is_some_and(|n| needs_release(n, module)) {
                        w.line(&format!("if (val->{fname}_present) {{"));
                        w.indent();
                        release(w, &f.ty, &format!("val->{fname}"), false);
                        w.dedent();
                        w.line("}");
                    }
                } else {
                    release(w, &f.ty, &format!("val->{fname}"), false);
                }
            }
            if let Some(r) = &st.rest {
                if named_of(&r.val_ty).is_some_and(|n| needs_release(n, module)) {
                    let rname = to_snake_case(&r.name);
                    w.line(&format!("for (size_t i = 0; i < val->{rname}_count; i++) {{"));
                    w.indent();
                    release(w, &r.val_ty, &format!("val->{rname}[i].value"), false);
                    w.dedent();
                    w.line("}");
                }
            }
        }
        TypeDef::Enum(e) => {
            w.line("switch (val->tag) {");
            for v in &e.variants {
                if named_of(&v.ty).is_none() { continue; }
                let vname = to_snake_case(&v.name);
                let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                w.line(&format!("case {vconst}:"));
                w.indent();
                release(w, &v.ty, &format!("val->value.{vname}"), indirect(&e.name, &v.ty, module));
                w.line("break;");
                w.dedent();
            }
            w.line("default: break;");
            w.line("}");
        }
        TypeDef::Array(a) => {
            w.line("for (size_t i = 0; i < val->count; i++) {");
            w.indent();
            release(w, &a.element, "val->items[i]", false);
            w.dedent();
            w.line("}");
        }
        TypeDef::Map(m) => {
            w.line("for (size_t i = 0; i < val->count; i++) {");
            w.indent();
            release(w, &m.val_ty, "val->entries[i].value", false);
            w.dedent();
            w.line("}");
        }
        TypeDef::Alias(a) => {
            if a.tagged.is_some() || !a.constraints.is_empty() {
                release(w, &a.ty, "val->value", false);
            } else if let Some(n) = named_of(&a.ty) {
                w.line(&format!("{pfx}_{}_release(val{args});", to_snake_case(n)));
            }
        }
    }
    w.dedent();
    w.line("}");
    w.blank();
}

/// Where a member decodes to: the member, or for a pointer member, memory
/// from the caller's `alloc`, which it then points at (dgtlrift/cddlc#156).
fn c_decode_dest(w: &mut IndentWriter, s: &StructDef, f: &cddlc_ir::FieldDef, module: &IrModule, pfx: &str, pfx_upper: &str) -> String {
    let fname = to_snake_case(&f.name);
    if !indirect(&s.name, &f.ty, module) {
        return format!("out->{fname}");
    }
    let ptr = ptr_type(&f.ty, module);
    w.line(&format!(
        "out->{fname} = (cb && cb->alloc) ? ({ptr} *)cb->alloc(ctx, sizeof *out->{fname}) : NULL;"
    ));
    w.line(&format!(
        "if (!out->{fname}) {{ if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_ALLOC, {:?}, 0}}); return -1; }}",
        f.name.as_str()
    ));
    w.line(&format!("memset(out->{fname}, 0, sizeof *out->{fname});"));
    format!("(*out->{fname})")
}

/// Whether a map's other entries have plain `tstr` keys, held as `cbor_str_t`
/// (otherwise they hold their key type; dgtlrift/cddlc#136).
fn text_rest(r: &cddlc_ir::MapRest) -> bool {
    matches!(r.key_ty, TypeRef::Primitive(Primitive::Tstr))
}

/// Writes a map member's key: its text, or an integer key as an integer
/// (dgtlrift/cddlc#176).
fn c_key_write(key: &cddlc_ir::FieldKey) -> String {
    match key {
        cddlc_ir::FieldKey::Text(t) => format!("nanocbor_put_tstr(enc, {:?});", t.as_str()),
        cddlc_ir::FieldKey::Uint(n) => format!("nanocbor_fmt_uint(enc, {n}ULL);"),
        cddlc_ir::FieldKey::Int(n) if *n >= 0 => format!("nanocbor_fmt_uint(enc, {n}ULL);"),
        cddlc_ir::FieldKey::Int(n) => format!("nanocbor_fmt_int(enc, {n}LL);"),
    }
}

fn emit_encode_fn(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    let type_name = to_snake_case(def.name());
    let pfx       = to_snake_case(mod_name);

    w.line(&format!(
        "int {pfx}_{type_name}_encode(nanocbor_encoder_t *enc, const {type_name}_t *val) {{"
    ));
    w.indent();

    match def {
        TypeDef::Struct(s) => {
            // Emit tag if present
            if let Some(tag) = s.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            // dCBOR: sort keys — for now emit in definition order
            let fields: Vec<_> = if opts.dcbor {
                let mut fs: Vec<_> = s.fields.iter().collect();
                fs.sort_by(|a, b| cddlc_codegen::dcbor_key_order(a, b));
                fs
            } else {
                s.fields.iter().collect()
            };
            // Compute map count: required fields always present; optional only when _present flag set
            let req_count = fields.iter().filter(|f| !matches!(f.occurrence, Occurrence::Optional)).count();
            let opt_fields: Vec<_> = fields.iter().filter(|f| matches!(f.occurrence, Occurrence::Optional)).collect();
            // A record (RFC 8610 §3.4) is an array of the members' values, in
            // order; optional members come last, so one present after an absent
            // one can't be written.
            let record = s.encoding == cddlc_ir::StructEncoding::Array;
            if record {
                for pair in opt_fields.windows(2) {
                    w.line(&format!(
                        "if (val->{}_present && !val->{}_present) return -1;",
                        to_snake_case(&pair[1].name), to_snake_case(&pair[0].name)
                    ));
                }
                let opt_sum: String = opt_fields.iter()
                    .map(|f| format!(" + (val->{}_present ? 1u : 0u)", to_snake_case(&f.name)))
                    .collect();
                // A repeated member's items go inline (dgtlrift/cddlc#134).
                match cddlc_codegen::record_repeated(s) {
                    Some(ri) => {
                        let rf = &s.fields[ri];
                        let rname = to_snake_case(&rf.name);
                        emit_c_repeat_check(w, rf, &format!("val->{rname}_count"));
                        w.line(&format!("nanocbor_fmt_array(enc, {}u + val->{rname}_count);", req_count - 1));
                    }
                    None => w.line(&format!("nanocbor_fmt_array(enc, {req_count}u{opt_sum});")),
                }
            } else {
                let rest_count = s.rest.as_ref()
                    .map(|r| format!(" + val->{}_count", to_snake_case(&r.name)))
                    .unwrap_or_default();
                let opt_sum: String = opt_fields.iter().map(|f| {
                    let fname = to_snake_case(&f.name);
                    format!(" + (val->{fname}_present ? 1u : 0u)")
                }).collect();
                w.line(&format!("nanocbor_fmt_map(enc, {req_count}u{opt_sum}{rest_count});"));
            }
            for f in &fields {
                let fname = to_snake_case(&f.name);
                // A pointer member is encoded through it (dgtlrift/cddlc#156).
                let fexpr = if indirect(&s.name, &f.ty, module) { format!("(*val->{fname})") } else { format!("val->{fname}") };
                // A required pointer member must point at a value.
                if indirect(&s.name, &f.ty, module) && f.occurrence.is_required() {
                    w.line(&format!("if (!val->{fname}) return -1; /* '{}' is required */", f.name));
                }
                if matches!(f.occurrence, Occurrence::Optional) {
                    w.line(&format!("if (val->{fname}_present) {{"));
                    w.indent();
                    if !record { w.line(&c_key_write(&f.key)); }
                    emit_c_encode_field(w, &f.ty, &fexpr, &f.occurrence, mod_name, opts);
                    w.dedent();
                    w.line("}");
                } else if record && f.occurrence.is_repeated() {
                    w.line(&format!("for (size_t i = 0; i < val->{fname}_count; i++) {{"));
                    w.indent();
                    emit_c_encode_field(w, &f.ty, &format!("val->{fname}[i]"), &Occurrence::Required, mod_name, opts);
                    w.dedent();
                    w.line("}");
                } else {
                    if !record { w.line(&c_key_write(&f.key)); }
                    emit_c_encode_field(w, &f.ty, &fexpr, &f.occurrence, mod_name, opts);
                }
            }
            if let Some(r) = &s.rest {
                let rname = to_snake_case(&r.name);
                w.line(&format!("for (size_t i = 0; i < val->{rname}_count; i++) {{"));
                w.indent();
                if text_rest(r) {
                    w.line(&format!("nanocbor_put_tstrn(enc, val->{rname}[i].key.ptr, val->{rname}[i].key.len);"));
                } else {
                    emit_c_encode_field(w, &r.key_ty, &format!("val->{rname}[i].key"), &Occurrence::Required, mod_name, opts);
                }
                emit_c_encode_field(w, &r.val_ty, &format!("val->{rname}[i].value"), &Occurrence::Required, mod_name, opts);
                w.dedent();
                w.line("}");
            }
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Enum(e) => {
            if let Some(tag) = e.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            w.line(&format!("switch (val->tag) {{"));
            for v in &e.variants {
                let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                let vname  = to_snake_case(&v.name);
                w.line(&format!("case {vconst}:"));
                w.indent();
                match &v.ty {
                    // A text literal is written as itself; a `tstr` alternative
                    // holds its text.
                    TypeRef::Primitive(Primitive::Tstr) if is_text_literal(v) => {
                        let s = cddlc_codegen::enum_variant_text(v);
                        w.line(&format!("nanocbor_put_tstr(enc, {:?});", s));
                    }
                    TypeRef::Primitive(Primitive::Tstr) => w.line(&format!(
                        "nanocbor_put_tstrn(enc, val->value.{vname}.ptr, val->value.{vname}.len);"
                    )),
                    TypeRef::Primitive(Primitive::Uint) =>
                        w.line(&format!("nanocbor_fmt_uint(enc, val->value.{vname});")),
                    TypeRef::Primitive(Primitive::Int) =>
                        w.line(&format!("nanocbor_fmt_int(enc, val->value.{vname});")),
                    TypeRef::Primitive(Primitive::Bool) =>
                        w.line(&format!("nanocbor_fmt_bool(enc, val->value.{vname});")),
                    TypeRef::Primitive(Primitive::Null) =>
                        w.line("nanocbor_fmt_null(enc);"),
                    // The encoded item, written back unchanged (dgtlrift/cddlc#178).
                    TypeRef::Primitive(Primitive::Any) => w.line(&format!(
                        "if (cddlc_put_item(enc, val->value.{vname}.ptr, val->value.{vname}.len) < 0) return -1;"
                    )),
                    TypeRef::Named(n) => {
                        let np = to_snake_case(mod_name);
                        let tn = to_snake_case(n);
                        // A pointer alternative is encoded through it
                        // (dgtlrift/cddlc#156).
                        if indirect(&e.name, &v.ty, module) {
                            w.line(&format!("{np}_{tn}_encode(enc, val->value.{vname});"));
                        } else {
                            w.line(&format!("{np}_{tn}_encode(enc, &val->value.{vname});"));
                        }
                    }
                    _ => w.line("nanocbor_fmt_null(enc);"),
                }
                w.line("break;");
                w.dedent();
            }
            w.line("}");
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Array(a) => {
            if let Some(tag) = a.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            // A repeated group's members sit inline (dgtlrift/cddlc#90).
            if let Some(rec) = flat_record(a, module) {
                w.line(&format!("nanocbor_fmt_array(enc, val->count * {});", rec.fields.len()));
                w.line("for (size_t i = 0; i < val->count; i++) {");
                w.indent();
                for f in &rec.fields {
                    let item = format!("val->items[i].{}", to_snake_case(&f.name));
                    emit_c_encode_field(w, &f.ty, &item, &Occurrence::Required, mod_name, opts);
                }
                w.dedent();
                w.line("}");
                w.line("return NANOCBOR_OK;");
                w.dedent();
                w.line("}");
                return;
            }
            w.line("nanocbor_fmt_array(enc, val->count);");
            w.line("for (size_t i = 0; i < val->count; i++) {");
            w.indent();
            emit_c_encode_field(w, &a.element, "val->items[i]", &Occurrence::Required, mod_name, opts);
            w.dedent();
            w.line("}");
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Alias(a) => {
            if let Some(tag) = a.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            let inner_expr = if a.tagged.is_some() || !a.constraints.is_empty() {
                "val->value".to_string()
            } else {
                "(*val)".to_string()
            };
            emit_c_encode_field(w, &a.ty, &inner_expr, &Occurrence::Required, mod_name, opts);
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Map(m) => {
            if let Some(tag) = m.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            w.line("nanocbor_fmt_map(enc, val->count);");
            w.line("for (size_t i = 0; i < val->count; i++) {");
            w.indent();
            emit_c_encode_field(w, &m.key_ty, "val->entries[i].key", &Occurrence::Required, mod_name, opts);
            emit_c_encode_field(w, &m.val_ty, "val->entries[i].value", &Occurrence::Required, mod_name, opts);
            w.dedent();
            w.line("}");
            w.line("return NANOCBOR_OK;");
        }
    }

    w.dedent();
    w.line("}");
}

fn emit_c_encode_field(
    w:        &mut IndentWriter,
    ty:       &TypeRef,
    expr:     &str,
    occ:      &Occurrence,
    mod_name: &str,
    opts:     &CodegenOptions,
) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = c_encode_call(p, expr);
            w.line(&format!("{call};"));
        }
        TypeRef::Named(n) => {
            let pfx = to_snake_case(mod_name);
            let tn  = to_snake_case(n);
            w.line(&format!("{pfx}_{tn}_encode(enc, &{expr});"));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            emit_c_encode_field(w, inner, expr, occ, mod_name, opts);
        }
        TypeRef::Choice(cs) => match prim_choice_c(cs) {
            // A tagged union: the alternative `kind` names.
            Some(alts) => {
                w.line(&format!("switch ({expr}.kind) {{"));
                for (i, p) in alts.iter().enumerate() {
                    w.line(&format!("case {i}: {}; break;", c_encode_call(p, &format!("{expr}.as.alt{i}"))));
                }
                w.line("default: return -1;");
                w.line("}");
            }
            None => w.line("/* choice encode */"),
        },
    }
}

/// A choice of primitives with distinct C types (`int / float`), which is a
/// tagged union `{ uint8_t kind; union { alt0; alt1; ... } as; }`. `None` for
/// any other choice, which keeps its first alternative's type.
fn prim_choice_c(cs: &[TypeRef]) -> Option<Vec<Primitive>> {
    let mut alts: Vec<Primitive> = Vec::new();
    for c in cs {
        let TypeRef::Primitive(p) = c else { return None };
        if matches!(p, Primitive::Any) { return None; }
        if alts.iter().any(|q| primitive_to_c(q) == primitive_to_c(p)) { return None; }
        alts.push(p.clone());
    }
    (alts.len() > 1).then_some(alts)
}

fn c_encode_call(p: &Primitive, expr: &str) -> String {
    match p {
        Primitive::Bool               => format!("nanocbor_fmt_bool(enc, {expr})"),
        Primitive::Null | Primitive::Undefined => "nanocbor_fmt_null(enc)".into(),
        Primitive::Uint               => format!("nanocbor_fmt_uint(enc, {expr})"),
        Primitive::Int                => format!("nanocbor_fmt_int(enc, {expr})"),
        Primitive::Float32            => format!("nanocbor_fmt_float(enc, {expr})"),
        Primitive::Float64 | Primitive::Float => format!("nanocbor_fmt_double(enc, {expr})"),
        Primitive::Float16            => format!("nanocbor_fmt_float(enc, {expr})"),
        Primitive::Bstr =>
            format!("nanocbor_put_bstr(enc, {expr}.ptr, {expr}.len)"),
        // The encoded item, written back unchanged (dgtlrift/cddlc#178).
        Primitive::Any =>
            format!("cddlc_put_item(enc, {expr}.ptr, {expr}.len)"),
        Primitive::Tstr               =>
            format!("nanocbor_put_tstrn(enc, {expr}.ptr, {expr}.len)"),
    }
}

fn emit_decode_fn(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    let type_name = to_snake_case(def.name());
    let pfx       = to_snake_case(mod_name);
    let pfx_upper = to_screaming_snake(mod_name);

    w.line(&format!(
        "int {pfx}_{type_name}_decode(nanocbor_value_t *dec, {type_name}_t *out, \
         const {pfx}_callbacks_t *cb, void *ctx) {{"
    ));
    w.indent();
    w.line("int rc;");
    w.line(&format!("(void)cb; (void)ctx;"));

    match def {
        TypeDef::Struct(s) => {
            // Tag verification
            if let Some(tag) = s.tagged {
                w.line("{");
                w.indent();
                w.line("uint32_t tag;");
                w.line("if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("if (tag != {}) {{", tag.0));
                w.indent();
                w.line(&format!(
                    "if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_TAG, \"{type_name}\", 0}});"
                ));
                w.line("return -1;");
                w.dedent();
                w.line("}");
                w.dedent();
                w.line("}");
            }

            if s.encoding == cddlc_ir::StructEncoding::Array {
                // A record: the members' values in order; optional ones last.
                w.line("nanocbor_value_t arr;");
                w.line("if (nanocbor_enter_array(dec, &arr) < 0) return -1;");
                w.line(&format!("memset(out, 0, sizeof({type_name}_t));"));
                // One optional member before required ones: present when the
                // array holds every member, so count the items on a copy
                // (dgtlrift/cddlc#134).
                let before = cddlc_codegen::record_optionals_before(s);
                let repeated = cddlc_codegen::record_repeated(s);
                if before.is_some() || repeated.is_some() {
                    w.line("size_t count = 0;");
                    w.line("{ nanocbor_value_t probe = arr; while (!nanocbor_at_end(&probe)) { if (nanocbor_skip(&probe) < 0) return -1; count++; } }");
                }
                let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();
                // A repeated member holds the items the others leave.
                if let Some(ri) = repeated {
                    let rf = &s.fields[ri];
                    let cap = capacity_value(rf.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)), opts.max_array);
                    w.line(&format!("if (count < {required}) return -1; /* too few items */"));
                    w.line(&format!("size_t reps = count - {required};"));
                    w.line(&format!("if (reps > {cap}) return -1; /* more items than its capacity */"));
                    emit_c_repeat_check(w, rf, "reps");
                }
                for (i, f) in s.fields.iter().enumerate() {
                    let fname = to_snake_case(&f.name);
                    if f.occurrence.is_repeated() {
                        w.line("for (size_t i = 0; i < reps; i++) {");
                        w.indent();
                        emit_c_decode_field(w, &f.ty, &format!("out->{fname}[i]"), &f.constraints, &fname, &pfx, &pfx_upper, "&arr", opts);
                        w.dedent();
                        w.line("}");
                        w.line(&format!("out->{fname}_count = reps;"));
                    } else if matches!(f.occurrence, Occurrence::Optional) {
                        if let Some(b) = &before {
                            w.line(&format!("if (count > {}) {{", required + b[i]));
                        } else {
                            w.line("if (!nanocbor_at_end(&arr)) {");
                        }
                        w.indent();
                        let dest = c_decode_dest(w, s, f, module, &pfx, &pfx_upper);
                        emit_c_decode_field(w, &f.ty, &dest, &f.constraints, &fname, &pfx, &pfx_upper, "&arr", opts);
                        w.line(&format!("out->{fname}_present = true;"));
                        w.dedent();
                        w.line("}");
                    } else {
                        w.line("if (nanocbor_at_end(&arr)) {");
                        w.indent();
                        w.line(&format!(
                            "if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_MISSING, {:?}, 0}});",
                            f.name.as_str()
                        ));
                        w.line("return -1;");
                        w.dedent();
                        w.line("}");
                        let dest = c_decode_dest(w, s, f, module, &pfx, &pfx_upper);
                        emit_c_decode_field(w, &f.ty, &dest, &f.constraints, &fname, &pfx, &pfx_upper, "&arr", opts);
                    }
                }
                w.line("if (!nanocbor_at_end(&arr)) return -1; /* too many items */");
                w.line("nanocbor_leave_container(dec, &arr);");
                w.line("return NANOCBOR_OK;");
                w.dedent();
                w.line("}");
                return;
            }

            w.line("nanocbor_value_t map;");
            w.line("if (nanocbor_enter_map(dec, &map) < 0) return -1;");
            w.blank();

            // Zero-init the output
            w.line(&format!("memset(out, 0, sizeof({type_name}_t));"));
            // Each required member must be read (dgtlrift/cddlc#102).
            for f in s.fields.iter().filter(|f| f.occurrence.is_required()) {
                w.line(&format!("bool have_{} = false;", to_snake_case(&f.name)));
            }
            w.blank();

            w.line("while (!nanocbor_at_end(&map)) {");
            w.indent();
            // A map with integer keys (`1 => tstr`) reads each key as text or
            // an integer (dgtlrift/cddlc#176).
            // Other entries with typed keys are read again as their key type
            // from where the key starts (dgtlrift/cddlc#136).
            let typed_rest = s.rest.as_ref().is_some_and(|r| !text_rest(r));
            let int_keys = cddlc_codegen::has_int_keys(s) || typed_rest;
            if typed_rest {
                w.line("nanocbor_value_t key_start = map;");
            }
            w.line("const uint8_t *key_ptr = NULL; size_t key_len = 0;");
            if int_keys {
                w.line("bool key_is_int = false; int64_t key_int = 0;");
                w.line("if (nanocbor_get_type(&map) == NANOCBOR_TYPE_TSTR) {");
                w.line("    if (nanocbor_get_tstr(&map, &key_ptr, &key_len) < 0) return -1;");
                w.line("} else {");
                w.line("    if (nanocbor_get_int64(&map, &key_int) < 0) return -1;");
                w.line("    key_is_int = true;");
                w.line("}");
            } else {
                w.line("if (nanocbor_get_tstr(&map, &key_ptr, &key_len) < 0) return -1;");
            }
            w.blank();

            for f in &s.fields {
                let fname  = to_snake_case(&f.name);
                let cond = match &f.key {
                    cddlc_ir::FieldKey::Text(t) => {
                        let text = format!("key_len == {} && memcmp(key_ptr, {:?}, {}) == 0", t.len(), t.as_str(), t.len());
                        if int_keys { format!("!key_is_int && {text}") } else { text }
                    }
                    cddlc_ir::FieldKey::Int(n) => format!("key_is_int && key_int == {n}LL"),
                    cddlc_ir::FieldKey::Uint(n) => format!("key_is_int && key_int == {}LL", (*n).min(i64::MAX as u64)),
                };
                w.line(&format!("if ({cond}) {{"));
                w.indent();
                let dest = c_decode_dest(w, s, f, module, &pfx, &pfx_upper);
                emit_c_decode_field(w, &f.ty, &dest, &f.constraints, &fname, &pfx, &pfx_upper, "&map", opts);
                if matches!(f.occurrence, Occurrence::Optional) {
                    w.line(&format!("out->{fname}_present = true;"));
                }
                if f.occurrence.is_required() {
                    w.line(&format!("have_{fname} = true;"));
                }
                w.dedent();
                w.line("} else");
            }
            if let Some(r) = &s.rest {
                // Other keys are the map's other entries (dgtlrift/cddlc#82).
                let rname = to_snake_case(&r.name);
                let cap = capacity_value(
                    r.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
                    opts.max_array,
                );
                if int_keys && !typed_rest {
                    w.line("if (key_is_int) return -1; /* the other entries have text keys */ else");
                }
                w.line("{");
                w.indent();
                w.line(&format!(
                    "if (out->{rname}_count >= {cap}) {{ if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_OVERFLOW, \"{rname}\", 0}}); return -1; }}"
                ));
                if typed_rest {
                    w.line("nanocbor_value_t kd = key_start;");
                    emit_c_decode_field(w, &r.key_ty, &format!("out->{rname}[out->{rname}_count].key"), &[], &rname, &pfx, &pfx_upper, "&kd", opts);
                } else {
                    w.line(&format!("out->{rname}[out->{rname}_count].key = (cbor_str_t){{ (const char*)key_ptr, key_len }};"));
                }
                emit_c_decode_field(w, &r.val_ty, &format!("out->{rname}[out->{rname}_count].value"), &[], &rname, &pfx, &pfx_upper, "&map", opts);
                w.line(&format!("out->{rname}_count++;"));
                w.dedent();
                w.line("}");
            } else {
                // An unknown member: reported, and the map is rejected, as a map
                // has only the members its schema gives it (dgtlrift/cddlc#102).
                w.line("{");
                w.indent();
                w.line("if (cb && cb->on_unknown_field) {");
                w.indent();
                w.line("cbor_str_t uk = { (const char*)key_ptr, key_len };");
                w.line("cb->on_unknown_field(ctx, uk);");
                w.dedent();
                w.line("}");
                w.line("return -1;");
                w.dedent();
                w.line("}");
            }

            w.dedent();
            w.line("}"); // while

            // Check required fields
            for f in s.fields.iter().filter(|f| f.occurrence.is_required()) {
                w.line(&format!("if (!have_{}) {{", to_snake_case(&f.name)));
                w.indent();
                w.line(&format!(
                    "if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_MISSING, {:?}, 0}});",
                    f.name.as_str()
                ));
                w.line("return -1;");
                w.dedent();
                w.line("}");
            }
            w.line("nanocbor_leave_container(dec, &map);");
            w.line("return NANOCBOR_OK;");
        }

        TypeDef::Enum(e) => {
            if let Some(tag) = e.tagged {
                w.line("{");
                w.indent();
                w.line("uint32_t tag;");
                w.line("if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("if (tag != {}) return -1;", tag.0));
                w.dedent();
                w.line("}");
            }

            // A variant of a map or array type: the first whose decoder reads
            // the value, each tried on a copy of the decoder (dgtlrift/cddlc#102).
            for v in &e.variants {
                let TypeRef::Named(n) = &v.ty else { continue };
                if !matches!(module.get(n), Some(TypeDef::Struct(_) | TypeDef::Array(_) | TypeDef::Map(_))) {
                    continue;
                }
                let vname  = to_snake_case(&v.name);
                let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                w.line("{");
                w.indent();
                w.line("nanocbor_value_t probe = *dec;");
                if indirect(&e.name, &v.ty, module) {
                    // A pointer alternative: memory from the caller, tried
                    // quietly, and given back if it isn't this one
                    // (dgtlrift/cddlc#156).
                    let ptr = ptr_type(&v.ty, module);
                    let rel = if needs_release(resolve_named(n, module), module) {
                        format!("{pfx}_{}_release(p, cb, ctx); ", to_snake_case(resolve_named(n, module)))
                    } else {
                        String::new()
                    };
                    w.line(&format!("{ptr} *p = (cb && cb->alloc) ? ({ptr} *)cb->alloc(ctx, sizeof *p) : NULL;"));
                    w.line("if (p) {");
                    w.indent();
                    w.line("memset(p, 0, sizeof *p);");
                    w.line(&format!("{pfx}_callbacks_t quiet = *cb; quiet.on_error = NULL; quiet.on_unknown_field = NULL;"));
                    w.line(&format!(
                        "if ({pfx}_{}_decode(&probe, p, &quiet, ctx) == 0) {{ *dec = probe; out->value.{vname} = p; out->tag = {vconst}; return NANOCBOR_OK; }}",
                        to_snake_case(n)
                    ));
                    w.line(&format!("{rel}if (cb->release) cb->release(ctx, p);"));
                    w.dedent();
                    w.line("}");
                } else {
                    w.line(&format!(
                        "if ({pfx}_{}_decode(&probe, &out->value.{vname}, NULL, NULL) == 0) {{ *dec = probe; out->tag = {vconst}; return NANOCBOR_OK; }}",
                        to_snake_case(n)
                    ));
                }
                w.dedent();
                w.line("}");
            }

            // `null` (CBOR null, dgtlrift/cddlc#147), a bool, or an integer:
            // each tried on a copy of the decoder.
            for v in &e.variants {
                let vname  = to_snake_case(&v.name);
                let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                let get = match &v.ty {
                    TypeRef::Primitive(Primitive::Null) => "nanocbor_get_null(&probe)".to_owned(),
                    TypeRef::Primitive(Primitive::Bool) => format!("nanocbor_get_bool(&probe, &out->value.{vname})"),
                    TypeRef::Primitive(Primitive::Uint) => format!("nanocbor_get_uint64(&probe, &out->value.{vname})"),
                    TypeRef::Primitive(Primitive::Int) => format!("nanocbor_get_int64(&probe, &out->value.{vname})"),
                    _ => continue,
                };
                w.line("{");
                w.indent();
                w.line("nanocbor_value_t probe = *dec;");
                w.line(&format!("if ({get} >= 0) {{ *dec = probe; out->tag = {vconst}; return NANOCBOR_OK; }}"));
                w.dedent();
                w.line("}");
            }

            // Peek at type
            // An `any` alternative takes what the others don't (dgtlrift/cddlc#178),
            // so text that matches no alternative is read from a probe.
            let any = e.variants.iter().find(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Any)));
            w.line("uint8_t major = nanocbor_get_type(dec);");
            w.line("if (major == NANOCBOR_TYPE_TSTR) {");
            w.indent();
            w.line("const uint8_t *s; size_t slen;");
            w.line("nanocbor_value_t _t = *dec;");
            w.line("if (nanocbor_get_tstr(&_t, &s, &slen) < 0) return -1;");
            for v in &e.variants {
                if matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)) && is_text_literal(v) {
                    let cddl_str = cddlc_codegen::enum_variant_text(v);
                    let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                    w.line(&format!(
                        "if (slen == {} && memcmp(s, {:?}, {}) == 0) {{ *dec = _t; out->tag = {vconst}; return NANOCBOR_OK; }}",
                        cddl_str.len(), cddl_str, cddl_str.len()
                    ));
                }
            }
            // A `tstr` alternative takes any other text.
            if let Some(v) = e.variants.iter().find(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)) && !is_text_literal(v)) {
                let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                w.line(&format!(
                    "*dec = _t; out->value.{} = (cbor_str_t){{ (const char*)s, slen }}; out->tag = {vconst}; return NANOCBOR_OK;",
                    to_snake_case(&v.name)
                ));
            }
            if any.is_none() {
                w.line("return -1; /* unknown variant */");
            }
            w.dedent();
            w.line("}");
            match any {
                Some(v) => {
                    let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
                    let vname = to_snake_case(&v.name);
                    w.line(&format!("if (nanocbor_get_subcbor(dec, &out->value.{vname}.ptr, &out->value.{vname}.len) < 0) return -1;"));
                    w.line(&format!("out->tag = {vconst}; return NANOCBOR_OK;"));
                }
                None => w.line("return -1;"),
            }
        }

        TypeDef::Array(a) => {
            if let Some(tag) = a.tagged {
                w.line("{");
                w.indent();
                w.line("uint32_t tag;");
                w.line("if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("if (tag != {}) return -1;", tag.0));
                w.dedent();
                w.line("}");
            }
            let cap = capacity_value(
                a.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
                opts.max_array,
            );
            w.line("nanocbor_value_t arr;");
            w.line("if (nanocbor_enter_array(dec, &arr) < 0) return -1;");
            w.line("out->count = 0;");
            w.line(&format!("while (!nanocbor_at_end(&arr) && out->count < {cap}) {{"));
            w.indent();
            if let Some(rec) = flat_record(a, module) {
                for f in &rec.fields {
                    let item = format!("out->items[out->count].{}", to_snake_case(&f.name));
                    emit_c_decode_field(w, &f.ty, &item, &f.constraints, &f.name, &pfx, &pfx_upper, "&arr", opts);
                }
            } else {
                emit_c_decode_field(w, &a.element, "out->items[out->count]", &[], "item", &pfx, &pfx_upper, "&arr", opts);
            }
            w.line("out->count++;");
            w.dedent();
            w.line("}");
            w.line("nanocbor_leave_container(dec, &arr);");
            w.line("return NANOCBOR_OK;");
        }

        TypeDef::Alias(a) => {
            if let Some(tag) = a.tagged {
                w.line("{");
                w.indent();
                w.line("uint32_t tag;");
                w.line("if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("if (tag != {}) {{", tag.0));
                w.indent();
                w.line(&format!(
                    "if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_TAG, \"{type_name}\", 0}});"
                ));
                w.line("return -1;");
                w.dedent();
                w.line("}");
                w.dedent();
                w.line("}");
            }
            let dest = if a.tagged.is_some() || !a.constraints.is_empty() {
                "out->value".to_string()
            } else {
                "(*out)".to_string()
            };
            emit_c_decode_field(w, &a.ty, &dest, &a.constraints, &type_name, &pfx, &pfx_upper, "dec", opts);
            w.line("return NANOCBOR_OK;");
        }

        TypeDef::Map(m) => {
            if let Some(tag) = m.tagged {
                w.line("{");
                w.indent();
                w.line("uint32_t tag;");
                w.line("if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("if (tag != {}) return -1;", tag.0));
                w.dedent();
                w.line("}");
            }
            let cap = capacity_value(
                m.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
                opts.max_array,
            );
            w.line("nanocbor_value_t map;");
            w.line("if (nanocbor_enter_map(dec, &map) < 0) return -1;");
            w.line("out->count = 0;");
            w.line("while (!nanocbor_at_end(&map)) {");
            w.indent();
            w.line(&format!(
                "if (out->count >= {cap}) {{ if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_OVERFLOW, \"{type_name}\", 0}}); return -1; }}"
            ));
            emit_c_decode_field(w, &m.key_ty, "out->entries[out->count].key", &[], "key", &pfx, &pfx_upper, "&map", opts);
            emit_c_decode_field(w, &m.val_ty, "out->entries[out->count].value", &[], "value", &pfx, &pfx_upper, "&map", opts);
            w.line("out->count++;");
            w.dedent();
            w.line("}");
            w.line("nanocbor_leave_container(dec, &map);");
            w.line("return NANOCBOR_OK;");
        }
    }

    w.dedent();
    w.line("}");
}

fn emit_c_decode_field(
    w:           &mut IndentWriter,
    ty:          &TypeRef,
    dest:        &str,
    constraints: &[Constraint],
    field_name:  &str,
    pfx:         &str,
    pfx_upper:   &str,
    dec_expr:    &str,
    opts:        &CodegenOptions,
) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = c_decode_call_expr(p, dest, dec_expr);
            w.line(&format!("if ({call} < 0) return -1;"));

            // Inline validation
            for c in constraints {
                if let Some(check) = c_constraint_check(c, dest, p) {
                    w.line(&format!("if (!({check})) {{"));
                    w.indent();
                    w.line(&format!(
                        "if (cb && cb->on_error) cb->on_error(ctx, ({pfx}_field_err_t){{{pfx_upper}_ERR_VALIDATION, {:?}, 0}});",
                        field_name
                    ));
                    w.line("return -1;");
                    w.dedent();
                    w.line("}");
                }
            }
        }
        TypeRef::Named(n) => {
            let tn = to_snake_case(n);
            w.line(&format!("if ({pfx}_{tn}_decode({dec_expr}, &{dest}, cb, ctx) < 0) return -1;"));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line("{");
            w.indent();
            w.line("uint32_t tag;");
            w.line(&format!("if (nanocbor_get_tag({dec_expr}, &tag) < 0) return -1;"));
            w.line(&format!("if (tag != {}) return -1;", tag.0));
            emit_c_decode_field(w, inner, dest, constraints, field_name, pfx, pfx_upper, dec_expr, opts);
            w.dedent();
            w.line("}");
        }
        TypeRef::Choice(cs) => match prim_choice_c(cs) {
            // The first alternative that reads the item, tried on a copy of the
            // decoder so a failed attempt consumes nothing.
            Some(alts) => {
                w.line("{");
                w.indent();
                w.line("bool _matched = false;");
                for (i, p) in alts.iter().enumerate() {
                    w.line("if (!_matched) {");
                    w.indent();
                    w.line(&format!("nanocbor_value_t _probe = *({dec_expr});"));
                    w.line(&format!(
                        "if ({} >= 0) {{ *({dec_expr}) = _probe; {dest}.kind = {i}; _matched = true; }}",
                        c_decode_call_expr(p, &format!("{dest}.as.alt{i}"), "&_probe")
                    ));
                    w.dedent();
                    w.line("}");
                }
                w.line("if (!_matched) return -1;");
                w.dedent();
                w.line("}");
            }
            None => w.line(&format!("nanocbor_skip({dec_expr}); /* choice — skip */")),
        },
    }
}

/// Type-aware constraint check for C.
///
/// For string/byte primitives, `.size` checks the `.len` field.
/// For integer primitives, `.size N` checks that the value fits in N bytes
/// (e.g., `.size 4` on a uint → `val <= 0xFFFFFFFFU`).
fn c_constraint_check(c: &Constraint, val_expr: &str, p: &Primitive) -> Option<String> {
    match c {
        Constraint::SizeExact(n) => match p {
            Primitive::Uint => {
                let max = uint_max_for_bytes(*n)?;
                Some(format!("{val_expr} <= {max}"))
            }
            Primitive::Int => {
                let (lo, hi) = int_range_for_bytes(*n)?;
                Some(format!("{val_expr} >= {lo} && {val_expr} <= {hi}"))
            }
            _ => constraint_to_c_check(c, val_expr),
        },
        Constraint::SizeRange { .. } => match p {
            Primitive::Uint | Primitive::Int => None, // uncommon; skip
            _ => constraint_to_c_check(c, val_expr),
        },
        // A text literal (`cat: "kernel"`): compare length and bytes; `==` would
        // compare a `cbor_str_t` struct against a pointer.
        Constraint::Eq(cddlc_ir::LiteralValue::Text(t)) if *p == Primitive::Tstr => Some(format!(
            "({val_expr}.len == {n} && memcmp({val_expr}.ptr, {t:?}, {n}) == 0)",
            n = t.len()
        )),
        Constraint::Ne(cddlc_ir::LiteralValue::Text(t)) if *p == Primitive::Tstr => Some(format!(
            "!({val_expr}.len == {n} && memcmp({val_expr}.ptr, {t:?}, {n}) == 0)",
            n = t.len()
        )),
        // A byte-string literal (RFC 9277's `'BOR'`): compare length and
        // bytes (dgtlrift/cddlc#142).
        Constraint::Eq(cddlc_ir::LiteralValue::Bytes(b)) if *p == Primitive::Bstr => Some(c_bytes_eq(val_expr, b)),
        Constraint::Ne(cddlc_ir::LiteralValue::Bytes(b)) if *p == Primitive::Bstr => Some(format!("!{}", c_bytes_eq(val_expr, b))),
        // `.bits` (dgtlrift/cddlc#107): no bit outside the mask is set.
        Constraint::Bits(bits) => match p {
            Primitive::Uint => Some(format!(
                "(({val_expr}) & ~UINT64_C({:#x})) == 0", cddlc_codegen::bits_mask_u64(bits)
            )),
            Primitive::Bstr => {
                let m = cddlc_codegen::bits_mask_bytes(bits);
                let bytes = m.iter().map(|b| format!("{b:#04x}")).collect::<Vec<_>>().join(", ");
                Some(format!(
                    "cddlc_bits_ok({val_expr}.ptr, {val_expr}.len, (const uint8_t[]){{{bytes}}}, {})", m.len()
                ))
            }
            _ => None,
        },
        _ => constraint_to_c_check(c, val_expr),
    }
}

/// The bits a member's `.bits` allows.
fn bits_of(f: &cddlc_ir::FieldDef) -> Option<&Vec<u64>> {
    f.constraints.iter().find_map(|c| match c {
        Constraint::Bits(b) => Some(b),
        _ => None,
    })
}

fn uint_max_for_bytes(n: usize) -> Option<String> {
    match n {
        1 => Some("0xFFU".into()),
        2 => Some("0xFFFFU".into()),
        4 => Some("0xFFFFFFFFU".into()),
        8 => None, // uint64 always fits
        _ => None,
    }
}

fn int_range_for_bytes(n: usize) -> Option<(i64, i64)> {
    match n {
        1 => Some((-128, 127)),
        2 => Some((-32768, 32767)),
        4 => Some((-2147483648, 2147483647)),
        8 => None,
        _ => None,
    }
}

fn c_decode_call_expr(p: &Primitive, dest: &str, dec_expr: &str) -> String {
    match p {
        Primitive::Bool =>
            format!("nanocbor_get_bool({dec_expr}, &{dest})"),
        Primitive::Null | Primitive::Undefined =>
            format!("nanocbor_get_null({dec_expr})"),
        Primitive::Uint =>
            format!("nanocbor_get_uint64({dec_expr}, &{dest})"),
        Primitive::Int =>
            format!("nanocbor_get_int64({dec_expr}, &{dest})"),
        Primitive::Float32 | Primitive::Float16 =>
            format!("nanocbor_get_float({dec_expr}, &{dest})"),
        Primitive::Float64 | Primitive::Float =>
            format!("nanocbor_get_double({dec_expr}, &{dest})"),
        Primitive::Tstr =>
            format!("(nanocbor_get_tstr({dec_expr}, (const uint8_t**)&{dest}.ptr, &{dest}.len))"),
        Primitive::Bstr =>
            format!("nanocbor_get_bstr({dec_expr}, &{dest}.ptr, &{dest}.len)"),
        // The whole item's encoding, which nanocbor then skips.
        Primitive::Any =>
            format!("nanocbor_get_subcbor({dec_expr}, &{dest}.ptr, &{dest}.len)"),
    }
}

// ── Test file ─────────────────────────────────────────────────────────────────

fn emit_tests(module: &IrModule, opts: &CodegenOptions) -> String {
    let name  = module.name.as_str();
    let pfx   = to_snake_case(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line("/* Roundtrip encode/decode tests */");
    w.line(&format!("#include \"{name}.h\""));
    w.line("#include <assert.h>");
    w.line("#include <string.h>");
    w.line("#include <stdio.h>");
    w.blank();
    w.line("#include <stdlib.h>");
    w.line("static uint8_t buf[1024];");
    w.blank();
    // A recursive member's memory, from the heap here; any allocator (a pool,
    // an arena) works (dgtlrift/cddlc#156).
    w.line("static void *test_alloc(void *ctx, size_t size) { (void)ctx; return calloc(1, size); }");
    w.line("static void test_release(void *ctx, void *ptr) { (void)ctx; free(ptr); }");
    w.line(&format!("static const {pfx}_callbacks_t test_cb = {{ .alloc = test_alloc, .release = test_release }};"));
    w.blank();

    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => emit_c_struct_test(&mut w, s, &pfx, module, opts),
            TypeDef::Alias(a)  => emit_c_alias_test(&mut w, a, &pfx, opts),
            TypeDef::Array(a)  => emit_c_array_test(&mut w, a, &pfx, opts),
            TypeDef::Enum(e)   => emit_c_enum_test(&mut w, e, &pfx, module, opts),
            TypeDef::Map(m)    => emit_c_map_test(&mut w, m, &pfx, opts),
        }
        w.blank();
    }

    // main
    w.line("int main(void) {");
    w.indent();
    for (_, def) in &module.types {
        let type_name = to_snake_case(def.name());
        w.line(&format!("test_{pfx}_{type_name}_roundtrip();"));
    }
    w.line("printf(\"All roundtrip tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}

fn emit_c_struct_test(w: &mut IndentWriter, s: &StructDef, pfx: &str, module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_snake_case(&s.name);
    w.line(&format!("static void test_{pfx}_{type_name}_roundtrip(void) {{"));
    w.indent();

    // Static buffers for any size-constrained tstr named fields, and for inline Tstr/Bstr/Any fields
    for f in &s.fields {
        if matches!(f.occurrence, Occurrence::Optional) { continue; }
        if is_prim_choice_c(&f.ty) { continue; }
        let fname = to_snake_case(&f.name);
        if let TypeRef::Named(n) = &f.ty {
            if let Some(n) = tstr_size_constraint_for(n, module) {
                let s_lit: String = "a".repeat(n);
                w.line(&format!("static const char _{fname}_data[{n}] = {:?};", s_lit));
            }
        }
        match effective_primitive(&f.ty) {
            Some(Primitive::Tstr) => {
                let text = text_literal_of(f).unwrap_or("hello");
                w.line(&format!("static const char _{fname}_str[] = {text:?};"));
            }
            // `.bits`: the mask itself is a value it allows (dgtlrift/cddlc#107).
            Some(Primitive::Bstr) if bits_of(f).is_some() => {
                let m = cddlc_codegen::bits_mask_bytes(bits_of(f).expect("checked"));
                let bytes = m.iter().map(|b| format!("{b:#04x}")).collect::<Vec<_>>().join(", ");
                w.line(&format!("static const uint8_t _{fname}_cbor[{}] = {{{bytes}}};", m.len().max(1)));
            }
            Some(Primitive::Bstr | Primitive::Any) => {
                w.line(&format!("static const uint8_t _{fname}_cbor[1] = {{0xf6}};"));
            }
            _ => {}
        }
    }

    // Build a zeroed instance
    w.line(&format!("{type_name}_t original = {{0}};"));

    // Fill in default values for each field
    for f in &s.fields {
        if matches!(f.occurrence, Occurrence::Optional) { continue; }
        // A choice's zeroed union is its first alternative's zero.
        if is_prim_choice_c(&f.ty) { continue; }
        // A required pointer member points at a zero value of its type
        // (dgtlrift/cddlc#156); an optional one stays absent.
        if indirect(&s.name, &f.ty, module) {
            let fname = to_snake_case(&f.name);
            w.line(&format!("static {} _{fname}_target;", ptr_type(&f.ty, module)));
            w.line(&format!("original.{fname} = &_{fname}_target;"));
            continue;
        }
        // A repeated member gets as many items as it needs, each like the
        // first (dgtlrift/cddlc#134); one that may be empty stays empty.
        if f.occurrence.is_repeated() && cddlc_codegen::repeat_bounds(&f.occurrence).0 == 0 { continue; }
        let base = to_snake_case(&f.name);
        let fname = if f.occurrence.is_repeated() { format!("{}[0]", to_snake_case(&f.name)) } else { to_snake_case(&f.name) };
        if let TypeRef::Named(n) = &f.ty {
            if let Some(n) = tstr_size_constraint_for(n, module) {
                w.line(&format!("original.{fname}.value.ptr = _{base}_data;"));
                w.line(&format!("original.{fname}.value.len = {n};"));
                continue;
            }
        }
        match effective_primitive(&f.ty) {
            Some(Primitive::Tstr) => {
                let len = text_literal_of(f).map_or(5, str::len);
                w.line(&format!("original.{fname}.ptr = _{base}_str;"));
                w.line(&format!("original.{fname}.len = {len};"));
                continue;
            }
            Some(Primitive::Bstr) if bits_of(f).is_some() => {
                let n = cddlc_codegen::bits_mask_bytes(bits_of(f).expect("checked")).len();
                w.line(&format!("original.{fname}.ptr = _{base}_cbor;"));
                w.line(&format!("original.{fname}.len = {n};"));
                continue;
            }
            Some(Primitive::Uint) if bits_of(f).is_some() => {
                let m = cddlc_codegen::bits_mask_u64(bits_of(f).expect("checked"));
                w.line(&format!("original.{fname} = UINT64_C({m:#x});"));
                continue;
            }
            Some(Primitive::Bstr | Primitive::Any) => {
                w.line(&format!("original.{fname}.ptr = _{base}_cbor;"));
                w.line(&format!("original.{fname}.len = 1; /* CBOR null 0xf6 */"));
                continue;
            }
            _ => {}
        }
        // A member pinned to one value (`always: true`, `version: 1`) must use it, or
        // its own decoder rejects the roundtrip (dgtlrift/cddlc#35).
        let val = scalar_literal_of(f).or_else(|| c_default_value(&f.ty, opts));
        if let Some(v) = val {
            w.line(&format!("original.{fname} = {v};"));
        }
    }
    for f in s.fields.iter().filter(|f| f.occurrence.is_repeated()) {
        let min = cddlc_codegen::repeat_bounds(&f.occurrence).0;
        if min == 0 || is_prim_choice_c(&f.ty) { continue; }
        let fname = to_snake_case(&f.name);
        w.line(&format!("for (size_t i = 1; i < {min}; i++) original.{fname}[i] = original.{fname}[0];"));
        w.line(&format!("original.{fname}_count = {min};"));
    }
    w.blank();

    // Encode
    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line(&format!("assert({pfx}_{type_name}_encode(&enc, &original) == 0);"));
    w.line("size_t enc_len = nanocbor_encoded_len(&enc);");
    w.blank();

    // Decode
    w.line(&format!("{type_name}_t decoded = {{0}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line(&format!("assert({pfx}_{type_name}_decode(&dec, &decoded, &test_cb, NULL) == 0);"));
    w.blank();

    // Assert equality for scalar fields
    for f in &s.fields {
        let fname = to_snake_case(&f.name);
        if f.occurrence.is_repeated() {
            w.line(&format!("assert(original.{fname}_count == decoded.{fname}_count);"));
            continue;
        }
        match &f.ty {
            TypeRef::Primitive(Primitive::Tstr | Primitive::Bstr | Primitive::Any) => {
                w.line(&format!("assert(original.{fname}.len == decoded.{fname}.len);"));
            }
            TypeRef::Primitive(_) => {
                w.line(&format!("assert(original.{fname} == decoded.{fname});"));
            }
            _ => {}
        }
    }

    if needs_release(&s.name, module) {
        w.line(&format!("{pfx}_{type_name}_release(&decoded, &test_cb, NULL);"));
    }
    w.line(&format!("printf(\"{pfx}_{type_name}_roundtrip: PASS\\n\");"));
    w.dedent();
    w.line("}");
}

fn emit_c_alias_test(w: &mut IndentWriter, a: &AliasDef, pfx: &str, _opts: &CodegenOptions) {
    let type_name = to_snake_case(&a.name);
    w.line(&format!("static void test_{pfx}_{type_name}_roundtrip(void) {{"));
    w.indent();

    // For size-constrained tstr aliases, initialise with a valid-length literal
    // so that the roundtrip encode→decode passes constraint validation.
    let tstr_size = a.constraints.iter().find_map(|c| match c {
        Constraint::SizeExact(n) => Some(*n),
        Constraint::SizeRange { min: Some(n), .. } => Some(*n),
        _ => None,
    }).filter(|_| matches!(&a.ty, TypeRef::Primitive(Primitive::Tstr)));

    w.line(&format!("{type_name}_t original = {{0}};"));
    // A value pinned to a literal, or with a lower bound (`40 .plus 2`,
    // `uint .within (1..5)`; dgtlrift/cddlc#86).
    for c in &a.constraints {
        let set = match c {
            Constraint::Eq(cddlc_ir::LiteralValue::Text(t)) => format!("(cbor_str_t){{ {t:?}, {} }}", t.len()),
            // A byte-string literal (dgtlrift/cddlc#142).
            Constraint::Eq(cddlc_ir::LiteralValue::Bytes(b)) if !b.is_empty() => {
                let bytes = b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ");
                format!("(cbor_slice_t){{ (const uint8_t[]){{{bytes}}}, {} }}", b.len())
            }
            Constraint::Eq(cddlc_ir::LiteralValue::Uint(n)) => format!("{n}U"),
            Constraint::Eq(cddlc_ir::LiteralValue::Int(n)) => format!("{n}"),
            Constraint::ValueRangeUint { min: Some(lo), .. } if *lo > 0 => format!("{}U", *lo),
            Constraint::ValueRangeInt { min: Some(lo), .. } if *lo > 0 => format!("{}", *lo),
            _ => continue,
        };
        w.line(&format!("original.value = {set};"));
        break;
    }
    if let Some(n) = tstr_size {
        let s_lit: String = "a".repeat(n);
        w.line(&format!("static const char _val_data[{n}] = {:?};", s_lit));
        w.line(&format!("original.value.ptr = _val_data;"));
        w.line(&format!("original.value.len = {n};"));
    }
    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line(&format!("assert({pfx}_{type_name}_encode(&enc, &original) == 0);"));
    w.line("size_t enc_len = nanocbor_encoded_len(&enc);");
    w.line(&format!("{type_name}_t decoded = {{0}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line(&format!("assert({pfx}_{type_name}_decode(&dec, &decoded, &test_cb, NULL) == 0);"));
    w.line(&format!("printf(\"{pfx}_{type_name}_roundtrip: PASS\\n\");"));
    w.dedent();
    w.line("}");
}

fn emit_c_array_test(w: &mut IndentWriter, a: &ArrayDef, pfx: &str, _opts: &CodegenOptions) {
    let type_name = to_snake_case(&a.name);
    w.line(&format!("static void test_{pfx}_{type_name}_roundtrip(void) {{"));
    w.indent();
    w.line(&format!("{type_name}_t original = {{0}};"));
    w.line("original.count = 0;");
    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line(&format!("assert({pfx}_{type_name}_encode(&enc, &original) == 0);"));
    w.line("size_t enc_len = nanocbor_encoded_len(&enc);");
    w.line(&format!("{type_name}_t decoded = {{0}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line(&format!("assert({pfx}_{type_name}_decode(&dec, &decoded, &test_cb, NULL) == 0);"));
    w.line("assert(decoded.count == original.count);");
    w.line(&format!("printf(\"{pfx}_{type_name}_roundtrip: PASS\\n\");"));
    w.dedent();
    w.line("}");
}

fn emit_c_map_test(w: &mut IndentWriter, m: &MapDef, pfx: &str, opts: &CodegenOptions) {
    let type_name = to_snake_case(&m.name);
    w.line(&format!("static void test_{pfx}_{type_name}_roundtrip(void) {{"));
    w.indent();
    w.line(&format!("{type_name}_t original = {{0}};"));
    // One entry when the key is text and the value a scalar.
    let value = c_default_value(&m.val_ty, opts);
    let one = matches!(m.key_ty, TypeRef::Primitive(Primitive::Tstr)) && value.is_some();
    if let (true, Some(v)) = (one, &value) {
        w.line("original.entries[0].key = (cbor_str_t){ \"k\", 1 };");
        w.line(&format!("original.entries[0].value = {v};"));
        w.line("original.count = 1;");
    }
    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line(&format!("assert({pfx}_{type_name}_encode(&enc, &original) == 0);"));
    w.line("size_t enc_len = nanocbor_encoded_len(&enc);");
    w.line(&format!("{type_name}_t decoded = {{0}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line(&format!("assert({pfx}_{type_name}_decode(&dec, &decoded, &test_cb, NULL) == 0);"));
    w.line("assert(decoded.count == original.count);");
    if one {
        w.line("assert(decoded.entries[0].key.len == 1 && memcmp(decoded.entries[0].key.ptr, \"k\", 1) == 0);");
        w.line("assert(decoded.entries[0].value == original.entries[0].value);");
    }
    w.line(&format!("printf(\"{pfx}_{type_name}_roundtrip: PASS\\n\");"));
    w.dedent();
    w.line("}");
}

fn emit_c_enum_test(w: &mut IndentWriter, e: &EnumDef, pfx: &str, module: &IrModule, _opts: &CodegenOptions) {
    let type_name = to_snake_case(&e.name);
    w.line(&format!("static void test_{pfx}_{type_name}_roundtrip(void) {{"));
    w.indent();
    if let Some(v) = e.variants.first() {
        let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
        w.line(&format!("{type_name}_t original = {{ .tag = {vconst} }};"));
        // A pointer alternative points at a zero value of its type
        // (dgtlrift/cddlc#156).
        if indirect(&e.name, &v.ty, module) {
            w.line(&format!("static {} _target;", ptr_type(&v.ty, module)));
            w.line(&format!("original.value.{} = &_target;", to_snake_case(&v.name)));
        }
        // A map variant's members pinned to a literal (`type: "start"`) hold it.
        if let (TypeRef::Named(n), false) = (&v.ty, indirect(&e.name, &v.ty, module)) {
            if let Some(TypeDef::Struct(st)) = module.get(n) {
                let vname = to_snake_case(&v.name);
                for f in st.fields.iter().filter(|f| f.occurrence.is_required()) {
                    let fname = to_snake_case(&f.name);
                    if let Some(t) = text_literal_of(f) {
                        w.line(&format!("original.value.{vname}.{fname} = (cbor_str_t){{ {t:?}, {} }};", t.len()));
                    } else if let Some(x) = scalar_literal_of(f) {
                        w.line(&format!("original.value.{vname}.{fname} = {x};"));
                    }
                }
            }
        }
    } else {
        w.line(&format!("{type_name}_t original = {{0}};"));
    }
    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line(&format!("assert({pfx}_{type_name}_encode(&enc, &original) == 0);"));
    w.line("size_t enc_len = nanocbor_encoded_len(&enc);");
    w.line(&format!("{type_name}_t decoded = {{0}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line(&format!("assert({pfx}_{type_name}_decode(&dec, &decoded, &test_cb, NULL) == 0);"));
    w.line("assert(decoded.tag == original.tag);");
    if needs_release(&e.name, module) {
        w.line(&format!("{pfx}_{type_name}_release(&decoded, &test_cb, NULL);"));
    }
    w.line(&format!("printf(\"{pfx}_{type_name}_roundtrip: PASS\\n\");"));
    w.dedent();
    w.line("}");
}

// ── C type helpers ────────────────────────────────────────────────────────────

fn c_field_type(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::ZeroOrMore { capacity } | Occurrence::OneOrMore { capacity }
        | Occurrence::Bounded { capacity, .. } => {
            let cap  = capacity_value(capacity, opts.max_array);
            let base = typeref_to_c(ty, opts);
            // `T name[cap]; size_t name_count`: the declaration puts the
            // member's name in (dgtlrift/cddlc#134).
            format!("{base} {{name}}[{cap}]; size_t {{name}}_count")
        }
        _ => typeref_to_c(ty, opts),
    }
}

/// Whether `ty` (or the type a tag wraps) is a choice of primitives held in a
/// tagged union.
fn is_prim_choice_c(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Tagged(_, inner) => is_prim_choice_c(inner),
        TypeRef::Choice(cs) => prim_choice_c(cs).is_some(),
        _ => false,
    }
}

fn typeref_to_c(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p)    => primitive_to_c(p).to_owned(),
        TypeRef::Named(n)        => format!("{}_t", to_snake_case(n)),
        TypeRef::Tagged(_, inner) => typeref_to_c(inner, opts),
        TypeRef::Choice(cs) => match prim_choice_c(cs) {
            Some(alts) => {
                let members: String = alts.iter().enumerate()
                    .map(|(i, p)| format!("{} alt{i}; ", primitive_to_c(p)))
                    .collect();
                format!("struct {{ uint8_t kind; union {{ {members}}} as; }}")
            }
            None => cs.first()
                .map(|c| typeref_to_c(c, opts))
                .unwrap_or_else(|| "void*".into()),
        },
    }
}

/// Returns the first concrete Primitive in a TypeRef, following Tagged and Choice.
fn effective_primitive(ty: &TypeRef) -> Option<&Primitive> {
    match ty {
        TypeRef::Primitive(p)     => Some(p),
        TypeRef::Choice(cs)       => cs.first().and_then(|c| effective_primitive(c)),
        TypeRef::Tagged(_, inner) => effective_primitive(inner),
        _ => None,
    }
}

/// If `type_name` resolves to a `tstr .size N` alias, return `Some(N)`.
fn tstr_size_constraint_for(type_name: &str, module: &IrModule) -> Option<usize> {
    match module.types.get(type_name)? {
        TypeDef::Alias(a) if matches!(&a.ty, TypeRef::Primitive(Primitive::Tstr)) => {
            a.constraints.iter().find_map(|c| match c {
                Constraint::SizeExact(n) => Some(*n),
                Constraint::SizeRange { min: Some(n), .. } => Some(*n),
                _ => None,
            })
        }
        _ => None,
    }
}

/// The text a `tstr` member is pinned to (`kind: "kernel"`), if any.
fn text_literal_of(f: &cddlc_ir::FieldDef) -> Option<&str> {
    f.constraints.iter().find_map(|c| match c {
        Constraint::Eq(cddlc_ir::LiteralValue::Text(t)) => Some(t.as_str()),
        _ => None,
    })
}

/// The C value of a bool or integer member pinned to one literal, if any.
fn scalar_literal_of(f: &cddlc_ir::FieldDef) -> Option<String> {
    f.constraints.iter().find_map(|c| match c {
        Constraint::Eq(cddlc_ir::LiteralValue::Bool(b)) => Some(b.to_string()),
        Constraint::Eq(cddlc_ir::LiteralValue::Uint(n)) => Some(format!("{n}U")),
        Constraint::Eq(cddlc_ir::LiteralValue::Int(n)) => Some(format!("{n}")),
        _ => None,
    })
}

fn c_default_value(ty: &TypeRef, _opts: &CodegenOptions) -> Option<String> {
    match ty {
        TypeRef::Primitive(Primitive::Uint) => Some("42U".into()),
        TypeRef::Primitive(Primitive::Int)  => Some("-1".into()),
        TypeRef::Primitive(Primitive::Bool) => Some("true".into()),
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => Some("1.0f".into()),
        TypeRef::Primitive(Primitive::Float64 | Primitive::Float)   => Some("1.0".into()),
        _ => None,
    }
}

fn emit_c_doc(w: &mut IndentWriter, doc: Option<&str>) {
    cddlc_codegen::emit_doc_lines(w, doc, cddlc_codegen::DocStyle::Block { open: "/*" });
}

// ── C JSON backend (cJSON) ────────────────────────────────────────────────────

fn emit_cmake_json(name: &str, opts: &CodegenOptions) -> String {
    let header = cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Hash);
    format!(r#"{header}cmake_minimum_required(VERSION 3.16)
project({name}_json C)

set(CMAKE_C_STANDARD 11)

include(FetchContent)
FetchContent_Declare(cjson
    GIT_REPOSITORY https://github.com/DaveGamble/cJSON.git
    # Pinned: v1.7.19 (the latest release) needs CMake < 4; later master commits
    # raise its cmake_minimum_required and fix several memory-safety bugs.
    GIT_TAG        6d9f2443ab071f86e5d9b43025a40929ec41c46c
)
set(ENABLE_CJSON_TEST   OFF CACHE BOOL "" FORCE)
set(ENABLE_CJSON_UTILS  OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS   OFF CACHE BOOL "" FORCE)
FetchContent_MakeAvailable(cjson)

add_library({name}_json
    src/{name}.c
)
target_include_directories({name}_json PUBLIC include ${{cjson_SOURCE_DIR}})
target_link_libraries({name}_json PRIVATE cjson)

enable_testing()
add_executable(test_roundtrip test/test_roundtrip.c)
target_link_libraries(test_roundtrip PRIVATE {name}_json cjson)
add_test(NAME roundtrip COMMAND test_roundtrip)
"#)
}

#[allow(dead_code)]
fn c_json_field_type(ty: &TypeRef, opts: &CodegenOptions) -> String {
    typeref_to_c(ty, opts)
}

// ── JSON-mode type helpers ────────────────────────────────────────────────────

fn typeref_to_c_json(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Tstr)                  => "json_str_t".into(),
        TypeRef::Primitive(Primitive::Bstr)                  => "json_bytes_t".into(),
        TypeRef::Primitive(Primitive::Any)                   => "json_any_t".into(),
        TypeRef::Primitive(p)                                => primitive_to_c(p).to_owned(),
        TypeRef::Named(n)                                    => format!("{}_t", to_snake_case(n)),
        TypeRef::Tagged(_, inner)                            => typeref_to_c_json(inner),
        TypeRef::Choice(cs)                                  => cs.first()
            .map(|c| typeref_to_c_json(c))
            .unwrap_or_else(|| "json_bytes_t".into()),
    }
}

fn c_field_type_json(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::ZeroOrMore { capacity } | Occurrence::OneOrMore { capacity }
        | Occurrence::Bounded { capacity, .. } => {
            let cap  = capacity_value(capacity, opts.max_array);
            let base = typeref_to_c_json(ty);
            // `T name[cap]; size_t name_count`: the declaration puts the
            // member's name in (dgtlrift/cddlc#134).
            format!("{base} {{name}}[{cap}]; size_t {{name}}_count")
        }
        _ => typeref_to_c_json(ty),
    }
}

fn emit_struct_decl_json(w: &mut IndentWriter, s: &StructDef, module: &IrModule, opts: &CodegenOptions) {
    emit_c_doc(w, s.doc.as_deref());
    let type_name = to_snake_case(&s.name);
    w.line("typedef struct {");
    w.indent();
    for f in &s.fields {
        emit_c_doc(w, f.doc.as_deref());
        // A member that holds this type again is a pointer (dgtlrift/cddlc#156).
        let ty_str = if indirect(&s.name, &f.ty, module) {
            format!("{} *", ptr_type(&f.ty, module))
        } else {
            c_field_type_json(&f.ty, &f.occurrence, opts)
        };
        let fname  = to_snake_case(&f.name);
        if ty_str.contains("{name}") {
            w.line(&format!("{};", ty_str.replace("{name}", &fname)));
        } else {
            w.line(&format!("{ty_str} {fname};"));
        }
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("bool {fname}_present;"));
        }
    }
    // The map's other entries (dgtlrift/cddlc#82).
    if let Some(r) = &s.rest {
        let cap = capacity_value(
            r.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
            opts.max_array,
        );
        let rname = to_snake_case(&r.name);
        w.line("/* The map's other entries. */");
        w.line(&format!("struct {{ json_str_t key; {} value; }} {rname}[{cap}];", typeref_to_c_json(&r.val_ty)));
        w.line(&format!("size_t {rname}_count;"));
    }
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_enum_decl_json(w: &mut IndentWriter, e: &EnumDef, module: &IrModule) {
    emit_c_doc(w, e.doc.as_deref());
    let type_name = to_snake_case(&e.name);

    w.line("typedef enum {");
    w.indent();
    for (i, v) in e.variants.iter().enumerate() {
        let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
        if i == e.variants.len() - 1 { w.line(&vconst); } else { w.line(&format!("{vconst},")); }
    }
    w.dedent();
    w.line(&format!("}} {type_name}_tag_t;"));
    w.blank();

    w.line("typedef struct {");
    w.indent();
    w.line(&format!("{type_name}_tag_t tag;"));
    w.line("union {");
    w.indent();
    for v in &e.variants {
        let vname = to_snake_case(&v.name);
        match &v.ty {
            TypeRef::Primitive(Primitive::Null)                  => {}
            TypeRef::Primitive(Primitive::Tstr)                  => { w.line(&format!("json_str_t {vname};")); }
            TypeRef::Primitive(Primitive::Bstr)                  => { w.line(&format!("json_bytes_t {vname};")); }
            TypeRef::Primitive(Primitive::Any)                   => { w.line(&format!("json_any_t {vname};")); }
            TypeRef::Primitive(p)                                => { w.line(&format!("{} {vname};", primitive_to_c(p))); }
            TypeRef::Named(_) if indirect(&e.name, &v.ty, module) => { w.line(&format!("{} *{vname};", ptr_type(&v.ty, module))); }
            TypeRef::Named(n)                                    => { w.line(&format!("{}_t {vname};", to_snake_case(n))); }
            _ => {}
        }
    }
    w.dedent();
    w.line("} value;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_array_decl_json(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    emit_c_doc(w, a.doc.as_deref());
    let type_name = to_snake_case(&a.name);
    let elem_ty   = typeref_to_c_json(&a.element);
    let cap       = capacity_value(
        a.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line("typedef struct {");
    w.indent();
    w.line(&format!("{elem_ty} items[{cap}];"));
    w.line("size_t count;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

/// A table in JSON: entries keyed by text, as JSON objects are (dgtlrift/cddlc#82).
fn emit_map_decl_json(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    emit_c_doc(w, m.doc.as_deref());
    let type_name = to_snake_case(&m.name);
    let cap = capacity_value(
        m.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line("typedef struct {");
    w.indent();
    w.line(&format!("struct {{ json_str_t key; {} value; }} entries[{cap}];", typeref_to_c_json(&m.val_ty)));
    w.line("size_t count;");
    w.dedent();
    w.line(&format!("}} {type_name}_t;"));
}

fn emit_alias_decl_json(w: &mut IndentWriter, a: &AliasDef) {
    emit_c_doc(w, a.doc.as_deref());
    let type_name = to_snake_case(&a.name);
    if a.tagged.is_some() || !a.constraints.is_empty() {
        let inner = typeref_to_c_json(&a.ty);
        w.line(&format!("typedef struct {{ {inner} value; }} {type_name}_t;"));
    } else {
        let inner = typeref_to_c_json(&a.ty);
        w.line(&format!("typedef {inner} {type_name}_t;"));
    }
}

fn emit_type_decl_h_json(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct_decl_json(w, s, module, opts),
        TypeDef::Enum(e)   => emit_enum_decl_json(w, e, module),
        TypeDef::Array(a)  => emit_array_decl_json(w, a, opts),
        TypeDef::Alias(a)  => emit_alias_decl_json(w, a),
        TypeDef::Map(m)    => emit_map_decl_json(w, m, opts),
    }
}

fn emit_header_json(module: &IrModule, opts: &CodegenOptions) -> String {
    let name     = module.name.as_str();
    let guard    = to_screaming_snake(&name);
    let mod_name = to_snake_case(&name);
    let mut w    = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line("/* Format: JSON (RFC 8259) */");
    w.line(&format!("#ifndef {guard}_JSON_H"));
    w.line(&format!("#define {guard}_JSON_H"));
    w.blank();
    w.line("#include <stdint.h>");
    w.line("#include <stdbool.h>");
    w.line("#include <stddef.h>");
    w.line("#include <stdlib.h>");
    w.line("#include <string.h>");
    w.line("#include \"cJSON.h\"");
    w.blank();

    // Base64url helpers declaration
    w.line("/* Base64url helpers (RFC 4648, no padding) */");
    w.line("char* cddlc_b64url_encode(const uint8_t* data, size_t len);");
    w.line("uint8_t* cddlc_b64url_decode(const char* s, size_t* out_len);");
    w.blank();

    w.line("/* JSON owned-value types */");
    w.line("typedef char json_str_t[256];");
    w.line("typedef struct { uint8_t data[512]; size_t len; } json_bytes_t;");
    w.line("/* A JSON value of any type (CDDL `any`): its text (dgtlrift/cddlc#178). */");
    w.line("typedef struct { char data[512]; size_t len; } json_any_t;");
    w.line("cJSON* cddlc_json_any_to(const json_any_t* a);");
    w.line("bool cddlc_json_any_from(const cJSON* j, json_any_t* out);");
    w.blank();

    // Types something points at are declared ahead (dgtlrift/cddlc#156).
    let targets = ptr_targets(module);
    for t in &targets {
        w.line(&format!("typedef struct {t}_s {t}_t;"));
    }
    if !targets.is_empty() { w.blank(); }
    for tname in cddlc_codegen::inline_decl_order(module) {
        let Some(def) = module.get(&tname) else { continue };
        let mut sub = IndentWriter::new(4);
        emit_type_decl_h_json(&mut sub, def, module, opts);
        for line in declare_tagged(sub.finish(), &to_snake_case(&tname), &targets).lines() {
            if line.is_empty() { w.blank() } else { w.line(line) }
        }
        w.blank();
    }

    // Function declarations
    for (_, def) in &module.types {
        emit_json_fn_decls(&mut w, def, &mod_name, opts);
        // What a recursive type's pointers point at is freed by its
        // `_release` (dgtlrift/cddlc#156).
        if needs_release(def.name(), module) {
            let t = to_snake_case(def.name());
            w.line(&format!("void {mod_name}_{t}_release({t}_t *val);"));
        }
        w.blank();
    }

    w.line(&format!("#endif /* {guard}_JSON_H */"));
    w.finish()
}

fn emit_json_fn_decls(w: &mut IndentWriter, def: &TypeDef, mod_name: &str, opts: &CodegenOptions) {
    let (name, ctype) = match def {
        TypeDef::Struct(s) => (s.name.as_str(), format!("{}_t", to_snake_case(&s.name))),
        TypeDef::Enum(e)   => (e.name.as_str(), format!("{}_t", to_snake_case(&e.name))),
        TypeDef::Array(a)  => (a.name.as_str(), format!("{}_t", to_snake_case(&a.name))),
        TypeDef::Alias(a)  => (a.name.as_str(), format!("{}_t", to_snake_case(&a.name))),
        TypeDef::Map(m)    => (m.name.as_str(), format!("{}_t", to_snake_case(&m.name))),
    };
    let snake = to_snake_case(&name);
    let pfx   = format!("{mod_name}_{snake}");
    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj);"));
    w.line(&format!("bool   {pfx}_from_json(const cJSON* j, {ctype}* out);"));
    w.line(&format!("char*  {pfx}_encode(const {ctype}* obj);  /* caller frees */"));
    w.line(&format!("bool   {pfx}_decode(const char* s, {ctype}* out);"));
    let _ = opts;
}

fn emit_source_json(module: &IrModule, opts: &CodegenOptions) -> String {
    let name     = module.name.as_str();
    let mod_name = to_snake_case(&name);
    let mut w    = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.h\""));
    w.blank();

    // Base64url implementation (RFC 4648 §5, no padding)
    w.line("static const char B64URL[] =");
    w.line("    \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_\";");
    w.blank();
    // `any`: a JSON value held as its text (dgtlrift/cddlc#178).
    w.line("/* The value an `any`'s text holds; `null` for an empty one. */");
    w.line("cJSON* cddlc_json_any_to(const json_any_t* a) {");
    w.line("    if (a->len == 0) return cJSON_CreateNull();");
    w.line("    return cJSON_ParseWithLength(a->data, a->len);");
    w.line("}");
    w.blank();
    w.line("/* Holds `j` as its text; false if the text doesn't fit. */");
    w.line("bool cddlc_json_any_from(const cJSON* j, json_any_t* out) {");
    w.line("    char* t = cJSON_PrintUnformatted(j);");
    w.line("    if (!t) return false;");
    w.line("    size_t n = strlen(t);");
    w.line("    bool fits = n <= sizeof(out->data);");
    w.line("    if (fits) { memcpy(out->data, t, n); out->len = n; }");
    w.line("    cJSON_free(t);");
    w.line("    return fits;");
    w.line("}");
    w.blank();
    w.line("char* cddlc_b64url_encode(const uint8_t* data, size_t len) {");
    w.indent();
    w.line("size_t out_len = ((len + 2) / 3) * 4;");
    w.line("char* out = (char*)malloc(out_len + 1);");
    w.line("if (!out) return NULL;");
    w.line("size_t i = 0, j = 0;");
    w.line("while (i + 2 < len) {");
    w.indent();
    w.line("out[j++] = B64URL[(data[i] >> 2) & 0x3F];");
    w.line("out[j++] = B64URL[((data[i] << 4) | (data[i+1] >> 4)) & 0x3F];");
    w.line("out[j++] = B64URL[((data[i+1] << 2) | (data[i+2] >> 6)) & 0x3F];");
    w.line("out[j++] = B64URL[data[i+2] & 0x3F];");
    w.line("i += 3;");
    w.dedent();
    w.line("}");
    w.line("if (i < len) {");
    w.indent();
    w.line("out[j++] = B64URL[(data[i] >> 2) & 0x3F];");
    w.line("if (i + 1 < len) { out[j++] = B64URL[((data[i] << 4) | (data[i+1] >> 4)) & 0x3F]; out[j++] = B64URL[(data[i+1] << 2) & 0x3F]; }");
    w.line("else { out[j++] = B64URL[(data[i] << 4) & 0x3F]; }");
    w.dedent();
    w.line("}");
    w.line("out[j] = '\\0';");
    w.line("return out;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("uint8_t* cddlc_b64url_decode(const char* s, size_t* out_len) {");
    w.indent();
    w.line("static const int8_t DEC[256] = {");
    w.line("    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,");
    w.line("    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,");
    w.line("    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,62,-1,-1,");
    w.line("    52,53,54,55,56,57,58,59,60,61,-1,-1,-1,-1,-1,-1,");
    w.line("    -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,");
    w.line("    15,16,17,18,19,20,21,22,23,24,25,-1,-1,-1,-1,63,");
    w.line("    -1,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,");
    w.line("    41,42,43,44,45,46,47,48,49,50,51,-1,-1,-1,-1,-1");
    w.line("};");
    w.line("size_t slen = strlen(s);");
    w.line("*out_len = (slen * 3) / 4;");
    w.line("uint8_t* out = (uint8_t*)malloc(*out_len + 1);");
    w.line("if (!out) return NULL;");
    w.line("size_t i = 0, j = 0;");
    w.line("while (i + 3 < slen) {");
    w.indent();
    w.line("int a = DEC[(uint8_t)s[i]], b = DEC[(uint8_t)s[i+1]];");
    w.line("int c = DEC[(uint8_t)s[i+2]], d = DEC[(uint8_t)s[i+3]];");
    w.line("if (a<0||b<0||c<0||d<0) break;");
    w.line("out[j++] = (a<<2)|(b>>4);");
    w.line("out[j++] = (b<<4)|(c>>2);");
    w.line("out[j++] = (c<<6)|d;");
    w.line("i += 4;");
    w.dedent();
    w.line("}");
    w.line("if (i+1 < slen) {");
    w.indent();
    w.line("int a = DEC[(uint8_t)s[i]], b = DEC[(uint8_t)s[i+1]];");
    w.line("if (a>=0&&b>=0) out[j++]=(a<<2)|(b>>4);");
    w.line("if (i+2 < slen) { int c=DEC[(uint8_t)s[i+2]]; if(b>=0&&c>=0) out[j++]=(b<<4)|(c>>2); }");
    w.dedent();
    w.line("}");
    w.line("*out_len = j;");
    w.line("return out;");
    w.dedent();
    w.line("}");
    w.blank();

    for (_, def) in &module.types {
        emit_json_type_impl(&mut w, def, module, &mod_name, opts);
        w.blank();
        emit_release_fn(&mut w, def, module, &mod_name, true);
    }

    w.finish()
}

fn emit_json_type_impl(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_json_struct_impl(w, s, module, mod_name, opts),
        TypeDef::Enum(e)   => emit_json_enum_impl(w, e, module, mod_name, opts),
        TypeDef::Array(a)  => emit_json_array_impl(w, a, flat_record(a, module).map(|r| r.fields.len()), mod_name, opts),
        TypeDef::Alias(a)  => emit_json_alias_impl(w, a, mod_name, opts),
        TypeDef::Map(m)    => emit_json_map_impl(w, m, mod_name, opts),
    }
}

fn json_value_expr(ty: &TypeRef, expr: &str, mod_name: &str) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Bool) => format!("cJSON_CreateBool({expr})"),
        TypeRef::Primitive(Primitive::Uint) => format!("cJSON_CreateNumber((double){expr})"),
        TypeRef::Primitive(Primitive::Int)  => format!("cJSON_CreateNumber((double){expr})"),
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16
            | Primitive::Float64 | Primitive::Float) => format!("cJSON_CreateNumber((double){expr})"),
        TypeRef::Primitive(Primitive::Tstr) => format!("cJSON_CreateString({expr})"),
        TypeRef::Primitive(Primitive::Bstr) =>
            format!("cJSON_CreateString(cddlc_b64url_encode(({expr}).data, ({expr}).len))"),
        TypeRef::Primitive(Primitive::Any) => format!("cddlc_json_any_to(&({expr}))"),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => "cJSON_CreateNull()".into(),
        TypeRef::Named(n) => {
            let snake = to_snake_case(&n);
            format!("{mod_name}_{snake}_to_json(&{expr})")
        }
        TypeRef::Tagged(_, inner) => json_value_expr(inner, expr, mod_name),
        TypeRef::Choice(_) => "cJSON_CreateNull()".into(),
    }
}

fn emit_json_struct_impl(w: &mut IndentWriter, s: &StructDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    let snake = to_snake_case(&s.name);
    let ctype = format!("{}_t", to_snake_case(&s.name));
    let pfx   = format!("{mod_name}_{snake}");

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_json_record_impl(w, s, module, mod_name, &pfx, &ctype, opts);
        emit_json_encode_decode(w, &pfx, &ctype);
        return;
    }

    // to_json
    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    w.line("cJSON* j = cJSON_CreateObject();");
    w.line("if (!j) return NULL;");
    for f in &s.fields {
        let fname    = to_snake_case(&f.name);
        let key_text = cddlc_codegen::field_key_json(&f.key);
        let key      = key_text.as_str();
        let access   = json_access(&s.name, f, module);
        let val_expr = json_value_expr(&f.ty, &access, mod_name);
        // A required pointer member must point at a value (dgtlrift/cddlc#156).
        if indirect(&s.name, &f.ty, module) && f.occurrence.is_required() {
            w.line(&format!("if (!obj->{fname}) {{ cJSON_Delete(j); return NULL; }}"));
        }
        match &f.occurrence {
            Occurrence::Optional => {
                let present_field = format!("{fname}_present");
                w.line(&format!("if (obj->{present_field}) cJSON_AddItemToObject(j, {key:?}, {val_expr});"));
            }
            _ => {
                w.line(&format!("cJSON_AddItemToObject(j, {key:?}, {val_expr});"));
            }
        }
    }
    if let Some(r) = &s.rest {
        let rname = to_snake_case(&r.name);
        let val_expr = json_value_expr(&r.val_ty, &format!("obj->{rname}[i].value"), mod_name);
        w.line(&format!("for (size_t i = 0; i < obj->{rname}_count; i++) cJSON_AddItemToObject(j, obj->{rname}[i].key, {val_expr});"));
    }
    w.line("return j;");
    w.dedent();
    w.line("}");
    w.blank();

    // from_json
    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    w.line("if (!cJSON_IsObject(j)) return false;");
    w.line("memset(out, 0, sizeof(*out));");
    // A map has only the members its schema gives it (dgtlrift/cddlc#102).
    if s.rest.is_none() {
        let named: Vec<String> = s.fields.iter().map(|f| format!("strcmp(k->string, {:?}) == 0", cddlc_codegen::field_key_json(&f.key))).collect();
        w.line(&format!(
            "{{ cJSON* k; cJSON_ArrayForEach(k, j) {{ if (!({})) return false; }} }}",
            if named.is_empty() { "0".into() } else { named.join(" || ") }
        ));
    }
    for f in &s.fields {
        let fname = to_snake_case(&f.name);
        let key_text = cddlc_codegen::field_key_json(&f.key);
        let key   = key_text.as_str();
        w.line(&format!("cJSON* _{fname} = cJSON_GetObjectItemCaseSensitive(j, {key:?});"));
        match &f.occurrence {
            Occurrence::Optional => {
                let present_field = format!("{fname}_present");
                w.line(&format!("if (_{fname}) {{"));
                w.indent();
                json_decode_member(w, &s.name, f, &format!("_{fname}"), module, mod_name, opts);
                w.line(&format!("out->{present_field} = true;"));
                w.dedent();
                w.line("}");
            }
            _ => {
                w.line(&format!("if (!_{fname}) return false;"));
                json_decode_member(w, &s.name, f, &format!("_{fname}"), module, mod_name, opts);
            }
        }
    }
    // Keys no member has are the map's other entries.
    if let Some(r) = &s.rest {
        let rname = to_snake_case(&r.name);
        let cap = capacity_value(
            r.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
            opts.max_array,
        );
        let named: Vec<String> = s.fields.iter().map(|f| format!("strcmp(item->string, {:?}) == 0", cddlc_codegen::field_key_json(&f.key))).collect();
        w.line("cJSON* item; cJSON_ArrayForEach(item, j) {");
        w.indent();
        if !named.is_empty() {
            w.line(&format!("if ({}) continue;", named.join(" || ")));
        }
        w.line(&format!("if (out->{rname}_count >= {cap}) return false;"));
        w.line(&format!("strncpy(out->{rname}[out->{rname}_count].key, item->string, sizeof(out->{rname}[out->{rname}_count].key) - 1);"));
        json_decode_field(w, &r.val_ty, &format!("{rname}[out->{rname}_count].value"), "item", mod_name, opts);
        w.line(&format!("out->{rname}_count++;"));
        w.dedent();
        w.line("}");
    }
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();

    emit_json_encode_decode(w, &pfx, &ctype);
    let _ = opts;
}

/// A record's to_json / from_json (RFC 8610 §3.4): a JSON array of the members'
/// values, in order. Optional members come last: one present after an absent
/// one can't be written, so to_json returns NULL.
fn emit_json_record_impl(w: &mut IndentWriter, s: &StructDef, module: &IrModule, mod_name: &str, pfx: &str, ctype: &str, opts: &CodegenOptions) {
    if let Some(ri) = cddlc_codegen::record_repeated(s) {
        emit_json_record_repeated(w, s, ri, module, mod_name, pfx, ctype, opts);
        return;
    }
    let optional: Vec<_> = s.fields.iter().filter(|f| matches!(f.occurrence, Occurrence::Optional)).collect();
    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    for pair in optional.windows(2) {
        w.line(&format!(
            "if (obj->{}_present && !obj->{}_present) return NULL;",
            to_snake_case(&pair[1].name), to_snake_case(&pair[0].name)
        ));
    }
    w.line("cJSON* j = cJSON_CreateArray();");
    w.line("if (!j) return NULL;");
    for f in &s.fields {
        let fname = to_snake_case(&f.name);
        let val_expr = json_value_expr(&f.ty, &json_access(&s.name, f, module), mod_name);
        // A required pointer member must point at a value (dgtlrift/cddlc#156).
        if indirect(&s.name, &f.ty, module) && f.occurrence.is_required() {
            w.line(&format!("if (!obj->{fname}) {{ cJSON_Delete(j); return NULL; }}"));
        }
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("if (obj->{fname}_present) cJSON_AddItemToArray(j, {val_expr});"));
        } else {
            w.line(&format!("cJSON_AddItemToArray(j, {val_expr});"));
        }
    }
    w.line("return j;");
    w.dedent();
    w.line("}");
    w.blank();

    let required = s.fields.iter().filter(|f| !matches!(f.occurrence, Occurrence::Optional)).count();
    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    w.line("if (!cJSON_IsArray(j)) return false;");
    w.line("int n = cJSON_GetArraySize(j);");
    w.line(&format!("if (n < {required} || n > {}) return false;", s.fields.len()));
    w.line("memset(out, 0, sizeof(*out));");
    // One optional member before required ones: present when the array holds
    // every member; the members after it move down one when it's absent
    // (dgtlrift/cddlc#134).
    let before = cddlc_codegen::record_optionals_before(s);
    let index = |i: usize| match &before {
        Some(b) if b[i] > 0 => { let x = required + b[i]; format!("(n >= {x} ? {i} : {i} - ({x} - n))") }
        _ => i.to_string(),
    };
    for (i, f) in s.fields.iter().enumerate() {
        let fname = to_snake_case(&f.name);
        if matches!(f.occurrence, Occurrence::Optional) {
            if let Some(b) = &before {
                w.line(&format!("if (n > {}) {{", required + b[i]));
            } else {
                w.line(&format!("if (n > {i}) {{"));
            }
            w.indent();
            w.line(&format!("cJSON* _{fname} = cJSON_GetArrayItem(j, {i});"));
            json_decode_member(w, &s.name, f, &format!("_{fname}"), module, mod_name, opts);
            w.line(&format!("out->{fname}_present = true;"));
            w.dedent();
            w.line("}");
        } else {
            w.line(&format!("cJSON* _{fname} = cJSON_GetArrayItem(j, {});", index(i)));
            json_decode_member(w, &s.name, f, &format!("_{fname}"), module, mod_name, opts);
        }
    }
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();
}

/// A record with a repeated member (`[primary, * extension, payload]`): its
/// items go inline, where it stands, and the array's length says how many
/// there are (dgtlrift/cddlc#134).
fn emit_json_record_repeated(w: &mut IndentWriter, s: &StructDef, ri: usize, module: &IrModule, mod_name: &str, pfx: &str, ctype: &str, opts: &CodegenOptions) {
    let rf = &s.fields[ri];
    let rname = to_snake_case(&rf.name);
    let fixed = s.fields.len() - 1;
    let (min, max) = cddlc_codegen::repeat_bounds(&rf.occurrence);
    let cap = capacity_value(rf.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)), opts.max_array);
    let bounds = |w: &mut IndentWriter, n: &str, fail: &str| {
        if min > 0 { w.line(&format!("if ({n} < {min}) return {fail};")); }
        if let Some(max) = max { w.line(&format!("if ({n} > {max}) return {fail};")); }
    };
    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    bounds(w, &format!("obj->{rname}_count"), "NULL");
    w.line("cJSON* j = cJSON_CreateArray();");
    w.line("if (!j) return NULL;");
    for f in &s.fields {
        let fname = to_snake_case(&f.name);
        if f.occurrence.is_repeated() {
            let val_expr = json_value_expr(&f.ty, &format!("obj->{fname}[i]"), mod_name);
            w.line(&format!("for (size_t i = 0; i < obj->{fname}_count; i++) cJSON_AddItemToArray(j, {val_expr});"));
        } else {
            let val_expr = json_value_expr(&f.ty, &json_access(&s.name, f, module), mod_name);
            w.line(&format!("cJSON_AddItemToArray(j, {val_expr});"));
        }
    }
    w.line("return j;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    w.line("if (!cJSON_IsArray(j)) return false;");
    w.line("int n = cJSON_GetArraySize(j);");
    w.line(&format!("if (n < {fixed}) return false;"));
    w.line(&format!("int reps = n - {fixed};"));
    w.line(&format!("if (reps > {cap}) return false;"));
    bounds(w, "reps", "false");
    w.line("memset(out, 0, sizeof(*out));");
    for (i, f) in s.fields.iter().enumerate() {
        let fname = to_snake_case(&f.name);
        if f.occurrence.is_repeated() {
            w.line("for (int k = 0; k < reps; k++) {");
            w.indent();
            w.line(&format!("cJSON* _{fname} = cJSON_GetArrayItem(j, {i} + k);"));
            json_decode_field(w, &f.ty, &format!("{fname}[k]"), &format!("_{fname}"), mod_name, opts);
            w.dedent();
            w.line("}");
            w.line(&format!("out->{fname}_count = (size_t)reps;"));
        } else {
            let index = if i < ri { i.to_string() } else { format!("{} + reps", i - 1) };
            w.line(&format!("cJSON* _{fname} = cJSON_GetArrayItem(j, {index});"));
            json_decode_member(w, &s.name, f, &format!("_{fname}"), module, mod_name, opts);
        }
    }
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();
}

/// A member as `to_json` reads it: through its pointer for a pointer member
/// (dgtlrift/cddlc#156).
fn json_access(owner: &str, f: &cddlc_ir::FieldDef, module: &IrModule) -> String {
    let fname = to_snake_case(&f.name);
    if indirect(owner, &f.ty, module) { format!("(*obj->{fname})") } else { format!("obj->{fname}") }
}

/// Reads a member from the JSON value `src`. A pointer member gets its memory
/// from the heap (cJSON's own values are on it too), freed by `_release`
/// (dgtlrift/cddlc#156).
fn json_decode_member(w: &mut IndentWriter, owner: &str, f: &cddlc_ir::FieldDef, src: &str, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    let fname = to_snake_case(&f.name);
    if !indirect(owner, &f.ty, module) {
        json_decode_field(w, &f.ty, &fname, src, mod_name, opts);
        return;
    }
    let n = named_of(&f.ty).unwrap_or_default();
    w.line(&format!("out->{fname} = calloc(1, sizeof *out->{fname});"));
    w.line(&format!("if (!out->{fname}) return false;"));
    w.line(&format!("if (!{mod_name}_{}_from_json({src}, out->{fname})) return false;", to_snake_case(n)));
}

/// `expr` as a C identifier fragment: `items[out->count]` → `items_out_count_`.
fn c_ident(expr: &str) -> String {
    expr.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

fn json_decode_field(w: &mut IndentWriter, ty: &TypeRef, dest: &str, src: &str, mod_name: &str, _opts: &CodegenOptions) {
    match ty {
        TypeRef::Primitive(Primitive::Bool) =>
            w.line(&format!("out->{dest} = cJSON_IsTrue({src});")),
        TypeRef::Primitive(Primitive::Uint) =>
            w.line(&format!("out->{dest} = (uint64_t)cJSON_GetNumberValue({src});")),
        TypeRef::Primitive(Primitive::Int) =>
            w.line(&format!("out->{dest} = (int64_t)cJSON_GetNumberValue({src});")),
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) =>
            w.line(&format!("out->{dest} = (float)cJSON_GetNumberValue({src});")),
        TypeRef::Primitive(Primitive::Float64 | Primitive::Float) =>
            w.line(&format!("out->{dest} = cJSON_GetNumberValue({src});")),
        // Temporaries are named for `dest`, as an identifier.
        TypeRef::Primitive(Primitive::Tstr) => {
            let v = c_ident(dest);
            w.line(&format!("const char* _s_{v} = cJSON_GetStringValue({src});"));
            w.line(&format!("if (_s_{v}) {{ strncpy(out->{dest}, _s_{v}, sizeof(out->{dest}) - 1); }}"));
        }
        TypeRef::Primitive(Primitive::Any) => {
            w.line(&format!("if (!cddlc_json_any_from({src}, &out->{dest})) return false;"));
        }
        TypeRef::Primitive(Primitive::Bstr) => {
            let v = c_ident(dest);
            w.line(&format!("const char* _b64_{v} = cJSON_GetStringValue({src});"));
            w.line(&format!("if (_b64_{v}) {{ size_t _bl; uint8_t* _bd = cddlc_b64url_decode(_b64_{v}, &_bl); if(_bd) {{ memcpy(out->{dest}.data, _bd, _bl); out->{dest}.len = _bl; free(_bd); }} }}"));
        }
        TypeRef::Named(n) => {
            let snake = to_snake_case(&n);
            w.line(&format!("{mod_name}_{snake}_from_json({src}, &out->{dest});"));
        }
        TypeRef::Tagged(_, inner) => json_decode_field(w, inner, dest, src, mod_name, _opts),
        _ => {}
    }
}

fn emit_json_enum_impl(w: &mut IndentWriter, e: &EnumDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    let snake = to_snake_case(&e.name);
    let ctype = format!("{}_t", to_snake_case(&e.name));
    let pfx   = format!("{mod_name}_{snake}");

    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    w.line("switch (obj->tag) {");
    for v in &e.variants {
        let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
        let vsnake = to_snake_case(&v.name);
        let cddl_str = cddlc_codegen::enum_variant_text(v);
        w.line(&format!("case {vconst}:"));
        w.indent();
        match &v.ty {
            // `null` is JSON null, not text (dgtlrift/cddlc#147).
            TypeRef::Primitive(Primitive::Null) => w.line("return cJSON_CreateNull();"),
            TypeRef::Primitive(Primitive::Tstr) =>
                w.line(&format!("return cJSON_CreateString({cddl_str:?});")),
            TypeRef::Primitive(Primitive::Uint) =>
                w.line(&format!("return cJSON_CreateNumber((double)obj->value.{vsnake});")),
            TypeRef::Primitive(Primitive::Int) =>
                w.line(&format!("return cJSON_CreateNumber((double)obj->value.{vsnake});")),
            TypeRef::Primitive(Primitive::Bool) =>
                w.line(&format!("return cJSON_CreateBool(obj->value.{vsnake});")),
            // The value an `any` holds as text (dgtlrift/cddlc#178).
            TypeRef::Primitive(Primitive::Any) =>
                w.line(&format!("return cddlc_json_any_to(&obj->value.{vsnake});")),
            // A variant of a named type (dgtlrift/cddlc#102).
            // A pointer alternative is read through it (dgtlrift/cddlc#156).
            TypeRef::Named(n) if indirect(&e.name, &v.ty, module) =>
                w.line(&format!("return {mod_name}_{}_to_json(obj->value.{vsnake});", to_snake_case(&n))),
            TypeRef::Named(n) =>
                w.line(&format!("return {mod_name}_{}_to_json(&obj->value.{vsnake});", to_snake_case(&n))),
            _ => w.line("return cJSON_CreateNull();"),
        }
        w.dedent();
    }
    w.line("default: return cJSON_CreateNull();");
    w.line("}");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    // A variant of a map or array type: the first whose decoder reads the value.
    for v in &e.variants {
        let TypeRef::Named(n) = &v.ty else { continue };
        let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
        let vsnake = to_snake_case(&v.name);
        if indirect(&e.name, &v.ty, module) {
            // A pointer alternative: memory from the heap, freed if it isn't
            // this one (dgtlrift/cddlc#156).
            let ptr = ptr_type(&v.ty, module);
            let target = resolve_named(n, module);
            let rel = if needs_release(target, module) { format!("{mod_name}_{}_release(p); ", to_snake_case(target)) } else { String::new() };
            w.line("if (cJSON_IsObject(j) || cJSON_IsArray(j)) {");
            w.indent();
            w.line(&format!("{ptr} *p = ({ptr} *)calloc(1, sizeof *p);"));
            w.line("if (!p) return false;");
            w.line(&format!(
                "if ({mod_name}_{}_from_json(j, p)) {{ out->value.{vsnake} = p; out->tag = {vconst}; return true; }}",
                to_snake_case(&n)
            ));
            w.line(&format!("{rel}free(p);"));
            w.dedent();
            w.line("}");
            continue;
        }
        w.line(&format!(
            "if ((cJSON_IsObject(j) || cJSON_IsArray(j)) && {mod_name}_{}_from_json(j, &out->value.{vsnake})) {{ out->tag = {vconst}; return true; }}",
            to_snake_case(&n)
        ));
    }
    for v in &e.variants {
        if matches!(&v.ty, TypeRef::Primitive(Primitive::Null)) {
            let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
            w.line(&format!("if (cJSON_IsNull(j)) {{ out->tag = {vconst}; return true; }}"));
        }
    }
    w.line("if (cJSON_IsString(j)) {");
    w.indent();
    w.line("const char* s = cJSON_GetStringValue(j);");
    for v in &e.variants {
        if matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)) {
            let vconst   = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
            let cddl_str = cddlc_codegen::enum_variant_text(v);
            w.line(&format!("if (strcmp(s, {cddl_str:?}) == 0) {{ out->tag = {vconst}; return true; }}"));
        }
    }
    // An `any` alternative takes what the others don't (dgtlrift/cddlc#178).
    let any = e.variants.iter().find(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Any)));
    if any.is_none() {
        w.line("return false;");
    }
    w.dedent();
    w.line("}");
    let has_num = e.variants.iter().any(|v|
        matches!(&v.ty, TypeRef::Primitive(Primitive::Uint | Primitive::Int)));
    if has_num {
        w.line("if (cJSON_IsNumber(j)) {");
        w.indent();
        for v in &e.variants {
            let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
            let vsnake = to_snake_case(&v.name);
            if matches!(&v.ty, TypeRef::Primitive(Primitive::Uint)) {
                w.line(&format!("out->tag = {vconst};"));
                w.line(&format!("out->value.{vsnake} = (uint64_t)cJSON_GetNumberValue(j);"));
                w.line("return true;");
                break;
            } else if matches!(&v.ty, TypeRef::Primitive(Primitive::Int)) {
                w.line(&format!("out->tag = {vconst};"));
                w.line(&format!("out->value.{vsnake} = (int64_t)cJSON_GetNumberValue(j);"));
                w.line("return true;");
                break;
            }
        }
        w.dedent();
        w.line("}");
    }
    match any {
        Some(v) => {
            let vconst = format!("{}_{}", to_screaming_snake(&e.name), to_screaming_snake(&v.name));
            w.line(&format!("if (!cddlc_json_any_from(j, &out->value.{})) return false;", to_snake_case(&v.name)));
            w.line(&format!("out->tag = {vconst}; return true;"));
        }
        None => w.line("return false;"),
    }
    w.dedent();
    w.line("}");
    w.blank();

    emit_json_encode_decode(w, &pfx, &ctype);
    let _ = opts;
}

fn emit_json_array_impl(w: &mut IndentWriter, a: &ArrayDef, flat: Option<usize>, mod_name: &str, opts: &CodegenOptions) {
    let snake = to_snake_case(&a.name);
    let ctype = format!("{}_t", to_snake_case(&a.name));
    let pfx   = format!("{mod_name}_{snake}");

    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    w.line("cJSON* arr = cJSON_CreateArray();");
    w.line("if (!arr) return NULL;");
    w.line("for (size_t i = 0; i < obj->count; i++) {");
    w.indent();
    let val_expr = json_value_expr(&a.element, "obj->items[i]", mod_name);
    if flat.is_some() {
        // A repeated group: each record's members, inline (dgtlrift/cddlc#90).
        w.line(&format!("cJSON* g = {val_expr};"));
        w.line("if (!g) { cJSON_Delete(arr); return NULL; }");
        w.line("cJSON* m;");
        w.line("while ((m = cJSON_DetachItemFromArray(g, 0)) != NULL) cJSON_AddItemToArray(arr, m);");
        w.line("cJSON_Delete(g);");
    } else {
        w.line(&format!("cJSON_AddItemToArray(arr, {val_expr});"));
    }
    w.dedent();
    w.line("}");
    w.line("return arr;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    w.line("if (!cJSON_IsArray(j)) return false;");
    w.line("out->count = 0;");
    if let (Some(n), TypeRef::Named(entry)) = (flat, &a.element) {
        // A repeated group: one record per run of its members.
        let entry_fn = format!("{mod_name}_{}_from_json", to_snake_case(entry));
        w.line("int n = cJSON_GetArraySize(j);");
        w.line(&format!("if (n % {n} != 0) return false;"));
        w.line(&format!("for (int i = 0; i < n && out->count < {}; i += {n}) {{", opts.max_array));
        w.indent();
        w.line("cJSON* g = cJSON_CreateArray();");
        w.line("if (!g) return false;");
        w.line(&format!("for (int k = 0; k < {n}; k++) cJSON_AddItemReferenceToArray(g, cJSON_GetArrayItem(j, i + k));"));
        w.line(&format!("bool ok = {entry_fn}(g, &out->items[out->count]);"));
        w.line("cJSON_Delete(g);");
        w.line("if (!ok) return false;");
        w.line("out->count++;");
        w.dedent();
        w.line("}");
        w.line("return true;");
        w.dedent();
        w.line("}");
        w.blank();
        emit_json_encode_decode(w, &pfx, &ctype);
        return;
    }
    w.line("cJSON* item; cJSON_ArrayForEach(item, j) {");
    w.indent();
    w.line(&format!("if (out->count >= {}) break;", opts.max_array));
    json_decode_field(w, &a.element, "items[out->count]", "item", mod_name, opts);
    w.line("out->count++;");
    w.dedent();
    w.line("}");
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();

    emit_json_encode_decode(w, &pfx, &ctype);
}

fn emit_json_map_impl(w: &mut IndentWriter, m: &MapDef, mod_name: &str, opts: &CodegenOptions) {
    let snake = to_snake_case(&m.name);
    let ctype = format!("{}_t", to_snake_case(&m.name));
    let pfx   = format!("{mod_name}_{snake}");
    let cap = capacity_value(
        m.occurrence.capacity().unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );

    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    w.line("cJSON* o = cJSON_CreateObject();");
    w.line("if (!o) return NULL;");
    w.line("for (size_t i = 0; i < obj->count; i++) {");
    w.indent();
    let val_expr = json_value_expr(&m.val_ty, "obj->entries[i].value", mod_name);
    w.line(&format!("cJSON_AddItemToObject(o, obj->entries[i].key, {val_expr});"));
    w.dedent();
    w.line("}");
    w.line("return o;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    w.line("if (!cJSON_IsObject(j)) return false;");
    w.line("out->count = 0;");
    w.line("cJSON* item; cJSON_ArrayForEach(item, j) {");
    w.indent();
    w.line(&format!("if (out->count >= {cap}) return false;"));
    w.line("strncpy(out->entries[out->count].key, item->string, sizeof(out->entries[out->count].key) - 1);");
    json_decode_field(w, &m.val_ty, "entries[out->count].value", "item", mod_name, opts);
    w.line("out->count++;");
    w.dedent();
    w.line("}");
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();

    emit_json_encode_decode(w, &pfx, &ctype);
}

fn emit_json_alias_impl(w: &mut IndentWriter, a: &AliasDef, mod_name: &str, opts: &CodegenOptions) {
    let snake      = to_snake_case(&a.name);
    let ctype      = format!("{}_t", to_snake_case(&a.name));
    let pfx        = format!("{mod_name}_{snake}");
    let is_newtype = a.tagged.is_some() || !a.constraints.is_empty();

    // to_json
    w.line(&format!("cJSON* {pfx}_to_json(const {ctype}* obj) {{"));
    w.indent();
    let inner_expr = if is_newtype { "obj->value".to_owned() } else { "(*obj)".to_owned() };
    let val_expr   = json_value_expr(&a.ty, &inner_expr, mod_name);
    w.line(&format!("return {val_expr};"));
    w.dedent();
    w.line("}");
    w.blank();

    // from_json
    w.line(&format!("bool {pfx}_from_json(const cJSON* j, {ctype}* out) {{"));
    w.indent();
    if is_newtype {
        json_decode_field(w, &a.ty, "value", "j", mod_name, opts);
    } else {
        // Simple typedef — write directly to *out
        match &a.ty {
            TypeRef::Primitive(Primitive::Uint) => {
                w.line("if (!cJSON_IsNumber(j)) return false;");
                w.line("*out = (uint64_t)cJSON_GetNumberValue(j);");
            }
            TypeRef::Primitive(Primitive::Int) => {
                w.line("if (!cJSON_IsNumber(j)) return false;");
                w.line("*out = (int64_t)cJSON_GetNumberValue(j);");
            }
            TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => {
                w.line("if (!cJSON_IsNumber(j)) return false;");
                w.line("*out = (float)cJSON_GetNumberValue(j);");
            }
            TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => {
                w.line("if (!cJSON_IsNumber(j)) return false;");
                w.line("*out = cJSON_GetNumberValue(j);");
            }
            TypeRef::Primitive(Primitive::Bool) => {
                w.line("*out = cJSON_IsTrue(j);");
            }
            TypeRef::Primitive(Primitive::Tstr) => {
                w.line("const char* _s = cJSON_GetStringValue(j);");
                w.line("if (!_s) return false;");
                w.line("strncpy(*out, _s, sizeof(*out) - 1);");
            }
            TypeRef::Primitive(Primitive::Any) => {
                w.line("if (!cddlc_json_any_from(j, out)) return false;");
            }
            TypeRef::Primitive(Primitive::Bstr) => {
                w.line("const char* _b64 = cJSON_GetStringValue(j);");
                w.line("if (!_b64) return false;");
                w.line("size_t _bl; uint8_t* _bd = cddlc_b64url_decode(_b64, &_bl);");
                w.line("if (!_bd) return false;");
                w.line("memcpy(out->data, _bd, _bl); out->len = _bl; free(_bd);");
            }
            _ => { w.line("(void)j;"); }
        }
    }
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.blank();

    emit_json_encode_decode(w, &pfx, &ctype);
    let _ = opts;
}

fn emit_json_encode_decode(w: &mut IndentWriter, pfx: &str, ctype: &str) {
    w.line(&format!("char* {pfx}_encode(const {ctype}* obj) {{"));
    w.indent();
    w.line(&format!("cJSON* j = {pfx}_to_json(obj);"));
    w.line("if (!j) return NULL;");
    w.line("char* s = cJSON_PrintUnformatted(j);");
    w.line("cJSON_Delete(j);");
    w.line("return s;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("bool {pfx}_decode(const char* s, {ctype}* out) {{"));
    w.indent();
    w.line("cJSON* j = cJSON_Parse(s);");
    w.line("if (!j) return false;");
    w.line(&format!("bool ok = {pfx}_from_json(j, out);"));
    w.line("cJSON_Delete(j);");
    w.line("return ok;");
    w.dedent();
    w.line("}");
}

fn emit_tests_json(module: &IrModule, opts: &CodegenOptions) -> String {
    let name     = module.name.as_str();
    let mod_name = to_snake_case(&name);
    let mut w    = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::CBlock) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.h\""));
    w.line("#include <stdio.h>");
    w.line("#include <assert.h>");
    w.blank();

    for (_, def) in &module.types {
        emit_json_type_test(&mut w, def, module, &mod_name, opts);
    }

    w.blank();
    w.line("int main(void) {");
    w.indent();
    for (_, def) in &module.types {
        let tname = match def {
            TypeDef::Struct(s) => s.name.as_str(),
            TypeDef::Enum(e)   => e.name.as_str(),
            TypeDef::Array(a)  => a.name.as_str(),
            TypeDef::Alias(a)  => a.name.as_str(),
            TypeDef::Map(m)    => m.name.as_str(),
        };
        let snake = to_snake_case(&tname);
        w.line(&format!("test_{mod_name}_{snake}();"));
    }
    w.line("printf(\"All JSON roundtrip tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}

fn emit_json_type_test(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, mod_name: &str, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => {
            let snake = to_snake_case(&s.name);
            let ctype = format!("{}_t", to_snake_case(&s.name));
            let pfx   = format!("{mod_name}_{snake}");
            w.line(&format!("static void test_{pfx}(void) {{"));
            w.indent();
            w.line(&format!("{ctype} original = {{0}};"));
            // A required pointer member points at a zero value of its type
            // (dgtlrift/cddlc#156).
            for f in s.fields.iter().filter(|f| f.occurrence.is_required() && indirect(&s.name, &f.ty, module)) {
                let fname = to_snake_case(&f.name);
                w.line(&format!("static {} _{fname}_target;", ptr_type(&f.ty, module)));
                w.line(&format!("original.{fname} = &_{fname}_target;"));
            }
            // A repeated member gets as many (zeroed) items as it needs
            // (dgtlrift/cddlc#134).
            for f in s.fields.iter().filter(|f| f.occurrence.is_repeated()) {
                let min = cddlc_codegen::repeat_bounds(&f.occurrence).0;
                if min > 0 {
                    w.line(&format!("original.{}_count = {min};", to_snake_case(&f.name)));
                }
            }
            w.line(&format!("char* encoded = {pfx}_encode(&original);"));
            w.line("assert(encoded != NULL);");
            w.line(&format!("{ctype} decoded = {{0}};"));
            w.line(&format!("assert({pfx}_decode(encoded, &decoded));"));
            w.line("free(encoded);");
            if needs_release(&s.name, module) {
                w.line(&format!("{pfx}_release(&decoded);"));
            }
            w.dedent();
            w.line("}");
            w.blank();
        }
        TypeDef::Enum(e) => {
            let snake = to_snake_case(&e.name);
            let ctype = format!("{}_t", to_snake_case(&e.name));
            let pfx   = format!("{mod_name}_{snake}");
            w.line(&format!("static void test_{pfx}(void) {{"));
            w.indent();
            w.line(&format!("{ctype} original = {{0}};"));
            // A first alternative that's a pointer points at a zero value of
            // its type (dgtlrift/cddlc#156).
            if let Some(v) = e.variants.first().filter(|v| indirect(&e.name, &v.ty, module)) {
                w.line(&format!("static {} _target;", ptr_type(&v.ty, module)));
                w.line(&format!("original.value.{} = &_target;", to_snake_case(&v.name)));
            }
            w.line(&format!("char* encoded = {pfx}_encode(&original);"));
            w.line("assert(encoded != NULL);");
            w.line(&format!("{ctype} decoded = {{0}};"));
            w.line(&format!("assert({pfx}_decode(encoded, &decoded));"));
            w.line("free(encoded);");
            if needs_release(&e.name, module) {
                w.line(&format!("{pfx}_release(&decoded);"));
            }
            w.dedent();
            w.line("}");
            w.blank();
        }
        TypeDef::Array(a) => {
            let snake = to_snake_case(&a.name);
            let ctype = format!("{}_t", to_snake_case(&a.name));
            let pfx   = format!("{mod_name}_{snake}");
            w.line(&format!("static void test_{pfx}(void) {{"));
            w.indent();
            w.line(&format!("{ctype} original = {{0}};"));
            w.line(&format!("char* encoded = {pfx}_encode(&original);"));
            w.line("assert(encoded != NULL);");
            w.line(&format!("{ctype} decoded = {{0}};"));
            w.line(&format!("assert({pfx}_decode(encoded, &decoded));"));
            w.line("free(encoded);");
            w.dedent();
            w.line("}");
            w.blank();
        }
        TypeDef::Alias(a) => {
            let snake = to_snake_case(&a.name);
            let ctype = format!("{}_t", to_snake_case(&a.name));
            let pfx   = format!("{mod_name}_{snake}");
            w.line(&format!("static void test_{pfx}(void) {{"));
            w.indent();
            w.line(&format!("{ctype} original = {{0}};"));
            w.line(&format!("char* encoded = {pfx}_encode(&original);"));
            w.line("assert(encoded != NULL);");
            w.line(&format!("{ctype} decoded = {{0}};"));
            w.line(&format!("assert({pfx}_decode(encoded, &decoded));"));
            w.line("free(encoded);");
            w.dedent();
            w.line("}");
            w.blank();
        }
        TypeDef::Map(m) => {
            let snake = to_snake_case(&m.name);
            let ctype = format!("{}_t", to_snake_case(&m.name));
            let pfx   = format!("{mod_name}_{snake}");
            w.line(&format!("static void test_{pfx}(void) {{"));
            w.indent();
            w.line(&format!("{ctype} original = {{0}};"));
            // A value that holds a pointer has no valid zero value: the
            // table stays empty (dgtlrift/cddlc#156).
            let one = named_of(&m.val_ty).is_none_or(|n| !needs_release(n, module));
            if one {
                w.line("strcpy(original.entries[0].key, \"k\");");
                w.line("original.count = 1;");
            }
            w.line(&format!("char* encoded = {pfx}_encode(&original);"));
            w.line("assert(encoded != NULL);");
            w.line(&format!("{ctype} decoded = {{0}};"));
            w.line(&format!("assert({pfx}_decode(encoded, &decoded));"));
            if one {
                w.line("assert(decoded.count == 1 && strcmp(decoded.entries[0].key, \"k\") == 0);");
            }
            w.line("free(encoded);");
            w.dedent();
            w.line("}");
            w.blank();
        }
    }
    let _ = opts;
}
