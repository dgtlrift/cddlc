/// C++ backend for cddlc.
///
/// Emits a C++17 library crate wrapping the C nanocbor runtime:
///   <module>-cbor/
///     CMakeLists.txt
///     include/<module>.hpp    ← types in namespace, std::optional, std::variant
///     src/<module>.cpp        ← encode/decode implementations
///     test/test_roundtrip.cpp ← roundtrip tests
use cddlc_codegen::{
    capacity_value, constraint_to_cpp_check, to_pascal_case, to_screaming_snake, Backend,
    CodegenError, CodegenOptions, CodegenOutput, GeneratedFile, IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Capacity, Constraint, EnumDef, EnumVariant, IrModule, LiteralValue, MapDef,
    Occurrence, Primitive, StructDef, TypeDef, TypeRef,
};

pub struct CppBackend;

impl Backend for CppBackend {
    fn language_name(&self) -> &str {
        "C++"
    }
    fn file_extension(&self) -> &str {
        "hpp"
    }

    fn generate(
        &self,
        module: &IrModule,
        opts: &CodegenOptions,
    ) -> Result<CodegenOutput, CodegenError> {
        cddlc_codegen::check_rest_keys(module, opts, "C++", true)?;
        // A rule named like a runtime type (`ErrorCode`) is `<module>-<rule>`.
        let renamed =
            cddlc_codegen::rename_types(module, &cddlc_codegen::cpp_reserved_renames(module));
        let module = &renamed;
        if let Some(msg) = unsupported_recursion(module, opts) {
            return Err(CodegenError::new(msg));
        }
        // The members and variants held in a `Box` (dgtlrift/cddlc#156).
        BOXED.with(|b| *b.borrow_mut() = boxed_items(module, opts));
        let name = module.name.as_str();
        let fmt_tag = if opts.is_json() { "json" } else { "cbor" };
        let crate_dir = format!("{name}-{fmt_tag}-cpp");

        if opts.is_json() {
            return Ok(CodegenOutput {
                files: vec![
                    GeneratedFile {
                        path: format!("{crate_dir}/CMakeLists.txt"),
                        content: emit_cmake_json_cpp(name, opts),
                    },
                    GeneratedFile {
                        path: format!("{crate_dir}/include/{name}.hpp"),
                        content: emit_header_json_cpp(module, opts),
                    },
                    GeneratedFile {
                        path: format!("{crate_dir}/src/{name}.cpp"),
                        content: emit_source_json_cpp(module, opts),
                    },
                    GeneratedFile {
                        path: format!("{crate_dir}/test/test_roundtrip.cpp"),
                        content: emit_tests_json_cpp(module, opts),
                    },
                ],
            });
        }

        Ok(CodegenOutput {
            files: vec![
                GeneratedFile {
                    path: format!("{crate_dir}/CMakeLists.txt"),
                    content: emit_cmake(name, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/include/{name}.hpp"),
                    content: emit_header(module, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/compat/endian.h"),
                    content: nanocbor_macos_compat_h(opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/src/{name}.cpp"),
                    content: emit_source(module, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/test/test_roundtrip.cpp"),
                    content: emit_tests(module, opts),
                },
            ],
        })
    }
}

// ── macOS compat header ───────────────────────────────────────────────────────

/// Compatibility shim for nanocbor on macOS — see backend-c for details.

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
    format!(
        r#"{header}cmake_minimum_required(VERSION 3.17)
project({name}_cbor_cpp C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

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

add_library({name}_cbor_cpp
    src/{name}.cpp
    ${{NANOCBOR_SOURCES}}
)
target_include_directories({name}_cbor_cpp PUBLIC include ${{nanocbor_SOURCE_DIR}}/include)

enable_testing()
add_executable(test_roundtrip test/test_roundtrip.cpp)
target_link_libraries(test_roundtrip {name}_cbor_cpp)
add_test(NAME roundtrip COMMAND test_roundtrip)
"#
    )
}

// ── Header file ───────────────────────────────────────────────────────────────

fn emit_header(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let guard = format!("{}_HPP", to_screaming_snake(&name));
    let ns = cddlc_codegen::cpp_ident(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line(&format!("#ifndef {guard}"));
    w.line(&format!("#define {guard}"));
    w.blank();
    w.line("#include <algorithm>");
    w.line("#include <cstdint>");
    w.line("#include <cstddef>");
    w.line("#include <optional>");
    w.line("#include <variant>");
    w.line("#include <new>");
    w.line("#include <utility>");
    w.line("#include <string_view>");
    w.line("#include <array>");
    w.line("#include <vector>");
    w.line("#include <cstring>");
    if module_uses_regex(module) {
        w.line("#include <regex>");
    }
    w.line("#include <nanocbor/nanocbor.h>");
    w.blank();

    w.line(&format!("namespace {ns} {{"));
    w.blank();

    // ── Error codes ───────────────────────────────────────────────────────────
    w.line("enum class ErrorCode {");
    w.indent();
    w.line("Ok = 0,");
    w.line("UnexpectedType,");
    w.line("UnexpectedValue,");
    w.line("MissingField,");
    w.line("Overflow,");
    w.line("InvalidUtf8,");
    w.line("UnknownField,");
    w.line("TagMismatch,");
    w.line("ValidationFailed,");
    // A recursive member's memory couldn't be had (dgtlrift/cddlc#156).
    w.line("AllocFailed,");
    w.dedent();
    w.line("};");
    w.blank();

    w.line("struct FieldError {");
    w.indent();
    w.line("ErrorCode   code;");
    w.line("const char *field;");
    w.line("std::size_t index{0};");
    w.dedent();
    w.line("};");
    w.blank();

    // ── Callbacks ─────────────────────────────────────────────────────────────
    w.line("struct Callbacks {");
    w.indent();
    w.line("void (*on_error)(void *ctx, FieldError err){nullptr};");
    w.line("void (*on_unknown_field)(void *ctx, std::string_view key){nullptr};");
    // A recursive member's memory when decoding without `--alloc heap`; it's
    // given back through `release` when its `Box` goes (dgtlrift/cddlc#156).
    w.line("void *(*alloc)(void *ctx, std::size_t size){nullptr};");
    w.line("void (*release)(void *ctx, void *ptr){nullptr};");
    w.dedent();
    w.line("};");
    w.blank();

    if cddlc_codegen::module_uses_any(module) {
        // `any`: the item's encoding, in the decode buffer (dgtlrift/cddlc#178).
        w.line("/// Writes an encoded data item (an `any` member) as is, as nanocbor");
        w.line("/// writes its own bytes.");
        w.line("inline int cddlc_put_item(nanocbor_encoder_t *enc, std::string_view item) {");
        w.indent();
        w.line("enc->len += item.size();");
        w.line("if (!enc->fits(enc, enc->context, item.size())) return NANOCBOR_ERR_END;");
        w.line("enc->append(enc, enc->context, reinterpret_cast<const uint8_t *>(item.data()), item.size());");
        w.line("return static_cast<int>(item.size());");
        w.dedent();
        w.line("}");
        w.blank();
    }

    // ── Forward declarations ──────────────────────────────────────────────────
    w.line("// Forward declarations");
    for (_, def) in &module.types {
        match def {
            // Arrays and tables are structs too, which a `Box` may hold
            // (dgtlrift/cddlc#156).
            TypeDef::Struct(_) | TypeDef::Enum(_) | TypeDef::Array(_) | TypeDef::Map(_) => {
                w.line(&format!("struct {};", to_pascal_case(def.name())));
            }
            _ => {}
        }
    }
    w.blank();

    // A type that holds itself again does so through a `Box`
    // (dgtlrift/cddlc#156).
    let boxes = BOXED.with(|b| !b.borrow().is_empty());
    if boxes {
        for line in BOX_TEMPLATE.lines() {
            if line.is_empty() {
                w.blank()
            } else {
                w.line(line)
            }
        }
        w.blank();
    }

    // ── Type declarations ─────────────────────────────────────────────────────
    // After what each holds by value.
    for tname in cddlc_codegen::inline_decl_order(module) {
        let Some(def) = module.get(&tname) else {
            continue;
        };
        emit_type_decl_hpp(&mut w, def, opts);
        w.blank();
    }

    // ── Function declarations ─────────────────────────────────────────────────
    w.line("// Encode/decode function declarations");
    for (_, def) in &module.types {
        let type_name = to_pascal_case(def.name());
        w.line(&format!(
            "int encode(nanocbor_encoder_t *enc, const {type_name} &val);"
        ));
        w.line(&format!(
            "int decode(nanocbor_value_t *dec, {type_name} &out, const Callbacks &cb = {{}}, void *ctx = nullptr);"
        ));
    }
    w.blank();
    // A `Box` is encoded through it, and decoded into memory from the heap
    // (`--alloc heap`) or `Callbacks::alloc` (dgtlrift/cddlc#156).
    if boxes {
        w.line("template <class T>");
        w.line("int encode(nanocbor_encoder_t *enc, const Box<T> &val) { return val ? encode(enc, *val) : -1; }");
        w.line("template <class T>");
        w.line("int decode(nanocbor_value_t *dec, Box<T> &out, const Callbacks &cb = {}, void *ctx = nullptr) {");
        w.indent();
        if is_heap(opts) {
            w.line("out = Box<T>::heap();");
        } else {
            w.line("out = Box<T>::in(cb.alloc, cb.release, ctx);");
            w.line("if (!out) { if (cb.on_error) cb.on_error(ctx, {ErrorCode::AllocFailed, \"Box\"}); return -1; }");
        }
        w.line("return decode(dec, *out, cb, ctx);");
        w.dedent();
        w.line("}");
        w.blank();
    }

    w.line(&format!("}} // namespace {ns}"));
    w.blank();
    w.line(&format!("#endif // {guard}"));
    w.finish()
}

fn emit_type_decl_hpp(w: &mut IndentWriter, def: &TypeDef, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct_hpp(w, s, opts),
        TypeDef::Enum(e) => emit_enum_hpp(w, e, opts),
        TypeDef::Array(a) => emit_array_hpp(w, a, opts),
        TypeDef::Alias(a) => emit_alias_hpp(w, a, opts),
        TypeDef::Map(m) => emit_map_hpp(w, m, opts),
    }
}

fn emit_struct_hpp(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    cddlc_codegen::emit_doc_lines(w, s.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
    let type_name = to_pascal_case(&s.name);
    w.line(&format!("struct {type_name} {{"));
    w.indent();
    for f in &s.fields {
        cddlc_codegen::emit_doc_lines(w, f.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
        // A member that holds this type again is a `Box`, empty when an
        // optional one is absent (dgtlrift/cddlc#156).
        let ty_str = if is_boxed(&s.name, &f.name) {
            format!("Box<{}>", cpp_typeref(&f.ty, opts))
        } else {
            cpp_field_type(&f.ty, &f.occurrence, opts)
        };
        let fname = cddlc_codegen::cpp_ident(&f.name);
        w.line(&format!("{ty_str} {fname}{};", cpp_pinned_init(f, false)));
        // A fixed-capacity repeated member says how many items it holds
        // (dgtlrift/cddlc#134).
        if f.occurrence.is_repeated() && !is_heap(opts) {
            w.line(&format!("std::size_t {fname}_count{{0}};"));
        }
    }
    // The map's other entries (dgtlrift/cddlc#82): any number with `--alloc
    // heap`, else up to their capacity.
    let mut eqs: Vec<String> = s.fields.iter()
        .map(|f| {
            let n = cddlc_codegen::cpp_ident(&f.name);
            if f.occurrence.is_repeated() && !is_heap(opts) {
                format!("{n}_count == o.{n}_count && std::equal({n}.begin(), {n}.begin() + {n}_count, o.{n}.begin())")
            } else {
                format!("{n} == o.{n}")
            }
        })
        .collect();
    if let Some(r) = &s.rest {
        let rname = cddlc_codegen::cpp_ident(&r.name);
        let entry = format!(
            "std::pair<{}, {}>",
            rest_key_cpp(r, opts),
            cpp_typeref(&r.val_ty, opts)
        );
        w.line("/// The map's other entries.");
        if is_heap(opts) {
            w.line(&format!("std::vector<{entry}> {rname}{{}};"));
            eqs.push(format!("{rname} == o.{rname}"));
        } else {
            let cap = capacity_value(
                r.occurrence
                    .capacity()
                    .unwrap_or(&Capacity::Bounded(opts.max_array)),
                opts.max_array,
            );
            w.line(&format!("std::array<{entry}, {cap}> {rname}{{}};"));
            w.line(&format!("std::size_t {rname}_count{{0}};"));
            eqs.push(format!(
                "{rname}_count == o.{rname}_count && std::equal({rname}.begin(), {rname}.begin() + {rname}_count, o.{rname}.begin())"
            ));
        }
    }
    w.blank();
    w.line(&format!(
        "bool operator==(const {type_name} &o) const noexcept {{"
    ));
    w.indent();
    if eqs.is_empty() {
        w.line("return true;");
    } else {
        w.line(&format!("return {};", eqs.join(" && ")));
    }
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("};");
}

fn emit_enum_hpp(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions) {
    cddlc_codegen::emit_doc_lines(w, e.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
    let type_name = to_pascal_case(&e.name);

    // The payload types of valued variants, deduplicated in order.
    let mut payloads: Vec<String> = Vec::new();
    for v in &e.variants {
        if let Some(t) = cpp_variant_payload_in(&e.name, v, opts) {
            if !payloads.contains(&t) {
                payloads.push(t);
            }
        }
    }
    let mut alternatives = vec!["std::monostate".to_owned()];
    alternatives.extend(payloads.iter().cloned());

    let kinds: Vec<String> = e.variants.iter().map(|v| to_pascal_case(&v.name)).collect();

    w.line(&format!("struct {type_name} {{"));
    w.indent();
    w.line("/// Which variant this value is.");
    w.line(&format!("enum class Kind {{ {} }};", kinds.join(", ")));
    w.line(&format!(
        "using Variant = std::variant<{}>;",
        alternatives.join(", ")
    ));
    w.blank();
    let first = e.variants.first();
    let first_kind = first.map(|v| to_pascal_case(&v.name));
    match first_kind {
        Some(k) => w.line(&format!("Kind kind{{Kind::{k}}};")),
        None => w.line("Kind kind{};"),
    }
    // The payload of a valued variant; `std::monostate` for text, nil and
    // integer-literal variants. Defaults to the first variant's payload.
    match first.and_then(|v| cpp_variant_payload_in(&e.name, v, opts)) {
        Some(t) => w.line(&format!("Variant value{{std::in_place_type<{t}>}};")),
        None => w.line("Variant value{};"),
    }
    w.blank();

    for v in &e.variants {
        let k = to_pascal_case(&v.name);
        match cpp_variant_payload_in(&e.name, v, opts) {
            None => w.line(&format!(
                "static {type_name} make_{k}() {{ return {type_name}{{Kind::{k}, Variant{{}}}}; }}"
            )),
            Some(t) => w.line(&format!(
                "static {type_name} make_{k}({t} v) {{ return {type_name}{{Kind::{k}, Variant(std::in_place_type<{t}>, std::move(v))}}; }}"
            )),
        }
    }
    w.blank();
    w.line(&format!(
        "bool operator==(const {type_name} &o) const noexcept {{ return kind == o.kind && value == o.value; }}"
    ));
    w.dedent();
    w.line("};");
}

/// `--alloc heap`: arrays are `std::vector`s; `stack` and `arena` keep a
/// fixed-capacity `std::array` plus a count.
fn is_heap(opts: &CodegenOptions) -> bool {
    matches!(opts.alloc, cddlc_codegen::AllocStrategy::Heap)
}

/// The C++ payload type of an enum variant, or `None` for a variant that carries
/// no value: a text literal, `nil`, or an integer literal (dgtlrift/cddlc#49).
fn cpp_variant_payload(v: &EnumVariant, opts: &CodegenOptions) -> Option<String> {
    cpp_variant_payload_in("", v, opts)
}

/// A variant's payload type, `Box<T>` for one that holds its enum again
/// (dgtlrift/cddlc#156).
fn cpp_variant_payload_in(owner: &str, v: &EnumVariant, opts: &CodegenOptions) -> Option<String> {
    if let TypeRef::Named(n) = &v.ty {
        if is_boxed(owner, &v.name) {
            return Some(format!("Box<{}>", to_pascal_case(n)));
        }
    }
    cpp_variant_payload_plain(v, opts)
}

fn cpp_variant_payload_plain(v: &EnumVariant, opts: &CodegenOptions) -> Option<String> {
    if cpp_int_literal(v).is_some() {
        return None;
    }
    match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) if cpp_text_literal(v).is_some() => None,
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => None,
        TypeRef::Primitive(p) => Some(cpp_primitive(p, opts).to_owned()),
        TypeRef::Named(n) => Some(to_pascal_case(n)),
        TypeRef::Tagged(..) | TypeRef::Choice(_) => None,
    }
}

/// The text literal of a text variant (`"free-space"`).
fn cpp_text_literal(v: &EnumVariant) -> Option<&str> {
    v.constraints.iter().find_map(|c| match c {
        Constraint::Eq(LiteralValue::Text(t)) => Some(t.as_str()),
        _ => None,
    })
}

/// The integer literal of an integer variant (the `2` in `1 / 2 / 3`).
fn cpp_int_literal(v: &EnumVariant) -> Option<i128> {
    match &v.ty {
        TypeRef::Primitive(Primitive::Uint | Primitive::Int) => {
            v.constraints.iter().find_map(|c| match c {
                Constraint::Eq(LiteralValue::Uint(n)) => Some(*n as i128),
                Constraint::Eq(LiteralValue::Int(n)) => Some(*n as i128),
                _ => None,
            })
        }
        _ => None,
    }
}

/// A C++ string literal and its byte length, for `nanocbor_put_tstrn`.
fn cpp_str_lit(s: &str) -> (String, usize) {
    (format!("{s:?}"), s.len())
}

fn emit_array_hpp(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
    let type_name = to_pascal_case(&a.name);
    let elem_ty = cpp_typeref(&a.element, opts);
    let cap = capacity_value(
        a.occurrence
            .capacity()
            .unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line(&format!("struct {type_name} {{"));
    w.indent();
    if is_heap(opts) {
        // `--alloc heap`: no capacity limit (dgtlrift/cddlc#50).
        w.line(&format!("std::vector<{elem_ty}> items{{}};"));
        w.blank();
        w.line(&format!(
            "bool operator==(const {type_name} &o) const noexcept {{ return items == o.items; }}"
        ));
        w.dedent();
        w.line("};");
        return;
    }
    w.line(&format!("std::array<{elem_ty}, {cap}> items{{}};"));
    w.line("std::size_t count{0};");
    w.blank();
    w.line(&format!(
        "bool operator==(const {type_name} &o) const noexcept {{"
    ));
    w.indent();
    w.line("if (count != o.count) return false;");
    w.line("for (std::size_t i = 0; i < count; i++) {");
    w.indent();
    w.line("if (!(items[i] == o.items[i])) return false;");
    w.dedent();
    w.line("}");
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("};");
}

/// A table (`{ * tstr => uint }`): key/value pairs, up to its capacity, or
/// any number with `--alloc heap` (dgtlrift/cddlc#82).
fn emit_map_hpp(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    cddlc_codegen::emit_doc_lines(w, m.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
    let type_name = to_pascal_case(&m.name);
    let entry = format!(
        "std::pair<{}, {}>",
        cpp_typeref(&m.key_ty, opts),
        cpp_typeref(&m.val_ty, opts)
    );
    w.line(&format!("struct {type_name} {{"));
    w.indent();
    if is_heap(opts) {
        w.line(&format!("std::vector<{entry}> entries{{}};"));
        w.blank();
        w.line(&format!("bool operator==(const {type_name} &o) const noexcept {{ return entries == o.entries; }}"));
        w.dedent();
        w.line("};");
        return;
    }
    let cap = capacity_value(
        m.occurrence
            .capacity()
            .unwrap_or(&Capacity::Bounded(opts.max_array)),
        opts.max_array,
    );
    w.line(&format!("std::array<{entry}, {cap}> entries{{}};"));
    w.line("std::size_t count{0};");
    w.blank();
    w.line(&format!(
        "bool operator==(const {type_name} &o) const noexcept {{"
    ));
    w.indent();
    w.line("if (count != o.count) return false;");
    w.line("for (std::size_t i = 0; i < count; i++) {");
    w.indent();
    w.line("if (!(entries[i] == o.entries[i])) return false;");
    w.dedent();
    w.line("}");
    w.line("return true;");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("};");
}

fn emit_alias_hpp(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Line("///"));
    let type_name = to_pascal_case(&a.name);
    // Every alias is its own struct, never a `using`: two aliases of one C++ type
    // (`prim-tstr = tstr`, `prim-bstr = bstr`, both `std::string_view`) would
    // otherwise share encode/decode overloads (dgtlrift/cddlc#25), and the type
    // also records which CBOR type its value is written as.
    {
        let inner = cpp_typeref(&a.ty, opts);
        w.line(&format!("struct {type_name} {{"));
        w.indent();
        // Generate a valid default for size-constrained string fields so that
        // default-constructed instances satisfy constraints and roundtrip cleanly.
        let size_n = a.constraints.iter().find_map(|c| match c {
            Constraint::SizeExact(n) => Some(*n),
            Constraint::SizeRange { min: Some(n), .. } => Some(*n),
            _ => None,
        });
        let value_default = match (&a.ty, size_n) {
            (TypeRef::Primitive(Primitive::Tstr), Some(n)) => {
                let s = "a".repeat(n);
                format!("value{{std::string_view({:?}, {n})}}", s)
            }
            // An `any` is the `null` item by default (an empty one isn't one).
            (TypeRef::Primitive(Primitive::Any), _) => format!("value{CPP_NULL_ITEM}"),
            // A sized byte string: that many zero bytes, from a string
            // literal, which outlives the view (`bytes .size 16`, RFC 9164).
            (TypeRef::Primitive(Primitive::Bstr), Some(n)) => {
                format!("value{{std::string_view(\"{}\", {n})}}", "\\x00".repeat(n))
            }
            _ => "value{}".into(),
        };
        w.line(&format!("{inner} {value_default};"));
        w.line(&format!(
            "bool operator==(const {type_name} &o) const noexcept {{ return value == o.value; }}"
        ));
        w.dedent();
        w.line("};");
    }
}

// ── Source file ───────────────────────────────────────────────────────────────

fn emit_source(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = cddlc_codegen::cpp_ident(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.hpp\""));
    w.blank();
    w.line(&format!("namespace {ns} {{"));
    w.blank();

    for (_, def) in &module.types {
        w.blank();
        emit_encode_fn(&mut w, def, module, opts);
        w.blank();
        emit_decode_fn(&mut w, def, module, opts);
    }

    w.blank();
    w.line(&format!("}} // namespace {ns}"));
    w.finish()
}

/// The record an array of a repeated group holds, whose members sit inline
/// in the array (dgtlrift/cddlc#90).
fn flat_record<'m>(a: &ArrayDef, module: &'m IrModule) -> Option<&'m cddlc_ir::StructDef> {
    match (&a.element, a.flat) {
        (TypeRef::Named(n), true) => match module.get(n) {
            Some(TypeDef::Struct(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

/// Reads one array element into `dest`: for a repeated group, each member in turn.
fn emit_cpp_array_item_decode(
    w: &mut IndentWriter,
    a: &ArrayDef,
    module: &IrModule,
    dest: &str,
    opts: &CodegenOptions,
) {
    match flat_record(a, module) {
        Some(r) => {
            for f in &r.fields {
                emit_cpp_decode_field(
                    w,
                    &f.ty,
                    &format!("{dest}.{}", cddlc_codegen::cpp_ident(&f.name)),
                    &f.constraints,
                    &f.name,
                    "&arr",
                    opts,
                );
            }
        }
        None => emit_cpp_decode_field(w, &a.element, dest, &[], "item", "&arr", opts),
    }
}

/// A `std::string_view` of the bytes `b` (a byte-string literal).
fn cpp_bytes_view(b: &[u8]) -> String {
    let esc: String = b.iter().map(|x| format!("\\x{x:02x}")).collect();
    format!("std::string_view(\"{esc}\", {})", b.len())
}

/// A repeated member's item count, in `obj` (`val`, `out`).
fn repeat_len(f: &cddlc_ir::FieldDef, obj: &str, opts: &CodegenOptions) -> String {
    let n = cddlc_codegen::cpp_ident(&f.name);
    if is_heap(opts) {
        format!("{obj}.{n}.size()")
    } else {
        format!("{obj}.{n}_count")
    }
}

/// Fails (returns -1) unless a repeated member's item count, `n`, is within
/// its bounds (dgtlrift/cddlc#134).
fn emit_cpp_repeat_check(w: &mut IndentWriter, f: &cddlc_ir::FieldDef, n: &str) {
    let (min, max) = cddlc_codegen::repeat_bounds(&f.occurrence);
    if min > 0 {
        w.line(&format!(
            "if ({n} < {min}) return -1; // too few '{}' items",
            f.name
        ));
    }
    if let Some(max) = max {
        w.line(&format!(
            "if ({n} > {max}) return -1; // too many '{}' items",
            f.name
        ));
    }
}

// ── Recursion (dgtlrift/cddlc#156) ───────────────────────────────────────────

thread_local! {
    /// The members and variants held in a `Box`, as (owner, item) names: set
    /// for each `generate`.
    static BOXED: std::cell::RefCell<std::collections::HashSet<(String, String)>> = Default::default();
}

/// Each member and variant that holds its owner again, inline: held in a
/// `Box`, as a type can't contain itself.
fn boxed_items(
    module: &IrModule,
    opts: &CodegenOptions,
) -> std::collections::HashSet<(String, String)> {
    let mut out = std::collections::HashSet::new();
    for (name, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                for f in &s.fields {
                    if matches!(f.occurrence, Occurrence::Required | Occurrence::Optional)
                        && boxed_named(&f.ty, opts).is_some()
                        && cddlc_codegen::is_indirect(name, &f.ty, module)
                    {
                        out.insert((name.to_string(), f.name.to_string()));
                    }
                }
            }
            TypeDef::Enum(e) => {
                for v in &e.variants {
                    if matches!(v.ty, TypeRef::Named(_))
                        && cddlc_codegen::is_indirect(name, &v.ty, module)
                    {
                        out.insert((name.to_string(), v.name.to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The named type a recursive member's `Box` holds: the member's own, or a
/// choice's first alternative, which is how CBOR holds a choice that isn't
/// of primitives (see `cpp_typeref`). JSON holds such a choice as an
/// `nlohmann::json`, which needs no `Box`.
fn boxed_named<'a>(ty: &'a TypeRef, opts: &CodegenOptions) -> Option<&'a str> {
    match ty {
        TypeRef::Named(n) => Some(n.as_str()),
        TypeRef::Choice(cs) if !opts.is_json() && prim_choice(cs, opts).is_none() => {
            match cs.first() {
                Some(TypeRef::Named(n)) => Some(n.as_str()),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether `owner`'s member or variant `item` is held in a `Box`.
fn is_boxed(owner: &str, item: &str) -> bool {
    BOXED.with(|b| b.borrow().contains(&(owner.to_owned(), item.to_owned())))
}

/// A recursive member C++ can't hold yet: repeated, of other entries, under
/// a tag, or of an inline choice.
fn unsupported_recursion(module: &IrModule, opts: &CodegenOptions) -> Option<String> {
    for (name, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                for f in &s.fields {
                    if !cddlc_codegen::is_indirect(name, &f.ty, module) {
                        continue;
                    }
                    if f.occurrence.is_repeated() {
                        return Some(format!("'{name}.{}': C++ can't hold a repeated member that holds '{name}' again yet; use an array type for it (dgtlrift/cddlc#156)", f.name));
                    }
                    // JSON holds an inline choice as an `nlohmann::json`.
                    let json_choice = opts.is_json() && matches!(f.ty, TypeRef::Choice(_));
                    if boxed_named(&f.ty, opts).is_none() && !json_choice {
                        return Some(format!("'{name}.{}': C++ can't hold a recursive member that isn't a named type yet (dgtlrift/cddlc#156)", f.name));
                    }
                }
                if let Some(r) = &s.rest {
                    if cddlc_codegen::is_indirect(name, &r.val_ty, module) {
                        return Some(format!("'{name}': C++ can't hold other entries whose value holds '{name}' again yet (dgtlrift/cddlc#156)"));
                    }
                }
            }
            TypeDef::Enum(e) => {
                for v in &e.variants {
                    if cddlc_codegen::is_indirect(name, &v.ty, module)
                        && !matches!(v.ty, TypeRef::Named(_))
                    {
                        return Some(format!("'{name}': C++ can't hold a recursive alternative that isn't a named type yet (dgtlrift/cddlc#156)"));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// `Box<T>`: holds a value of a type that holds itself again, which a type
/// can't contain directly. A value type: copies deep, compares by value, and
/// empty is absent for an optional member. Its memory is from the heap, or
/// from `Callbacks::alloc` when decoding without `--alloc heap`.
const BOX_TEMPLATE: &str = r#"/// Holds a value of a type that holds itself again (dgtlrift/cddlc#156): a
/// value type (copies deep, compares by value) whose value lives elsewhere.
/// Empty is absent, for an optional member. Its memory is from the heap, or,
/// when decoding without `--alloc heap`, from `Callbacks::alloc`.
template <class T>
class Box {
public:
    Box() noexcept = default;
    Box(const T &v) : p_(new T(v)) {}
    Box(T &&v) : p_(new T(std::move(v))) {}
    Box(const Box &o) : p_(o.p_ ? new T(*o.p_) : nullptr) {}
    Box(Box &&o) noexcept : p_(o.p_), release_(o.release_), ctx_(o.ctx_), own_(o.own_) { o.p_ = nullptr; }
    Box &operator=(Box o) noexcept {
        std::swap(p_, o.p_); std::swap(release_, o.release_); std::swap(ctx_, o.ctx_); std::swap(own_, o.own_);
        return *this;
    }
    ~Box() { reset(); }

    /// A default value on the heap.
    static Box heap() { Box b; b.p_ = new T{}; return b; }
    /// A default value in memory from `alloc`, given back with `release`;
    /// empty if `alloc` gives none.
    static Box in(void *(*alloc)(void *, std::size_t), void (*release)(void *, void *), void *ctx) {
        Box b;
        void *m = alloc ? alloc(ctx, sizeof(T)) : nullptr;
        if (m) { b.p_ = new (m) T{}; b.release_ = release; b.ctx_ = ctx; b.own_ = false; }
        return b;
    }

    void reset() noexcept {
        if (!p_) return;
        if (own_) { delete p_; }
        else { p_->~T(); if (release_) release_(ctx_, p_); }
        p_ = nullptr;
    }
    bool has_value() const noexcept { return p_ != nullptr; }
    explicit operator bool() const noexcept { return p_ != nullptr; }
    T &operator*() { return *p_; }
    const T &operator*() const { return *p_; }
    T *operator->() { return p_; }
    const T *operator->() const { return p_; }
    /// The value; throws `std::bad_optional_access` when empty, as
    /// `std::optional` does.
    T &value() { if (!p_) throw std::bad_optional_access{}; return *p_; }
    const T &value() const { if (!p_) throw std::bad_optional_access{}; return *p_; }
    bool operator==(const Box &o) const { return (p_ && o.p_) ? *p_ == *o.p_ : p_ == o.p_; }
    bool operator!=(const Box &o) const { return !(*this == o); }

private:
    T *p_ = nullptr;
    void (*release_)(void *, void *) = nullptr;
    void *ctx_ = nullptr;
    bool own_ = true;
};"#;

/// A test value of `name`: its default, with each required boxed member
/// holding a default value, which an empty `Box` doesn't encode as
/// (dgtlrift/cddlc#156).
fn cpp_filled(name: &str, module: &IrModule) -> String {
    cpp_filled_at(name, module, 0, false)
}

/// [`cpp_filled`] for JSON, where an enum holds an `nlohmann::json` and its
/// default is fine.
fn cpp_filled_json(name: &str, module: &IrModule) -> String {
    cpp_filled_at(name, module, 0, true)
}

/// [`cpp_filled`] at `depth` levels in. An enum takes a variant with no
/// payload, if it has one, as its default may be a variant whose `Box` is
/// empty (`link = next / null` is `Link::make_Null()`; dgtlrift/cddlc#224);
/// a struct fills its required boxed members the same way, to a few levels.
fn cpp_filled_at(name: &str, module: &IrModule, depth: usize, json: bool) -> String {
    let t = to_pascal_case(name);
    match module.get(name) {
        Some(TypeDef::Enum(e)) if !json => {
            if let Some(v) = e
                .variants
                .iter()
                .find(|v| cpp_variant_payload(v, &CodegenOptions::default()).is_none())
            {
                return format!("{t}::make_{}()", to_pascal_case(&v.name));
            }
            // A variant holding a primitive needs no nesting (`expr = int /
            // binop` is `Int`).
            if let Some(v) = e
                .variants
                .iter()
                .find(|v| matches!(v.ty, TypeRef::Primitive(_)))
            {
                return format!("{t}::make_{}({{}})", to_pascal_case(&v.name));
            }
            // Every variant holds a named value: the first, filled, in its
            // `Box` when it's one (RFC 8927's mapping values).
            if depth < 4 {
                if let Some((v, n)) = e.variants.iter().find_map(|v| match &v.ty {
                    TypeRef::Named(n) => Some((v, n)),
                    _ => None,
                }) {
                    let inner = cpp_filled_at(n, module, depth + 1, json);
                    let arg = if is_boxed(&e.name, &v.name) {
                        format!("Box<{}>({inner})", to_pascal_case(n))
                    } else {
                        inner
                    };
                    return format!("{t}::make_{}({arg})", to_pascal_case(&v.name));
                }
            }
            return format!("{t}{{}}");
        }
        Some(TypeDef::Struct(_)) if depth < 4 => {}
        _ => return format!("{t}{{}}"),
    }
    let Some(TypeDef::Struct(s)) = module.get(name) else {
        return format!("{t}{{}}");
    };
    let sets: Vec<String> = s
        .fields
        .iter()
        .filter(|f| f.occurrence.is_required() && is_boxed(&s.name, &f.name))
        .filter_map(|f| {
            let n = match &f.ty {
                TypeRef::Named(n) => n,
                TypeRef::Choice(cs) => match cs.iter().find(|c| matches!(c, TypeRef::Named(_))) {
                    Some(TypeRef::Named(n)) => n,
                    _ => return None,
                },
                _ => return None,
            };
            Some(format!(
                "v.{} = {};",
                cddlc_codegen::cpp_ident(&f.name),
                cpp_filled_at(n, module, depth + 1, json)
            ))
        })
        .collect();
    if sets.is_empty() {
        format!("{t}{{}}")
    } else {
        format!("[] {{ {t} v{{}}; {} return v; }}()", sets.join(" "))
    }
}

/// The type a JSON member's `Box` holds. An array or table is an alias in
/// JSON, which can't be declared ahead, so the `Box` names the type it
/// stands for (`std::vector<Node>`), which needs no complete `Node` yet.
fn json_box_target(ty: &TypeRef, module: &IrModule, opts: &CodegenOptions) -> String {
    let mut n = match ty {
        TypeRef::Named(n) => n.as_str(),
        _ => return cpp_json_field_type(ty, opts),
    };
    for _ in 0..16 {
        match module.get(n) {
            Some(TypeDef::Alias(a)) => match &a.ty {
                TypeRef::Named(t) if a.constraints.is_empty() && a.tagged.is_none() => {
                    n = t.as_str()
                }
                _ => break,
            },
            _ => break,
        }
    }
    match module.get(n) {
        Some(TypeDef::Array(a)) => {
            format!("std::vector<{}>", cpp_json_field_type(&a.element, opts))
        }
        Some(TypeDef::Map(m)) => format!(
            "std::map<std::string, {}>",
            cpp_json_field_type(&m.val_ty, opts)
        ),
        _ => cpp_json_field_type(ty, opts),
    }
}

/// A member as JSON encoding reads it: through its `Box` when it has one
/// (dgtlrift/cddlc#156).
fn json_member(owner: &str, f: &cddlc_ir::FieldDef) -> String {
    let fname = cddlc_codegen::cpp_ident(&f.name);
    if is_boxed(owner, &f.name) {
        format!("this->{fname}.value()")
    } else {
        format!("this->{fname}")
    }
}

/// Whether a choice has a `nil` alternative.
fn choice_has_null(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Choice(cs) if cs.iter().any(|c| matches!(c, TypeRef::Primitive(Primitive::Null))))
}

/// Writes a boxed choice member (`children: info-list / null`), held as its
/// first alternative: the value, or `null` for an empty box when the choice
/// has a `nil` alternative (dgtlrift/cddlc#156).
fn emit_boxed_choice_encode(w: &mut IndentWriter, ty: &TypeRef, expr: &str) {
    if choice_has_null(ty) {
        w.line(&format!("if ({expr}) {{ if (encode(enc, {expr}) < 0) return -1; }} else {{ nanocbor_fmt_null(enc); }}"));
    } else {
        w.line(&format!("if (encode(enc, {expr}) < 0) return -1;"));
    }
}

/// Reads a boxed choice member: `null` leaves the box empty, when the choice
/// has a `nil` alternative; anything else is its first alternative.
fn emit_boxed_choice_decode(w: &mut IndentWriter, ty: &TypeRef, dest: &str, dec: &str) {
    if choice_has_null(ty) {
        w.line(&format!("if (nanocbor_get_null({dec}) < 0) {{ if (decode({dec}, {dest}, cb, ctx) < 0) return -1; }}"));
    } else {
        w.line(&format!(
            "if (decode({dec}, {dest}, cb, ctx) < 0) return -1;"
        ));
    }
}

/// The type an optional member decodes into before it's set: its own, or
/// its `Box` (dgtlrift/cddlc#156).
fn member_temp_type(s: &StructDef, f: &cddlc_ir::FieldDef, opts: &CodegenOptions) -> String {
    if is_boxed(&s.name, &f.name) {
        format!("Box<{}>", cpp_typeref(&f.ty, opts))
    } else {
        cpp_typeref(&f.ty, opts)
    }
}

/// Whether a map's other entries have plain `tstr` keys, held as
/// `std::string_view` (otherwise they hold their key type; dgtlrift/cddlc#136).
fn text_rest(r: &cddlc_ir::MapRest) -> bool {
    matches!(r.key_ty, TypeRef::Primitive(Primitive::Tstr))
}

/// The C++ type of a map's other entries' keys.
fn rest_key_cpp(r: &cddlc_ir::MapRest, opts: &CodegenOptions) -> String {
    if text_rest(r) {
        "std::string_view".into()
    } else {
        cpp_typeref(&r.key_ty, opts)
    }
}

/// Writes a map member's key: its text, or an integer key as an integer
/// (dgtlrift/cddlc#176).
fn cpp_key_write(key: &cddlc_ir::FieldKey) -> String {
    match key {
        cddlc_ir::FieldKey::Text(t) => format!("nanocbor_put_tstr(enc, {:?});", t.as_str()),
        cddlc_ir::FieldKey::Uint(n) => format!("nanocbor_fmt_uint(enc, {n}ULL);"),
        cddlc_ir::FieldKey::Int(n) if *n >= 0 => format!("nanocbor_fmt_uint(enc, {n}ULL);"),
        cddlc_ir::FieldKey::Int(n) => format!("nanocbor_fmt_int(enc, {n}LL);"),
    }
}

fn emit_encode_fn(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_pascal_case(def.name());
    w.line(&format!(
        "int encode(nanocbor_encoder_t *enc, const {type_name} &val) {{"
    ));
    w.indent();

    match def {
        TypeDef::Struct(s) => {
            if let Some(tag) = s.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            let fields: Vec<_> = if opts.dcbor {
                let mut fs: Vec<_> = s.fields.iter().collect();
                fs.sort_by(|a, b| cddlc_codegen::dcbor_key_order(a, b));
                fs
            } else {
                s.fields.iter().collect()
            };
            let req_count = fields
                .iter()
                .filter(|f| !matches!(f.occurrence, Occurrence::Optional))
                .count();
            let opt_fields: Vec<_> = fields
                .iter()
                .filter(|f| matches!(f.occurrence, Occurrence::Optional))
                .collect();
            // A record (RFC 8610 §3.4) is an array of the members' values, in
            // order; optional members come last, so one present after an absent
            // one can't be written.
            let record = s.encoding == cddlc_ir::StructEncoding::Array;
            if record {
                for pair in opt_fields.windows(2) {
                    w.line(&format!(
                        "if (val.{}.has_value() && !val.{}.has_value()) return -1;",
                        cddlc_codegen::cpp_ident(&pair[1].name),
                        cddlc_codegen::cpp_ident(&pair[0].name)
                    ));
                }
                let opt_sum: String = opt_fields
                    .iter()
                    .map(|f| {
                        format!(
                            " + (val.{}.has_value() ? 1u : 0u)",
                            cddlc_codegen::cpp_ident(&f.name)
                        )
                    })
                    .collect();
                // A repeated member's items go inline (dgtlrift/cddlc#134).
                match cddlc_codegen::record_repeated(s) {
                    Some(ri) => {
                        let rf = &s.fields[ri];
                        let n = repeat_len(rf, "val", opts);
                        emit_cpp_repeat_check(w, rf, &n);
                        w.line(&format!(
                            "nanocbor_fmt_array(enc, {}u + {n});",
                            req_count - 1
                        ));
                    }
                    None => w.line(&format!("nanocbor_fmt_array(enc, {req_count}u{opt_sum});")),
                }
            } else {
                let rest_count = match &s.rest {
                    Some(r) if is_heap(opts) => {
                        format!(" + val.{}.size()", cddlc_codegen::cpp_ident(&r.name))
                    }
                    Some(r) => format!(" + val.{}_count", cddlc_codegen::cpp_ident(&r.name)),
                    None => String::new(),
                };
                let opt_sum: String = opt_fields
                    .iter()
                    .map(|f| {
                        format!(
                            " + (val.{}.has_value() ? 1u : 0u)",
                            cddlc_codegen::cpp_ident(&f.name)
                        )
                    })
                    .collect();
                w.line(&format!(
                    "nanocbor_fmt_map(enc, {req_count}u{opt_sum}{rest_count});"
                ));
            }
            for f in &fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                if matches!(f.occurrence, Occurrence::Optional) {
                    w.line(&format!("if (val.{fname}.has_value()) {{"));
                    w.indent();
                    if !record {
                        w.line(&cpp_key_write(&f.key));
                    }
                    emit_cpp_encode_field(
                        w,
                        &f.ty,
                        &format!("(*val.{fname})"),
                        &f.occurrence,
                        opts,
                    );
                    w.dedent();
                    w.line("}");
                } else if record && f.occurrence.is_repeated() {
                    w.line(&format!(
                        "for (std::size_t i = 0; i < {}; i++) {{",
                        repeat_len(f, "val", opts)
                    ));
                    w.indent();
                    emit_cpp_encode_field(
                        w,
                        &f.ty,
                        &format!("val.{fname}[i]"),
                        &Occurrence::Required,
                        opts,
                    );
                    w.dedent();
                    w.line("}");
                } else {
                    if !record {
                        w.line(&cpp_key_write(&f.key));
                    }
                    if is_boxed(&s.name, &f.name) && matches!(f.ty, TypeRef::Choice(_)) {
                        emit_boxed_choice_encode(w, &f.ty, &format!("val.{fname}"));
                    } else {
                        emit_cpp_encode_field(
                            w,
                            &f.ty,
                            &format!("val.{fname}"),
                            &f.occurrence,
                            opts,
                        );
                    }
                }
            }
            if let Some(r) = &s.rest {
                let rname = cddlc_codegen::cpp_ident(&r.name);
                if is_heap(opts) {
                    w.line(&format!("for (const auto &e : val.{rname}) {{"));
                } else {
                    w.line(&format!(
                        "for (std::size_t i = 0; i < val.{rname}_count; i++) {{"
                    ));
                    w.line(&format!("    const auto &e = val.{rname}[i];"));
                }
                w.indent();
                if text_rest(r) {
                    w.line("nanocbor_put_tstrn(enc, e.first.data(), e.first.size());");
                } else {
                    emit_cpp_encode_field(w, &r.key_ty, "e.first", &Occurrence::Required, opts);
                }
                emit_cpp_encode_field(w, &r.val_ty, "e.second", &Occurrence::Required, opts);
                w.dedent();
                w.line("}");
            }
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Enum(e) => {
            if let Some(tag) = e.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            let type_name = to_pascal_case(&e.name);
            w.line("switch (val.kind) {");
            for v in &e.variants {
                let k = to_pascal_case(&v.name);
                w.line(&format!("case {type_name}::Kind::{k}:"));
                w.indent();
                let payload = cpp_variant_payload_in(&e.name, v, opts);
                let get = |t: &str| format!("std::get<{t}>(val.value)");
                match (&v.ty, payload.as_deref()) {
                    (_, None) if cpp_int_literal(v).is_some() => {
                        let n = cpp_int_literal(v).unwrap_or_default();
                        if n >= 0 {
                            w.line(&format!("nanocbor_fmt_uint(enc, {n}u);"));
                        } else {
                            w.line(&format!("nanocbor_fmt_int(enc, {n});"));
                        }
                    }
                    // `null` is CBOR null, not text (dgtlrift/cddlc#147).
                    (TypeRef::Primitive(Primitive::Null | Primitive::Undefined), None) => {
                        w.line("nanocbor_fmt_null(enc);");
                    }
                    (TypeRef::Primitive(Primitive::Tstr), None) => {
                        let (lit, len) = cpp_str_lit(&cddlc_codegen::enum_variant_text(v));
                        w.line(&format!("nanocbor_put_tstrn(enc, {lit}, {len});"));
                    }
                    (TypeRef::Primitive(Primitive::Uint), Some(t)) => {
                        w.line(&format!("nanocbor_fmt_uint(enc, {});", get(t)))
                    }
                    (TypeRef::Primitive(Primitive::Int), Some(t)) => {
                        w.line(&format!("nanocbor_fmt_int(enc, {});", get(t)))
                    }
                    (TypeRef::Primitive(Primitive::Bool), Some(t)) => {
                        w.line(&format!("nanocbor_fmt_bool(enc, {});", get(t)))
                    }
                    (TypeRef::Primitive(Primitive::Float32 | Primitive::Float16), Some(t)) => {
                        w.line(&format!("nanocbor_fmt_float(enc, {});", get(t)))
                    }
                    (TypeRef::Primitive(Primitive::Float64 | Primitive::Float), Some(t)) => {
                        w.line(&format!("nanocbor_fmt_double(enc, {});", get(t)))
                    }
                    (TypeRef::Primitive(Primitive::Tstr), Some(t)) => {
                        w.line(&format!("{{ const auto &s = {}; nanocbor_put_tstrn(enc, s.data(), s.size()); }}", get(t)));
                    }
                    (TypeRef::Primitive(Primitive::Bstr), Some(t)) => {
                        w.line(&format!("{{ const auto &b = {}; nanocbor_put_bstr(enc, reinterpret_cast<const uint8_t*>(b.data()), b.size()); }}", get(t)));
                    }
                    // The encoded item, written back unchanged (dgtlrift/cddlc#178).
                    (TypeRef::Primitive(Primitive::Any), Some(t)) => {
                        w.line(&format!(
                            "if (cddlc_put_item(enc, {}) < 0) return -1;",
                            get(t)
                        ));
                    }
                    (TypeRef::Named(_), Some(t)) => {
                        w.line(&format!("return encode(enc, {});", get(t)))
                    }
                    _ => w.line("return -1; // unsupported variant type"),
                }
                w.line("return NANOCBOR_OK;");
                w.dedent();
            }
            w.line("}");
            w.line("return -1;");
        }
        TypeDef::Array(a) => {
            if let Some(tag) = a.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            // A repeated group's members sit inline (dgtlrift/cddlc#90).
            let rec = flat_record(a, module);
            let width = rec.map_or(String::new(), |r| format!(" * {}", r.fields.len()));
            let item = if is_heap(opts) {
                w.line(&format!(
                    "nanocbor_fmt_array(enc, val.items.size(){width});"
                ));
                w.line("for (const auto &item : val.items) {");
                "item"
            } else {
                w.line(&format!("nanocbor_fmt_array(enc, val.count{width});"));
                w.line("for (std::size_t i = 0; i < val.count; i++) {");
                "val.items[i]"
            };
            w.indent();
            match rec {
                Some(r) => {
                    for f in &r.fields {
                        emit_cpp_encode_field(
                            w,
                            &f.ty,
                            &format!("{item}.{}", cddlc_codegen::cpp_ident(&f.name)),
                            &Occurrence::Required,
                            opts,
                        );
                    }
                }
                None => emit_cpp_encode_field(w, &a.element, item, &Occurrence::Required, opts),
            }
            w.dedent();
            w.line("}");
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Alias(a) => {
            if let Some(tag) = a.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            emit_cpp_encode_field(w, &a.ty, "val.value", &Occurrence::Required, opts);
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Map(m) => {
            if let Some(tag) = m.tagged {
                w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            }
            if is_heap(opts) {
                w.line("nanocbor_fmt_map(enc, val.entries.size());");
                w.line("for (const auto &e : val.entries) {");
            } else {
                w.line("nanocbor_fmt_map(enc, val.count);");
                w.line("for (std::size_t i = 0; i < val.count; i++) {");
                w.line("    const auto &e = val.entries[i];");
            }
            w.indent();
            emit_cpp_encode_field(w, &m.key_ty, "e.first", &Occurrence::Required, opts);
            emit_cpp_encode_field(w, &m.val_ty, "e.second", &Occurrence::Required, opts);
            w.dedent();
            w.line("}");
            w.line("return NANOCBOR_OK;");
        }
    }

    w.dedent();
    w.line("}");
}

fn emit_cpp_encode_field(
    w: &mut IndentWriter,
    ty: &TypeRef,
    expr: &str,
    _occ: &Occurrence,
    opts: &CodegenOptions,
) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = cpp_encode_call(p, expr);
            w.line(&format!("{call};"));
        }
        // A nested value that fails to encode (an empty required `Box`,
        // dgtlrift/cddlc#156) fails the whole.
        TypeRef::Named(_) => {
            w.line(&format!("if (encode(enc, {expr}) < 0) return -1;"));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line(&format!("nanocbor_fmt_tag(enc, {});", tag.0));
            emit_cpp_encode_field(w, inner, expr, &Occurrence::Required, opts);
        }
        TypeRef::Choice(cs) => match prim_choice(cs, opts) {
            // Whichever alternative the variant holds.
            Some(alts) => {
                for (i, (p, t)) in alts.iter().enumerate() {
                    let kw = if i == 0 { "if" } else { "else if" };
                    w.line(&format!(
                        "{kw} (const auto *_c = std::get_if<{t}>(&{expr})) {{"
                    ));
                    w.indent();
                    w.line(&format!("{};", cpp_encode_call(p, "(*_c)")));
                    w.dedent();
                    w.line("}");
                }
            }
            // Any other choice is held as its first alternative, and written
            // as one: there's no `encode` for, say, a `std::string_view`.
            None => match cs.first() {
                Some(first) => emit_cpp_encode_field(w, first, expr, _occ, opts),
                None => w.line(&format!("encode(enc, {expr});")),
            },
        },
    }
}

fn cpp_encode_call(p: &Primitive, expr: &str) -> String {
    match p {
        Primitive::Bool      => format!("nanocbor_fmt_bool(enc, {expr})"),
        Primitive::Null | Primitive::Undefined => "nanocbor_fmt_null(enc)".into(),
        Primitive::Uint      => format!("nanocbor_fmt_uint(enc, {expr})"),
        Primitive::Int       => format!("nanocbor_fmt_int(enc, {expr})"),
        Primitive::Float32 | Primitive::Float16 => format!("nanocbor_fmt_float(enc, {expr})"),
        Primitive::Float64 | Primitive::Float   => format!("nanocbor_fmt_double(enc, {expr})"),
        Primitive::Tstr      => format!("nanocbor_put_tstrn(enc, {expr}.data(), {expr}.size())"),
        Primitive::Bstr =>
            format!("nanocbor_put_bstr(enc, reinterpret_cast<const uint8_t*>({expr}.data()), {expr}.size())"),
        // The encoded item, written back unchanged (dgtlrift/cddlc#178).
        Primitive::Any => format!("cddlc_put_item(enc, {expr})"),
    }
}

fn emit_decode_fn(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    let type_name = to_pascal_case(def.name());
    w.line(&format!(
        "int decode(nanocbor_value_t *dec, {type_name} &out, const Callbacks &cb, void *ctx) {{"
    ));
    w.indent();
    w.line("(void)cb; (void)ctx;");

    match def {
        TypeDef::Struct(s) => {
            if let Some(tag) = s.tagged {
                w.line("{ uint32_t tag;");
                w.line("  if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("  if (tag != {}) return -1; }}", tag.0));
            }
            if s.encoding == cddlc_ir::StructEncoding::Array {
                // A record: the members' values in order; optional ones last.
                w.line("nanocbor_value_t arr;");
                w.line("if (nanocbor_enter_array(dec, &arr) < 0) return -1;");
                // One optional member before required ones: present when the
                // array holds every member, so count the items on a copy
                // (dgtlrift/cddlc#134).
                let before = cddlc_codegen::record_optionals_before(s);
                let repeated = cddlc_codegen::record_repeated(s);
                let required = s
                    .fields
                    .iter()
                    .filter(|f| f.occurrence.is_required())
                    .count();
                if before.is_some() || repeated.is_some() {
                    w.line("std::size_t count = 0;");
                    w.line("{ nanocbor_value_t probe = arr; while (!nanocbor_at_end(&probe)) { if (nanocbor_skip(&probe) < 0) return -1; count++; } }");
                }
                // A repeated member holds the items the others leave.
                if let Some(ri) = repeated {
                    let rf = &s.fields[ri];
                    w.line(&format!(
                        "if (count < {required}) return -1; /* too few items */"
                    ));
                    w.line(&format!("std::size_t reps = count - {required};"));
                    if !is_heap(opts) {
                        let cap = capacity_value(
                            rf.occurrence
                                .capacity()
                                .unwrap_or(&Capacity::Bounded(opts.max_array)),
                            opts.max_array,
                        );
                        w.line(&format!(
                            "if (reps > {cap}) return -1; /* more items than its capacity */"
                        ));
                    }
                    emit_cpp_repeat_check(w, rf, "reps");
                }
                for (i, f) in s.fields.iter().enumerate() {
                    let fname = cddlc_codegen::cpp_ident(&f.name);
                    if f.occurrence.is_repeated() {
                        if is_heap(opts) {
                            w.line(&format!("out.{fname}.resize(reps);"));
                        } else {
                            w.line(&format!("out.{fname}_count = reps;"));
                        }
                        w.line("for (std::size_t i = 0; i < reps; i++) {");
                        w.indent();
                        emit_cpp_decode_field(
                            w,
                            &f.ty,
                            &format!("out.{fname}[i]"),
                            &f.constraints,
                            &f.name,
                            "&arr",
                            opts,
                        );
                        w.dedent();
                        w.line("}");
                    } else if matches!(f.occurrence, Occurrence::Optional) {
                        if let Some(b) = &before {
                            w.line(&format!("if (count > {}) {{", required + b[i]));
                        } else {
                            w.line("if (!nanocbor_at_end(&arr)) {");
                        }
                        w.indent();
                        w.line(&format!("{} _v{{}};", member_temp_type(s, f, opts)));
                        emit_cpp_decode_field(
                            w,
                            &f.ty,
                            "_v",
                            &f.constraints,
                            &f.name,
                            "&arr",
                            opts,
                        );
                        w.line(&format!("out.{fname} = std::move(_v);"));
                        w.dedent();
                        w.line("}");
                    } else {
                        w.line("if (nanocbor_at_end(&arr)) return -1; /* missing item */");
                        emit_cpp_decode_field(
                            w,
                            &f.ty,
                            &format!("out.{fname}"),
                            &f.constraints,
                            &f.name,
                            "&arr",
                            opts,
                        );
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
            // Each required member must be read (dgtlrift/cddlc#102).
            for f in s.fields.iter().filter(|f| f.occurrence.is_required()) {
                w.line(&format!(
                    "bool have_{} = false;",
                    cddlc_codegen::cpp_ident(&f.name)
                ));
            }
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
            w.line("const uint8_t *key_ptr = nullptr; size_t key_len = 0;");
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
            w.line("std::string_view key(reinterpret_cast<const char*>(key_ptr), key_len);");
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                let fkey = f.name.as_str();
                let cond = match &f.key {
                    cddlc_ir::FieldKey::Text(t) if int_keys => {
                        format!("!key_is_int && key == {:?}", t.as_str())
                    }
                    cddlc_ir::FieldKey::Text(t) => format!("key == {:?}", t.as_str()),
                    cddlc_ir::FieldKey::Int(n) => format!("key_is_int && key_int == {n}LL"),
                    cddlc_ir::FieldKey::Uint(n) => {
                        format!("key_is_int && key_int == {}LL", (*n).min(i64::MAX as u64))
                    }
                };
                w.line(&format!("if ({cond}) {{"));
                w.indent();
                if matches!(f.occurrence, Occurrence::Optional) {
                    // Decode into the inner type, then set the optional: nanocbor
                    // writes through a pointer to the inner type, never an optional
                    // (dgtlrift/cddlc#35). A present `nil` becomes a present monostate.
                    w.line(&format!("{} _v{{}};", member_temp_type(s, f, opts)));
                    if is_boxed(&s.name, &f.name) && matches!(f.ty, TypeRef::Choice(_)) {
                        emit_boxed_choice_decode(w, &f.ty, "_v", "&map");
                    } else {
                        emit_cpp_decode_field(w, &f.ty, "_v", &f.constraints, fkey, "&map", opts);
                    }
                    w.line(&format!("out.{fname} = std::move(_v);"));
                } else if is_boxed(&s.name, &f.name) && matches!(f.ty, TypeRef::Choice(_)) {
                    emit_boxed_choice_decode(w, &f.ty, &format!("out.{fname}"), "&map");
                } else {
                    emit_cpp_decode_field(
                        w,
                        &f.ty,
                        &format!("out.{fname}"),
                        &f.constraints,
                        fkey,
                        "&map",
                        opts,
                    );
                }
                if f.occurrence.is_required() {
                    w.line(&format!("have_{fname} = true;"));
                }
                w.dedent();
                w.line("} else");
            }
            w.line("{");
            w.indent();
            match &s.rest {
                // Other keys are the map's other entries (dgtlrift/cddlc#82).
                Some(r) => {
                    let rname = cddlc_codegen::cpp_ident(&r.name);
                    if typed_rest {
                        w.line(&format!(
                            "std::pair<{}, {}> e{{}};",
                            rest_key_cpp(r, opts),
                            cpp_typeref(&r.val_ty, opts)
                        ));
                        w.line("nanocbor_value_t kd = key_start;");
                        emit_cpp_decode_field(w, &r.key_ty, "e.first", &[], &r.name, "&kd", opts);
                    } else {
                        if int_keys {
                            w.line(
                                "if (key_is_int) return -1; // the other entries have text keys",
                            );
                        }
                        w.line(&format!(
                            "std::pair<std::string_view, {}> e{{key, {{}}}};",
                            cpp_typeref(&r.val_ty, opts)
                        ));
                    }
                    emit_cpp_decode_field(w, &r.val_ty, "e.second", &[], &r.name, "&map", opts);
                    if is_heap(opts) {
                        w.line(&format!("out.{rname}.push_back(e);"));
                    } else {
                        let cap = capacity_value(
                            r.occurrence
                                .capacity()
                                .unwrap_or(&Capacity::Bounded(opts.max_array)),
                            opts.max_array,
                        );
                        w.line(&format!("if (out.{rname}_count >= {cap}) return -1;"));
                        w.line(&format!("out.{rname}[out.{rname}_count++] = e;"));
                    }
                }
                // An unknown member: reported, and the map is rejected, as a map
                // has only the members its schema gives it (dgtlrift/cddlc#102).
                None => {
                    w.line("if (cb.on_unknown_field) cb.on_unknown_field(ctx, key);");
                    w.line("return -1;");
                }
            }
            w.dedent();
            w.line("}");
            w.dedent();
            w.line("}");
            for f in s.fields.iter().filter(|f| f.occurrence.is_required()) {
                w.line(&format!(
                    "if (!have_{}) return -1; /* missing member */",
                    cddlc_codegen::cpp_ident(&f.name)
                ));
            }
            w.line("nanocbor_leave_container(dec, &map);");
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Enum(e) => {
            if let Some(tag) = e.tagged {
                w.line("{ uint32_t tag;");
                w.line("  if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("  if (tag != {}) return -1; }}", tag.0));
            }
            emit_cpp_enum_decode(w, e, opts);
        }
        TypeDef::Array(a) => {
            if let Some(tag) = a.tagged {
                w.line("{ uint32_t tag;");
                w.line("  if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("  if (tag != {}) return -1; }}", tag.0));
            }
            let cap = capacity_value(
                a.occurrence
                    .capacity()
                    .unwrap_or(&Capacity::Bounded(opts.max_array)),
                opts.max_array,
            );
            w.line("nanocbor_value_t arr;");
            w.line("if (nanocbor_enter_array(dec, &arr) < 0) return -1;");
            if is_heap(opts) {
                w.line("out.items.clear();");
                w.line("while (!nanocbor_at_end(&arr)) {");
                w.indent();
                w.line(&format!("{} item{{}};", cpp_typeref(&a.element, opts)));
                emit_cpp_array_item_decode(w, a, module, "item", opts);
                w.line("out.items.push_back(item);");
                w.dedent();
                w.line("}");
                w.line("nanocbor_leave_container(dec, &arr);");
                w.line("return NANOCBOR_OK;");
                w.dedent();
                w.line("}");
                return;
            }
            w.line("out.count = 0;");
            w.line(&format!(
                "while (!nanocbor_at_end(&arr) && out.count < {cap}) {{"
            ));
            w.indent();
            emit_cpp_array_item_decode(w, a, module, "out.items[out.count]", opts);
            w.line("out.count++;");
            w.dedent();
            w.line("}");
            w.line("nanocbor_leave_container(dec, &arr);");
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Alias(a) => {
            if let Some(tag) = a.tagged {
                w.line("{ uint32_t tag;");
                w.line("  if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("  if (tag != {}) return -1; }}", tag.0));
            }
            emit_cpp_decode_field(w, &a.ty, "out.value", &a.constraints, "value", "dec", opts);
            w.line("return NANOCBOR_OK;");
        }
        TypeDef::Map(m) => {
            if let Some(tag) = m.tagged {
                w.line("{ uint32_t tag;");
                w.line("  if (nanocbor_get_tag(dec, &tag) < 0) return -1;");
                w.line(&format!("  if (tag != {}) return -1; }}", tag.0));
            }
            let entry = format!(
                "std::pair<{}, {}>",
                cpp_typeref(&m.key_ty, opts),
                cpp_typeref(&m.val_ty, opts)
            );
            w.line("nanocbor_value_t map;");
            w.line("if (nanocbor_enter_map(dec, &map) < 0) return -1;");
            if is_heap(opts) {
                w.line("out.entries.clear();");
            } else {
                w.line("out.count = 0;");
            }
            w.line("while (!nanocbor_at_end(&map)) {");
            w.indent();
            if !is_heap(opts) {
                let cap = capacity_value(
                    m.occurrence
                        .capacity()
                        .unwrap_or(&Capacity::Bounded(opts.max_array)),
                    opts.max_array,
                );
                w.line(&format!("if (out.count >= {cap}) return -1;"));
            }
            w.line(&format!("{entry} e{{}};"));
            emit_cpp_decode_field(w, &m.key_ty, "e.first", &[], "key", "&map", opts);
            emit_cpp_decode_field(w, &m.val_ty, "e.second", &[], "value", "&map", opts);
            if is_heap(opts) {
                w.line("out.entries.push_back(e);");
            } else {
                w.line("out.entries[out.count++] = e;");
            }
            w.dedent();
            w.line("}");
            w.line("nanocbor_leave_container(dec, &map);");
            w.line("return NANOCBOR_OK;");
        }
    }

    w.dedent();
    w.line("}");
}

/// Decodes an enum by the CBOR major type it finds: literals first, then the
/// variant whose payload type matches; a value no case accepts is tried against
/// each named variant (dgtlrift/cddlc#49).
fn emit_cpp_enum_decode(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions) {
    let t = to_pascal_case(&e.name);
    let make = |v: &EnumVariant, arg: &str| {
        // A payload is moved in (a `Box` is, not copied; dgtlrift/cddlc#156).
        let arg = if arg.is_empty() {
            String::new()
        } else {
            format!("std::move({arg})")
        };
        format!(
            "out = {t}::make_{}({arg}); return NANOCBOR_OK;",
            to_pascal_case(&v.name)
        )
    };
    let payload_is = |v: &EnumVariant, f: &dyn Fn(&Primitive) -> bool| {
        cpp_variant_payload(v, opts).is_some() && matches!(&v.ty, TypeRef::Primitive(p) if f(p))
    };

    // An `any` alternative takes what no other does (dgtlrift/cddlc#178): a case
    // that finds no alternative goes back to the item's start and takes it whole.
    let any = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Any)));
    let nomatch = if any.is_some() {
        "goto cddlc_any;"
    } else {
        "return -1;"
    };
    if any.is_some() {
        w.line("const nanocbor_value_t _start = *dec;");
    }
    w.line("switch (nanocbor_get_type(dec)) {");

    // Text: text literals, then a `tstr` payload variant.
    w.line("case NANOCBOR_TYPE_TSTR: {");
    w.indent();
    w.line("const uint8_t *s; size_t slen;");
    w.line("if (nanocbor_get_tstr(dec, &s, &slen) < 0) return -1;");
    w.line("std::string_view sv(reinterpret_cast<const char*>(s), slen);");
    for v in &e.variants {
        if cpp_variant_payload(v, opts).is_none()
            && matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr))
        {
            let text = cddlc_codegen::enum_variant_text(v);
            w.line(&format!(
                "if (sv == std::string_view({text:?})) {{ {} }}",
                make(v, "")
            ));
        }
    }
    if let Some(v) = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Tstr)))
    {
        w.line(&make(v, "sv"));
    } else {
        w.line(nomatch);
    }
    w.dedent();
    w.line("}");

    // Integers: integer literals, then a `uint` / `int` payload variant.
    w.line("case NANOCBOR_TYPE_UINT: {");
    w.indent();
    w.line("uint64_t u; if (nanocbor_get_uint64(dec, &u) < 0) return -1;");
    for v in &e.variants {
        if let Some(n) = cpp_int_literal(v).filter(|n| *n >= 0) {
            w.line(&format!("if (u == {n}u) {{ {} }}", make(v, "")));
        }
    }
    if let Some(v) = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Uint)))
    {
        w.line(&make(v, "u"));
    } else if let Some(v) = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Int)))
    {
        w.line(&format!(
            "if (u <= static_cast<uint64_t>(INT64_MAX)) {{ {} }}",
            make(v, "static_cast<int64_t>(u)")
        ));
        w.line(nomatch);
    } else {
        w.line(nomatch);
    }
    w.dedent();
    w.line("}");
    w.line("case NANOCBOR_TYPE_NINT: {");
    w.indent();
    w.line("int64_t n; if (nanocbor_get_int64(dec, &n) < 0) return -1;");
    for v in &e.variants {
        if let Some(n) = cpp_int_literal(v).filter(|n| *n < 0) {
            w.line(&format!("if (n == {n}) {{ {} }}", make(v, "")));
        }
    }
    if let Some(v) = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Int)))
    {
        w.line(&make(v, "n"));
    } else {
        w.line(nomatch);
    }
    w.dedent();
    w.line("}");

    // Byte strings: a `bstr` payload variant.
    if let Some(v) = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Bstr)))
    {
        w.line("case NANOCBOR_TYPE_BSTR: {");
        w.indent();
        w.line("const uint8_t *b; size_t blen;");
        w.line("if (nanocbor_get_bstr(dec, &b, &blen) < 0) return -1;");
        w.line(&make(
            v,
            "std::string_view(reinterpret_cast<const char*>(b), blen)",
        ));
        w.dedent();
        w.line("}");
    }

    // Major type 7: a bool, or a float.
    let bool_v = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Bool)));
    let f64_v = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Float64 | Primitive::Float)));
    let f32_v = e
        .variants
        .iter()
        .find(|v| payload_is(v, &|p| matches!(p, Primitive::Float32 | Primitive::Float16)));
    let nil_v = e.variants.iter().find(|v| {
        matches!(
            &v.ty,
            TypeRef::Primitive(Primitive::Null | Primitive::Undefined)
        )
    });
    if bool_v.is_some() || f64_v.is_some() || f32_v.is_some() || nil_v.is_some() {
        w.line("case NANOCBOR_TYPE_FLOAT: {");
        w.indent();
        if let Some(v) = nil_v {
            w.line(&format!(
                "if (nanocbor_get_null(dec) >= 0) {{ {} }}",
                make(v, "")
            ));
        }
        if let Some(v) = bool_v {
            w.line(&format!(
                "{{ bool b; if (nanocbor_get_bool(dec, &b) >= 0) {{ {} }} }}",
                make(v, "b")
            ));
        }
        if let Some(v) = f32_v {
            w.line(&format!(
                "{{ float f; if (nanocbor_get_float(dec, &f) >= 0) {{ {} }} }}",
                make(v, "f")
            ));
        }
        if let Some(v) = f64_v {
            w.line(&format!(
                "{{ double d; if (nanocbor_get_double(dec, &d) >= 0) {{ {} }} }}",
                make(v, "d")
            ));
        }
        w.line("break;");
        w.dedent();
        w.line("}");
    }
    w.line("default:");
    w.indent();
    w.line("break;");
    w.dedent();
    w.line("}");

    // Anything else: the first named variant whose decoder accepts it.
    for v in &e.variants {
        if let (TypeRef::Named(_), Some(pt)) = (&v.ty, cpp_variant_payload_in(&e.name, v, opts)) {
            // A boxed variant decodes into a `Box`, whose memory comes from
            // the heap or `Callbacks::alloc` (dgtlrift/cddlc#156).
            let nt = pt;
            w.line("{");
            w.indent();
            w.line("nanocbor_value_t tmp = *dec;");
            w.line(&format!("{nt} v{{}};"));
            w.line("if (decode(&tmp, v, cb, ctx) == 0) { *dec = tmp; ");
            w.indent();
            w.line(&make(v, "v"));
            w.dedent();
            w.line("}");
            w.dedent();
            w.line("}");
        }
    }
    match any {
        Some(v) => {
            w.line("cddlc_any:");
            w.line("*dec = _start;");
            w.line("{");
            w.indent();
            w.line("const uint8_t *p; size_t len;");
            w.line("if (nanocbor_get_subcbor(dec, &p, &len) < 0) return -1;");
            w.line(&make(
                v,
                "std::string_view(reinterpret_cast<const char*>(p), len)",
            ));
            w.dedent();
            w.line("}");
        }
        None => w.line("return -1;"),
    }
}

fn emit_cpp_decode_field(
    w: &mut IndentWriter,
    ty: &TypeRef,
    dest: &str,
    constraints: &[Constraint],
    field_name: &str,
    dec_expr: &str,
    opts: &CodegenOptions,
) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = cpp_decode_call(p, dest, dec_expr);
            w.line(&format!("if ({call} < 0) return -1;"));
            for c in constraints {
                if let Some(check) = cpp_constraint_check(c, dest, p) {
                    w.line(&format!("if (!({check})) {{"));
                    w.indent();
                    w.line(&format!(
                        "if (cb.on_error) cb.on_error(ctx, {{ErrorCode::ValidationFailed, {:?}}});",
                        field_name
                    ));
                    w.line("return -1;");
                    w.dedent();
                    w.line("}");
                }
            }
        }
        TypeRef::Named(_) => {
            w.line(&format!(
                "if (decode({dec_expr}, {dest}, cb, ctx) < 0) return -1;"
            ));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line("{ uint32_t tag;");
            w.line(&format!(
                "  if (nanocbor_get_tag({dec_expr}, &tag) < 0) return -1;"
            ));
            w.line(&format!("  if (tag != {}) return -1;", tag.0));
            emit_cpp_decode_field(w, inner, dest, constraints, field_name, dec_expr, opts);
            w.line("}");
        }
        TypeRef::Choice(cs) => match prim_choice(cs, opts) {
            // The first alternative that reads the item, tried on a copy of the
            // decoder so a failed attempt consumes nothing.
            Some(alts) => {
                w.line("{");
                w.indent();
                w.line("bool _matched = false;");
                for (p, t) in &alts {
                    w.line("if (!_matched) {");
                    w.indent();
                    w.line(&format!("nanocbor_value_t _probe = *({dec_expr});"));
                    w.line(&format!("{t} _alt{{}};"));
                    // `_alt`, not `_v`: the member being decoded may be `_v`
                    // itself, which an inner `_v` would shadow.
                    w.line(&format!("if ({} >= 0) {{ *({dec_expr}) = _probe; {dest} = _alt; _matched = true; }}", cpp_decode_call(p, "_alt", "&_probe")));
                    w.dedent();
                    w.line("}");
                }
                w.line("if (!_matched) return -1;");
                w.dedent();
                w.line("}");
            }
            // Any other choice is held as its first alternative (see
            // `cpp_typeref`), and read as one: it was skipped.
            None => match cs.first() {
                Some(first) => {
                    emit_cpp_decode_field(w, first, dest, constraints, field_name, dec_expr, opts)
                }
                None => w.line(&format!("nanocbor_skip({dec_expr});")),
            },
        },
    }
}

/// Type-aware constraint check: for integer primitives `.size N` means
/// "fits in N bytes" (value-range), not a `.size()` call.
fn cpp_constraint_check(c: &Constraint, val_expr: &str, p: &Primitive) -> Option<String> {
    // A byte-string literal (RFC 9277's `'BOR'`), as a `string_view` of its
    // bytes (dgtlrift/cddlc#142).
    match (c, p) {
        (Constraint::Eq(LiteralValue::Bytes(b)), Primitive::Bstr) => {
            return Some(format!("{val_expr} == {}", cpp_bytes_view(b)))
        }
        (Constraint::Ne(LiteralValue::Bytes(b)), Primitive::Bstr) => {
            return Some(format!("{val_expr} != {}", cpp_bytes_view(b)))
        }
        _ => {}
    }
    match c {
        Constraint::SizeExact(n) => match p {
            Primitive::Uint => {
                let max = cpp_uint_max_for_bytes(*n)?;
                Some(format!("{val_expr} <= {max}"))
            }
            Primitive::Int => {
                let (lo, hi) = cpp_int_range_for_bytes(*n)?;
                Some(format!("{val_expr} >= {lo} && {val_expr} <= {hi}"))
            }
            _ => constraint_to_cpp_check(c, val_expr),
        },
        Constraint::SizeRange { .. } => match p {
            Primitive::Uint | Primitive::Int => None, // uncommon; skip
            _ => constraint_to_cpp_check(c, val_expr),
        },
        // `.bits` (dgtlrift/cddlc#107): no bit outside the mask is set.
        Constraint::Bits(bits) => match p {
            Primitive::Uint => Some(format!(
                "(({val_expr}) & ~{:#x}ULL) == 0",
                cddlc_codegen::bits_mask_u64(bits)
            )),
            Primitive::Bstr => {
                let m = cddlc_codegen::bits_mask_bytes(bits);
                let bytes = m
                    .iter()
                    .map(|b| format!("{b:#04x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "[](const auto& v) {{ static const uint8_t m[] = {{{bytes}{}}}; std::size_t i = 0; \
                     for (auto b : v) {{ if (static_cast<uint8_t>(b) & static_cast<uint8_t>(~(i < {n} ? m[i] : 0))) return false; ++i; }} \
                     return true; }}({val_expr})",
                    if m.is_empty() { "0" } else { "" },
                    n = m.len(),
                ))
            }
            _ => None,
        },
        // `.regexp` without a hook: the whole text matches, with std::regex
        // compiled once (ECMAScript grammar).
        Constraint::Regexp { pattern, hook: None } if matches!(p, Primitive::Tstr) => Some(format!(
            "[](const auto& v) {{ static const std::regex re({}); return std::regex_match(v.begin(), v.end(), re); }}({val_expr})",
            cpp_raw_str(pattern)
        )),
        _ => constraint_to_cpp_check(c, val_expr),
    }
}

/// Throws if a decoded JSON member breaks one of its constraints: a member
/// of a primitive type, as CBOR checks (`.size`, ranges, `.regexp`, ...).
fn emit_cpp_json_checks(w: &mut IndentWriter, ctype: &str, f: &cddlc_ir::FieldDef) {
    let TypeRef::Primitive(p) = &f.ty else {
        return;
    };
    if f.occurrence.is_repeated() {
        return;
    }
    let fname = cddlc_codegen::cpp_ident(&f.name);
    let optional = matches!(f.occurrence, Occurrence::Optional);
    let val = if optional {
        format!("(*out.{fname})")
    } else {
        format!("out.{fname}")
    };
    for c in &f.constraints {
        // A byte-string literal compares as a `string_view`, which JSON's
        // vector isn't.
        if matches!(
            c,
            Constraint::Eq(LiteralValue::Bytes(_)) | Constraint::Ne(LiteralValue::Bytes(_))
        ) {
            continue;
        }
        let Some(check) = cpp_constraint_check(c, &val, p) else {
            continue;
        };
        let guard = if optional {
            format!("out.{fname} && ")
        } else {
            String::new()
        };
        w.line(&format!(
            "if ({guard}!({check})) throw std::invalid_argument(\"{ctype}.{}: value breaks its constraint\");",
            f.name
        ));
    }
}

/// A C++ literal of a numeric or text [`cddlc_codegen::SampleValue`].
fn cpp_sample_lit(v: &cddlc_codegen::SampleValue) -> String {
    match v {
        cddlc_codegen::SampleValue::Uint(n) => format!("{n}ULL"),
        cddlc_codegen::SampleValue::Int(n) if *n == i64::MIN => "INT64_MIN".into(),
        cddlc_codegen::SampleValue::Int(n) => format!("{n}LL"),
        cddlc_codegen::SampleValue::Float(x) => format!("{x:?}"),
        cddlc_codegen::SampleValue::Text(t) => format!("std::string({t:?})"),
    }
}

/// The CBOR `null` item, as an `any`'s initializer.
const CPP_NULL_ITEM: &str = "{\"\\xf6\", 1}";

/// A member's initializer: a required member pinned to one value
/// (`type: "accessibility"`) holds it by default, so a default value of its
/// type decodes back; any other is value-initialized (`{}`).
fn cpp_pinned_init(f: &cddlc_ir::FieldDef, json: bool) -> String {
    if !f.occurrence.is_required() || !matches!(f.ty, TypeRef::Primitive(_)) {
        return "{}".into();
    }
    // An `any` is the `null` item by default: an empty one isn't an item
    // (JSON's `nlohmann::json` is null already).
    if !json && matches!(f.ty, TypeRef::Primitive(Primitive::Any)) {
        return CPP_NULL_ITEM.into();
    }
    f.constraints
        .iter()
        .find_map(|c| match c {
            Constraint::Eq(LiteralValue::Text(t)) => Some(format!("{{{t:?}}}")),
            Constraint::Eq(LiteralValue::Bool(b)) => Some(format!("{{{b}}}")),
            Constraint::Eq(LiteralValue::Uint(n)) => Some(format!("{{{n}u}}")),
            Constraint::Eq(LiteralValue::Int(n)) => Some(format!("{{{n}}}")),
            _ => None,
        })
        .unwrap_or_else(|| "{}".into())
}

/// A C++ raw string literal of `s`, with a delimiter `s` doesn't contain.
fn cpp_raw_str(s: &str) -> String {
    let mut d = String::from("re");
    while s.contains(&format!("){d}\"")) {
        d.push('_');
    }
    format!("R\"{d}({s}){d}\"")
}

/// Whether a member or alias has a `.regexp` with no hook, which std::regex
/// checks (it needs `<regex>`).
fn module_uses_regex(module: &IrModule) -> bool {
    let has = |cs: &[Constraint]| {
        cs.iter()
            .any(|c| matches!(c, Constraint::Regexp { hook: None, .. }))
    };
    module.types.values().any(|d| match d {
        TypeDef::Struct(s) => s.fields.iter().any(|f| has(&f.constraints)),
        TypeDef::Alias(a) => has(&a.constraints),
        TypeDef::Enum(e) => e.variants.iter().any(|v| has(&v.constraints)),
        _ => false,
    })
}

fn cpp_uint_max_for_bytes(n: usize) -> Option<String> {
    match n {
        1 => Some("0xFFULL".into()),
        2 => Some("0xFFFFULL".into()),
        4 => Some("0xFFFFFFFFULL".into()),
        8 => None, // uint64 always fits
        _ => None,
    }
}

fn cpp_int_range_for_bytes(n: usize) -> Option<(i64, i64)> {
    match n {
        1 => Some((-128, 127)),
        2 => Some((-32768, 32767)),
        4 => Some((-2147483648, 2147483647)),
        8 => None,
        _ => None,
    }
}

fn cpp_decode_call(p: &Primitive, dest: &str, dec: &str) -> String {
    match p {
        Primitive::Bool => format!("nanocbor_get_bool({dec}, &{dest})"),
        Primitive::Null | Primitive::Undefined => format!("nanocbor_get_null({dec})"),
        Primitive::Uint => {
            format!("nanocbor_get_uint64({dec}, reinterpret_cast<uint64_t*>(&{dest}))")
        }
        Primitive::Int => format!("nanocbor_get_int64({dec}, reinterpret_cast<int64_t*>(&{dest}))"),
        Primitive::Float32 | Primitive::Float16 => format!("nanocbor_get_float({dec}, &{dest})"),
        Primitive::Float64 | Primitive::Float => format!("nanocbor_get_double({dec}, &{dest})"),
        Primitive::Tstr => format!(
            "({{ const uint8_t *_p; size_t _l; int _r = nanocbor_get_tstr({dec}, &_p, &_l); \
             if (_r >= 0) {dest} = std::string_view(reinterpret_cast<const char*>(_p), _l); _r; }})"
        ),
        Primitive::Bstr => format!(
            "({{ const uint8_t *_p; size_t _l; int _r = nanocbor_get_bstr({dec}, &_p, &_l); \
             if (_r >= 0) {dest} = std::string_view(reinterpret_cast<const char*>(_p), _l); _r; }})"
        ),
        // The whole item's encoding, which nanocbor then skips (dgtlrift/cddlc#178).
        Primitive::Any => format!(
            "({{ const uint8_t *_p; size_t _l; int _r = nanocbor_get_subcbor({dec}, &_p, &_l); \
             if (_r >= 0) {dest} = std::string_view(reinterpret_cast<const char*>(_p), _l); _r; }})"
        ),
    }
}

// ── Test file ─────────────────────────────────────────────────────────────────

/// Returns the first concrete Primitive in a TypeRef, following Tagged and Choice.
fn effective_primitive(ty: &TypeRef) -> Option<&Primitive> {
    match ty {
        TypeRef::Primitive(p) => Some(p),
        TypeRef::Choice(cs) => cs.first().and_then(|c| effective_primitive(c)),
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

fn emit_tests(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = cddlc_codegen::cpp_ident(name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Roundtrip encode/decode tests");
    w.line(&format!("#include \"{name}.hpp\""));
    w.line("#include <cassert>");
    w.line("#include <cstdio>");
    w.blank();

    w.line(&format!("using namespace {ns};"));
    w.blank();
    w.line("static uint8_t buf[1024];");
    w.blank();
    // A recursive member's memory, from the heap here; any allocator (a pool,
    // an arena) works (dgtlrift/cddlc#156).
    w.line("static void *test_alloc(void *, std::size_t n) { return ::operator new(n); }");
    w.line("static void test_release(void *, void *p) { ::operator delete(p); }");
    w.line("static const Callbacks test_cb = [] { Callbacks c{}; c.alloc = test_alloc; c.release = test_release; return c; }();");
    w.blank();

    for (_, def) in &module.types {
        emit_cpp_type_test(&mut w, def, module, opts);
        w.blank();
    }

    w.line("int main() {");
    w.indent();
    for (_, def) in &module.types {
        let fn_name = cddlc_codegen::cpp_ident(def.name());
        w.line(&format!("test_{fn_name}_roundtrip();"));
    }
    w.line("std::printf(\"All C++ roundtrip tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}

fn emit_cpp_type_test(
    w: &mut IndentWriter,
    def: &TypeDef,
    module: &IrModule,
    _opts: &CodegenOptions,
) {
    let type_name = to_pascal_case(def.name());
    let fn_name = cddlc_codegen::cpp_ident(def.name());

    w.line(&format!("static void test_{fn_name}_roundtrip() {{"));
    w.indent();

    // An enum test loops over one value per variant; the roundtrip below is the body.
    let mut enum_loop = false;
    match def {
        TypeDef::Alias(a) if !a.constraints.is_empty() => {
            // For size-constrained aliases, construct a valid test value.
            let n = a.constraints.iter().find_map(|c| match c {
                cddlc_ir::Constraint::SizeExact(n) => Some(*n),
                cddlc_ir::Constraint::SizeRange { min: Some(n), .. } => Some(*n),
                _ => None,
            });
            if let Some(n) = n {
                match &a.ty {
                    TypeRef::Primitive(Primitive::Tstr) => {
                        let s = "a".repeat(n);
                        w.line(&format!(
                            "{type_name} original{{ std::string_view({:?}, {n}) }};",
                            s
                        ));
                    }
                    TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => {
                        // bstr alias: value is std::string_view over a static byte array
                        let bytes: Vec<String> = (0..n)
                            .map(|i| format!("0x{:02x}", (i % 256) as u8))
                            .collect();
                        w.line(&format!(
                            "static const uint8_t _bytes[{n}] = {{{}}};",
                            bytes.join(", ")
                        ));
                        w.line(&format!("{type_name} original{{ std::string_view(reinterpret_cast<const char*>(_bytes), {n}) }};"));
                    }
                    _ => w.line(&format!("{type_name} original{{}};")),
                }
            } else {
                // A value pinned to a literal, or with a lower bound (`40 .plus 2`,
                // `uint .within (1..5)`; dgtlrift/cddlc#86).
                let value = a.constraints.iter().find_map(|c| match c {
                    cddlc_ir::Constraint::Eq(LiteralValue::Text(t)) => {
                        Some(format!("std::string_view({t:?})"))
                    }
                    cddlc_ir::Constraint::Eq(LiteralValue::Bytes(b)) => Some(cpp_bytes_view(b)),
                    cddlc_ir::Constraint::Eq(LiteralValue::Uint(n)) => Some(format!("{n}u")),
                    cddlc_ir::Constraint::Eq(LiteralValue::Int(n)) => Some(format!("{n}")),
                    cddlc_ir::Constraint::ValueRangeUint { min: Some(lo), .. } if *lo > 0 => {
                        Some(format!("{}u", *lo))
                    }
                    cddlc_ir::Constraint::ValueRangeInt { min: Some(lo), .. } if *lo > 0 => {
                        Some(format!("{}", *lo))
                    }
                    _ => None,
                });
                match value {
                    Some(v) => w.line(&format!("{type_name} original{{ {v} }};")),
                    None => w.line(&format!("{type_name} original{{}};")),
                }
            }
        }
        TypeDef::Enum(e) => {
            // Roundtrip every variant (dgtlrift/cddlc#49).
            let values: Vec<String> = e
                .variants
                .iter()
                .map(|v| {
                    let k = to_pascal_case(&v.name);
                    let arg = match (&v.ty, cpp_variant_payload(v, _opts)) {
                        (_, None) => String::new(),
                        (TypeRef::Primitive(Primitive::Uint), _) => "42u".into(),
                        (TypeRef::Primitive(Primitive::Int), _) => "-1".into(),
                        (TypeRef::Primitive(Primitive::Bool), _) => "true".into(),
                        (TypeRef::Primitive(Primitive::Float32 | Primitive::Float16), _) => {
                            "1.5f".into()
                        }
                        (TypeRef::Primitive(Primitive::Float64 | Primitive::Float), _) => {
                            "1.5".into()
                        }
                        (TypeRef::Primitive(Primitive::Tstr), _) => {
                            "std::string_view(\"hello\")".into()
                        }
                        // An item no other alternative takes (dgtlrift/cddlc#178):
                        // `true`, or `[]` beside a `bool` alternative.
                        (TypeRef::Primitive(Primitive::Any), _) => {
                            let has_bool = e
                                .variants
                                .iter()
                                .any(|o| matches!(o.ty, TypeRef::Primitive(Primitive::Bool)));
                            if has_bool {
                                "std::string_view(\"\\x80\", 1)".into()
                            } else {
                                "std::string_view(\"\\xf5\", 1)".into()
                            }
                        }
                        // A map variant: its pinned members hold their value by
                        // default (`cpp_pinned_init`), and its required boxed
                        // members are filled (dgtlrift/cddlc#156).
                        (TypeRef::Named(n), Some(t)) => match module.get(n) {
                            Some(TypeDef::Struct(_)) => cpp_filled(n, module),
                            _ => format!("{t}{{}}"),
                        },
                        (_, Some(t)) => format!("{t}{{}}"),
                    };
                    format!("{type_name}::make_{k}({arg})")
                })
                .collect();
            w.line(&format!(
                "for (const {type_name} &original : {{ {} }}) {{",
                values.join(", ")
            ));
            w.indent();
            enum_loop = true;
        }
        TypeDef::Struct(s) => {
            w.line(&format!("{type_name} original{{}};"));
            // A required boxed member holds a value (dgtlrift/cddlc#156).
            for f in s
                .fields
                .iter()
                .filter(|f| f.occurrence.is_required() && is_boxed(&s.name, &f.name))
            {
                if let Some(n) = boxed_named(&f.ty, _opts) {
                    w.line(&format!(
                        "original.{} = {};",
                        cddlc_codegen::cpp_ident(&f.name),
                        cpp_filled(n, module)
                    ));
                }
            }
            // A repeated member gets as many (default) items as it needs
            // (dgtlrift/cddlc#134).
            for f in s.fields.iter().filter(|f| f.occurrence.is_repeated()) {
                let min = cddlc_codegen::repeat_bounds(&f.occurrence).0;
                let fname = cddlc_codegen::cpp_ident(&f.name);
                if min > 0 {
                    if is_heap(_opts) {
                        w.line(&format!("original.{fname}.resize({min});"));
                    } else {
                        w.line(&format!("original.{fname}_count = {min};"));
                    }
                }
            }
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                // Skip optional and repeated fields
                if !matches!(f.occurrence, Occurrence::Required) {
                    continue;
                }
                // Resolve tagged inner type
                let effective_ty = match &f.ty {
                    TypeRef::Tagged(_, inner) => inner.as_ref(),
                    other => other,
                };
                // A member pinned to one value (`always: true`, `version: 1`) must use
                // it, or its own decoder rejects the roundtrip (dgtlrift/cddlc#35).
                if let Some(lit) = f.constraints.iter().find_map(|c| match c {
                    Constraint::Eq(LiteralValue::Bool(b)) => Some(b.to_string()),
                    Constraint::Eq(LiteralValue::Uint(n)) => Some(format!("{n}u")),
                    Constraint::Eq(LiteralValue::Int(n)) => Some(format!("{n}")),
                    Constraint::Eq(LiteralValue::Text(t)) => {
                        Some(format!("std::string_view({t:?})"))
                    }
                    _ => None,
                }) {
                    w.line(&format!("original.{fname} = {lit};"));
                    continue;
                }
                // A value its range, `.size` or `.regexp` allows.
                if let TypeRef::Primitive(p) = effective_ty {
                    if let Some(v) = cddlc_codegen::constrained_sample(p, &f.constraints) {
                        match v {
                            cddlc_codegen::SampleValue::Text(t) => {
                                w.line(&format!("static const char _{fname}_str[] = {t:?};"));
                                w.line(&format!(
                                    "original.{fname} = std::string_view(_{fname}_str);"
                                ));
                            }
                            other => {
                                w.line(&format!("original.{fname} = {};", cpp_sample_lit(&other)))
                            }
                        }
                        continue;
                    }
                }
                // `.bits`: the mask itself is a value it allows (dgtlrift/cddlc#107).
                if let Some(bits) = f.constraints.iter().find_map(|c| match c {
                    Constraint::Bits(b) => Some(b),
                    _ => None,
                }) {
                    match effective_ty {
                        TypeRef::Primitive(Primitive::Uint) => {
                            w.line(&format!(
                                "original.{fname} = {:#x}ULL;",
                                cddlc_codegen::bits_mask_u64(bits)
                            ));
                            continue;
                        }
                        TypeRef::Primitive(Primitive::Bstr) => {
                            let m = cddlc_codegen::bits_mask_bytes(bits);
                            let bytes = m
                                .iter()
                                .map(|b| format!("{b:#04x}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            w.line(&format!(
                                "static const uint8_t _{fname}_cbor[{}] = {{{bytes}}};",
                                m.len().max(1)
                            ));
                            w.line(&format!("original.{fname} = std::string_view(reinterpret_cast<const char*>(_{fname}_cbor), {});", m.len()));
                            continue;
                        }
                        _ => {}
                    }
                }
                match effective_ty {
                    TypeRef::Primitive(Primitive::Uint) => {
                        w.line(&format!("original.{fname} = 42u;"));
                    }
                    TypeRef::Primitive(Primitive::Int) => {
                        w.line(&format!("original.{fname} = -1;"));
                    }
                    TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => {
                        w.line(&format!("original.{fname} = 1.0f;"));
                    }
                    TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => {
                        w.line(&format!("original.{fname} = 1.0;"));
                    }
                    TypeRef::Primitive(Primitive::Bool) => {
                        w.line(&format!("original.{fname} = true;"));
                    }
                    TypeRef::Primitive(Primitive::Tstr) => {
                        w.line(&format!("static const char _{fname}_str[] = \"hello\";"));
                        w.line(&format!(
                            "original.{fname} = std::string_view(_{fname}_str);"
                        ));
                    }
                    TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => {
                        w.line(&format!(
                            "static const uint8_t _{fname}_cbor[1] = {{0xf6}};"
                        ));
                        w.line(&format!("original.{fname} = std::string_view(reinterpret_cast<const char*>(_{fname}_cbor), 1);"));
                    }
                    TypeRef::Choice(_) => match effective_primitive(effective_ty) {
                        Some(Primitive::Tstr) => {
                            w.line(&format!("static const char _{fname}_str[] = \"hello\";"));
                            w.line(&format!(
                                "original.{fname} = std::string_view(_{fname}_str);"
                            ));
                        }
                        Some(Primitive::Bstr | Primitive::Any) => {
                            w.line(&format!(
                                "static const uint8_t _{fname}_cbor[1] = {{0xf6}};"
                            ));
                            w.line(&format!("original.{fname} = std::string_view(reinterpret_cast<const char*>(_{fname}_cbor), 1);"));
                        }
                        _ => {}
                    },
                    TypeRef::Named(n) => {
                        if let Some(sz) = tstr_size_constraint_for(n, module) {
                            let s = "a".repeat(sz);
                            w.line(&format!("{{ static const char _{fname}_str[] = {s:?}; original.{fname} = {}{{ std::string_view(_{fname}_str, {sz}) }}; }}", to_pascal_case(n)));
                        }
                        // Otherwise leave as default {}
                    }
                    _ => {
                        // Leave as default {}
                    }
                }
            }
        }
        // A table with one entry under a text key.
        // A value that holds the table again has no valid default (its
        // required `Box` is empty), so that table stays empty
        // (dgtlrift/cddlc#156).
        TypeDef::Map(m)
            if matches!(m.key_ty, TypeRef::Primitive(Primitive::Tstr))
                && cddlc_codegen::is_indirect(&m.name, &m.val_ty, module) =>
        {
            w.line(&format!("{type_name} original{{}};"));
        }
        TypeDef::Map(m) if matches!(m.key_ty, TypeRef::Primitive(Primitive::Tstr)) => {
            w.line(&format!("{type_name} original{{}};"));
            // An `any` value is an item: `null`, as an empty one writes nothing.
            let val = match &m.val_ty {
                TypeRef::Primitive(Primitive::Any) => "std::string_view(\"\\xf6\", 1)".to_owned(),
                // A named value fills its required boxed members
                // (dgtlrift/cddlc#156; RFC 8927's `mapping`).
                TypeRef::Named(n) => cpp_filled(n, module),
                _ => "{}".to_owned(),
            };
            if is_heap(_opts) {
                w.line(&format!(
                    "original.entries.push_back({{std::string_view(\"k\"), {val}}});"
                ));
            } else {
                w.line(&format!(
                    "original.entries[0] = {{std::string_view(\"k\"), {val}}};"
                ));
                w.line("original.count = 1;");
            }
        }
        _ => {
            w.line(&format!("{type_name} original{{}};"));
        }
    }

    w.line("nanocbor_encoder_t enc;");
    w.line("nanocbor_encoder_init(&enc, buf, sizeof(buf));");
    w.line("assert(encode(&enc, original) == 0);");
    w.line("std::size_t enc_len = nanocbor_encoded_len(&enc);");
    w.blank();
    w.line(&format!("{type_name} decoded{{}};"));
    w.line("nanocbor_value_t dec;");
    w.line("nanocbor_decoder_init(&dec, buf, enc_len);");
    w.line("assert(decode(&dec, decoded, test_cb) == 0);");
    // An enum decodes to the first variant that takes the item, which may be
    // an earlier one with the same members (dgtlrift/cddlc#192), or one that
    // takes any map (WebDriver BiDi's `EmptyResult = { Extensible }`): it
    // decodes back to the same encoding.
    let ambiguous = matches!(def, TypeDef::Enum(_));
    if ambiguous {
        w.line("uint8_t buf2[sizeof(buf)];");
        w.line("nanocbor_encoder_t enc2;");
        w.line("nanocbor_encoder_init(&enc2, buf2, sizeof(buf2));");
        w.line("assert(encode(&enc2, decoded) == 0);");
        w.line("assert(nanocbor_encoded_len(&enc2) == enc_len && std::memcmp(buf, buf2, enc_len) == 0);");
    } else {
        w.line("assert(original == decoded);");
    }
    if enum_loop {
        w.dedent();
        w.line("}");
    }
    w.line(&format!("std::printf(\"{fn_name}_roundtrip: PASS\\n\");"));

    w.dedent();
    w.line("}");
}

// ── C++ type helpers ──────────────────────────────────────────────────────────

fn cpp_field_type(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    let base = cpp_typeref(ty, opts);
    match occ {
        Occurrence::Optional => format!("std::optional<{base}>"),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. }
            if is_heap(opts) =>
        {
            format!("std::vector<{base}>")
        }
        Occurrence::ZeroOrMore { capacity }
        | Occurrence::OneOrMore { capacity }
        | Occurrence::Bounded { capacity, .. } => {
            let cap = capacity_value(capacity, opts.max_array);
            format!("std::array<{base}, {cap}>")
        }
        Occurrence::Required => base,
    }
}

fn cpp_typeref(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => cpp_primitive(p, opts).to_owned(),
        TypeRef::Named(n) => to_pascal_case(n),
        TypeRef::Tagged(_, inner) => cpp_typeref(inner, opts),
        TypeRef::Choice(cs) => match prim_choice(cs, opts) {
            Some(alts) => format!(
                "std::variant<{}>",
                alts.iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => cs
                .first()
                .map(|c| cpp_typeref(c, opts))
                .unwrap_or_else(|| "std::monostate".into()),
        },
    }
}

/// A choice of primitives with distinct C++ types (`int / float`), which is a
/// `std::variant` of them: each alternative and its C++ type. `None` for any
/// other choice, which keeps its first alternative's type.
fn prim_choice(cs: &[TypeRef], opts: &CodegenOptions) -> Option<Vec<(Primitive, String)>> {
    let mut alts: Vec<(Primitive, String)> = Vec::new();
    for c in cs {
        let TypeRef::Primitive(p) = c else {
            return None;
        };
        if matches!(p, Primitive::Any) {
            return None;
        }
        let t = cpp_primitive(p, opts).to_owned();
        if alts.iter().any(|(_, u)| *u == t) {
            return None;
        }
        alts.push((p.clone(), t));
    }
    (alts.len() > 1).then_some(alts)
}

fn cpp_primitive<'a>(p: &Primitive, _opts: &CodegenOptions) -> &'a str {
    match p {
        Primitive::Bool => "bool",
        Primitive::Null | Primitive::Undefined => "std::monostate",
        Primitive::Uint => "uint64_t",
        Primitive::Int => "int64_t",
        Primitive::Float16 | Primitive::Float32 => "float",
        Primitive::Float64 | Primitive::Float => "double",
        Primitive::Tstr => "std::string_view",
        Primitive::Bstr | Primitive::Any => "std::string_view",
    }
}

// ── C++ JSON backend (nlohmann/json) ─────────────────────────────────────────

fn emit_cmake_json_cpp(name: &str, opts: &CodegenOptions) -> String {
    let header = cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Hash);
    format!(
        r#"{header}cmake_minimum_required(VERSION 3.16)
project({name}_json CXX)

set(CMAKE_CXX_STANDARD 17)

include(FetchContent)
# nlohmann/json v3.12.0 release archive, pinned by hash (dgtlrift/cddlc#51)
FetchContent_Declare(nlohmann_json
    URL      https://github.com/nlohmann/json/releases/download/v3.12.0/json.tar.xz
    URL_HASH SHA256=42f6e95cad6ec532fd372391373363b62a14af6d771056dbfc86160e6dfff7aa
)
set(JSON_BuildTests OFF CACHE INTERNAL "")
FetchContent_MakeAvailable(nlohmann_json)

add_library({name}_json
    src/{name}.cpp
)
target_include_directories({name}_json PUBLIC include)
target_link_libraries({name}_json PUBLIC nlohmann_json::nlohmann_json)

enable_testing()
add_executable(test_roundtrip test/test_roundtrip.cpp)
target_link_libraries(test_roundtrip PRIVATE {name}_json)
add_test(NAME roundtrip COMMAND test_roundtrip)
"#
    )
}

fn emit_header_json_cpp(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let guard = to_screaming_snake(&name);
    let ns = cddlc_codegen::cpp_ident(&name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line("// Format: JSON (RFC 8259)");
    w.line(&format!("#ifndef {guard}_JSON_HPP"));
    w.line(&format!("#define {guard}_JSON_HPP"));
    w.blank();
    w.line("#include <cstdint>");
    w.line("#include <map>");
    w.line("#include <string>");
    w.line("#include <vector>");
    w.line("#include <optional>");
    w.line("#include <variant>");
    w.line("#include <new>");
    w.line("#include <utility>");
    w.line("#include <stdexcept>");
    w.line("#include <nlohmann/json.hpp>");
    w.blank();
    w.line(&format!("namespace {ns} {{"));
    w.blank();

    // Base64url helpers
    w.line("std::string b64url_encode(const std::vector<uint8_t>& data);");
    w.line("std::vector<uint8_t> b64url_decode(const std::string& s);");
    w.blank();

    // A type that holds itself again does so through a `Box`, declared
    // ahead (dgtlrift/cddlc#156).
    if BOXED.with(|b| !b.borrow().is_empty()) {
        for (_, def) in &module.types {
            if matches!(def, TypeDef::Struct(_))
                || matches!(def, TypeDef::Enum(e) if !e.variants.iter().all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr))))
            {
                w.line(&format!("struct {};", to_pascal_case(def.name())));
            }
        }
        w.blank();
        for line in BOX_TEMPLATE.lines() {
            if line.is_empty() {
                w.blank()
            } else {
                w.line(line)
            }
        }
        w.blank();
    }

    // Emit type declarations, after what each holds by value
    for tname in cddlc_codegen::inline_decl_order(module) {
        let Some(def) = module.get(&tname) else {
            continue;
        };
        emit_cpp_json_type_decl(&mut w, def, module, opts);
        w.blank();
    }

    w.line(&format!("}} // namespace {ns}"));
    w.blank();
    w.line(&format!("#endif // {guard}_JSON_HPP"));
    w.finish()
}

fn cpp_json_field_type(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Bool => "bool".into(),
            Primitive::Uint => "uint64_t".into(),
            Primitive::Int => "int64_t".into(),
            Primitive::Float32 | Primitive::Float16 => "float".into(),
            Primitive::Float64 | Primitive::Float => "double".into(),
            Primitive::Tstr => "std::string".into(),
            Primitive::Bstr => "std::vector<uint8_t>".into(),
            // Any JSON value (dgtlrift/cddlc#178).
            Primitive::Any => "nlohmann::json".into(),
            Primitive::Null | Primitive::Undefined => "std::monostate".into(),
        },
        TypeRef::Named(n) => to_pascal_case(n),
        TypeRef::Tagged(_, inner) => cpp_json_field_type(inner, opts),
        TypeRef::Choice(_) => "nlohmann::json".into(),
    }
}

fn cpp_json_field_type_with_occ(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    let base = cpp_json_field_type(ty, opts);
    match occ {
        Occurrence::Optional => format!("std::optional<{base}>"),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => format!("std::vector<{base}>"),
        Occurrence::Required => base,
    }
}

fn emit_cpp_json_type_decl(
    w: &mut IndentWriter,
    def: &TypeDef,
    module: &IrModule,
    opts: &CodegenOptions,
) {
    match def {
        TypeDef::Struct(s) => {
            let ctype = to_pascal_case(&s.name);
            cddlc_codegen::emit_doc_lines(
                w,
                s.doc.as_deref(),
                cddlc_codegen::DocStyle::Line("///"),
            );
            w.line(&format!("struct {ctype} {{"));
            w.indent();
            for f in &s.fields {
                cddlc_codegen::emit_doc_lines(
                    w,
                    f.doc.as_deref(),
                    cddlc_codegen::DocStyle::Line("///"),
                );
                let fname = cddlc_codegen::cpp_ident(&f.name);
                // A member that holds this type again is a `Box`, empty when
                // an optional one is absent (dgtlrift/cddlc#156).
                let ty = if is_boxed(&s.name, &f.name) {
                    format!("Box<{}>", json_box_target(&f.ty, module, opts))
                } else {
                    cpp_json_field_type_with_occ(&f.ty, &f.occurrence, opts)
                };
                w.line(&format!("{ty} {fname}{};", cpp_pinned_init(f, true)));
            }
            // The map's other entries (dgtlrift/cddlc#82).
            if let Some(r) = &s.rest {
                w.line("/// The map's other entries.");
                w.line(&format!(
                    "std::map<std::string, {}> {}{{}};",
                    cpp_json_field_type(&r.val_ty, opts),
                    cddlc_codegen::cpp_ident(&r.name)
                ));
            }
            w.blank();
            let eq: Vec<String> = s
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .chain(s.rest.iter().map(|r| r.name.as_str()))
                .map(|n| {
                    let n = cddlc_codegen::cpp_ident(n);
                    format!("{n} == o.{n}")
                })
                .collect();
            let eq = if eq.is_empty() {
                "true".to_owned()
            } else {
                eq.join(" && ")
            };
            w.line(&format!(
                "bool operator==(const {ctype} &o) const {{ return {eq}; }}"
            ));
            w.line("std::vector<uint8_t> encode() const;");
            w.line(&format!(
                "static {ctype} decode(const std::vector<uint8_t>& data);"
            ));
            w.dedent();
            w.line("};");
        }
        TypeDef::Enum(e) => {
            let ctype = to_pascal_case(&e.name);
            cddlc_codegen::emit_doc_lines(
                w,
                e.doc.as_deref(),
                cddlc_codegen::DocStyle::Line("///"),
            );
            // Emit as std::variant for mixed enums or string enum
            let all_str = e
                .variants
                .iter()
                .all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
            if all_str {
                let variants: Vec<String> = e
                    .variants
                    .iter()
                    .map(|v| format!("{:?}", cddlc_codegen::enum_variant_text(v)))
                    .collect();
                w.line(&format!(
                    "// {ctype}: string enum ({} variants)",
                    variants.len()
                ));
                w.line(&format!("using {ctype} = std::string;"));
            } else {
                // Emit as tagged union via JSON
                w.line(&format!("struct {ctype} {{"));
                w.indent();
                w.line("nlohmann::json value;");
                w.blank();
                w.line(&format!(
                    "bool operator==(const {ctype} &o) const {{ return value == o.value; }}"
                ));
                w.line("std::vector<uint8_t> encode() const;");
                w.line(&format!(
                    "static {ctype} decode(const std::vector<uint8_t>& data);"
                ));
                w.dedent();
                w.line("};");
            }
        }
        TypeDef::Array(a) => {
            let ctype = to_pascal_case(&a.name);
            cddlc_codegen::emit_doc_lines(
                w,
                a.doc.as_deref(),
                cddlc_codegen::DocStyle::Line("///"),
            );
            let elem_ty = cpp_json_field_type(&a.element, opts);
            w.line(&format!("using {ctype} = std::vector<{elem_ty}>;"));
            w.line(&format!(
                "std::vector<uint8_t> encode_{ctype}(const {ctype}& val);"
            ));
            w.line(&format!(
                "{ctype} decode_{ctype}(const std::vector<uint8_t>& data);"
            ));
        }
        TypeDef::Alias(a) => {
            let ctype = to_pascal_case(&a.name);
            cddlc_codegen::emit_doc_lines(
                w,
                a.doc.as_deref(),
                cddlc_codegen::DocStyle::Line("///"),
            );
            let inner_ty = cpp_json_field_type(&a.ty, opts);
            w.line(&format!("struct {ctype} {{"));
            w.indent();
            w.line(&format!("{inner_ty} value{{}};"));
            w.blank();
            w.line(&format!(
                "bool operator==(const {ctype} &o) const {{ return value == o.value; }}"
            ));
            w.line("std::vector<uint8_t> encode() const;");
            w.line(&format!(
                "static {ctype} decode(const std::vector<uint8_t>& data);"
            ));
            w.dedent();
            w.line("};");
        }
        // A table: a `std::map` from text key (JSON keys are text), written in
        // key order (dgtlrift/cddlc#82).
        TypeDef::Map(m) => {
            let ctype = to_pascal_case(&m.name);
            cddlc_codegen::emit_doc_lines(
                w,
                m.doc.as_deref(),
                cddlc_codegen::DocStyle::Line("///"),
            );
            let val_ty = cpp_json_field_type(&m.val_ty, opts);
            w.line(&format!("using {ctype} = std::map<std::string, {val_ty}>;"));
            w.line(&format!(
                "std::vector<uint8_t> encode_{ctype}(const {ctype}& val);"
            ));
            w.line(&format!(
                "{ctype} decode_{ctype}(const std::vector<uint8_t>& data);"
            ));
        }
    }
}

fn emit_source_json_cpp(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = cddlc_codegen::cpp_ident(&name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.hpp\""));
    w.line("#include <cstring>");
    if module_uses_regex(module) {
        w.line("#include <regex>");
    }
    w.blank();
    w.line(&format!("namespace {ns} {{"));
    w.blank();

    // Base64url impl
    w.line("static const char B64URL[] =");
    w.line("    \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_\";");
    w.blank();
    w.line("std::string b64url_encode(const std::vector<uint8_t>& data) {");
    w.indent();
    w.line("std::string out;");
    w.line("size_t i = 0, n = data.size();");
    w.line("while (i + 2 < n) {");
    w.indent();
    w.line("out += B64URL[(data[i] >> 2) & 0x3F];");
    w.line("out += B64URL[((data[i] << 4) | (data[i+1] >> 4)) & 0x3F];");
    w.line("out += B64URL[((data[i+1] << 2) | (data[i+2] >> 6)) & 0x3F];");
    w.line("out += B64URL[data[i+2] & 0x3F];");
    w.line("i += 3;");
    w.dedent();
    w.line("}");
    w.line("if (i < n) {");
    w.indent();
    w.line("out += B64URL[(data[i] >> 2) & 0x3F];");
    w.line("if (i+1 < n) { out += B64URL[((data[i]<<4)|(data[i+1]>>4))&0x3F]; out += B64URL[(data[i+1]<<2)&0x3F]; }");
    w.line("else { out += B64URL[(data[i] << 4) & 0x3F]; }");
    w.dedent();
    w.line("}");
    w.line("return out;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("std::vector<uint8_t> b64url_decode(const std::string& s) {");
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
    w.line("std::vector<uint8_t> out;");
    w.line("size_t i = 0, n = s.size();");
    w.line("while (i + 3 < n) {");
    w.indent();
    w.line("int a=DEC[(uint8_t)s[i]],b=DEC[(uint8_t)s[i+1]],c=DEC[(uint8_t)s[i+2]],d=DEC[(uint8_t)s[i+3]];");
    w.line("if(a<0||b<0||c<0||d<0) break;");
    w.line("out.push_back((a<<2)|(b>>4)); out.push_back((b<<4)|(c>>2)); out.push_back((c<<6)|d);");
    w.line("i += 4;");
    w.dedent();
    w.line("}");
    // The unpadded tail: 2 characters are one byte, 3 are two.
    w.line("if (n - i >= 2) {");
    w.indent();
    w.line("int a=DEC[(uint8_t)s[i]],b=DEC[(uint8_t)s[i+1]];");
    w.line("if(a>=0&&b>=0) {");
    w.indent();
    w.line("out.push_back((a<<2)|(b>>4));");
    w.line(
        "if (n - i >= 3) { int c=DEC[(uint8_t)s[i+2]]; if(c>=0) out.push_back((b<<4)|(c>>2)); }",
    );
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");
    w.line("return out;");
    w.dedent();
    w.line("}");
    w.blank();

    w.line("/// A JSON value's serialized bytes, as the `decode` functions take them.");
    w.line("[[maybe_unused]] static std::vector<uint8_t> json_bytes(const nlohmann::json& j) {");
    w.indent();
    w.line("auto s = j.dump();");
    w.line("return std::vector<uint8_t>(s.begin(), s.end());");
    w.dedent();
    w.line("}");
    w.blank();

    for (_, def) in &module.types {
        emit_cpp_json_impl(&mut w, def, opts, module);
        w.blank();
    }

    w.line(&format!("}} // namespace {ns}"));
    w.finish()
}

/// How a named type is represented in C++ JSON output.
enum JsonNamed {
    TextEnum,
    Array,
    Object,
}

fn json_named_kind(n: &str, module: &IrModule) -> JsonNamed {
    match module.get(n) {
        Some(TypeDef::Enum(e)) if json_enum_is_string(e) => JsonNamed::TextEnum,
        Some(TypeDef::Array(_) | TypeDef::Map(_)) => JsonNamed::Array,
        _ => JsonNamed::Object,
    }
}

/// A text-only enum is emitted as `using X = std::string`.
fn json_enum_is_string(e: &EnumDef) -> bool {
    e.variants
        .iter()
        .all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)))
}

/// A C++ expression converting `expr` to a `nlohmann::json` value.
fn cpp_json_to_val(ty: &TypeRef, expr: &str, module: &IrModule) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Bstr) => format!("b64url_encode({expr})"),
        TypeRef::Primitive(Primitive::Any) => expr.to_owned(),
        // `std::monostate` has no JSON conversion (dgtlrift/cddlc#54).
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => "nullptr".into(),
        TypeRef::Named(n) => match json_named_kind(n, module) {
            JsonNamed::TextEnum => expr.to_owned(),
            JsonNamed::Array => format!(
                "nlohmann::json::parse(encode_{}({expr}))",
                to_pascal_case(n)
            ),
            JsonNamed::Object => format!("nlohmann::json::parse({expr}.encode())"),
        },
        TypeRef::Tagged(_, inner) => cpp_json_to_val(inner, expr, module),
        _ => expr.to_owned(),
    }
}

/// A C++ expression converting the `nlohmann::json` value `expr` to its type.
fn cpp_json_from_val(ty: &TypeRef, expr: &str, opts: &CodegenOptions, module: &IrModule) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Uint) => format!("{expr}.get<uint64_t>()"),
        TypeRef::Primitive(Primitive::Int) => format!("{expr}.get<int64_t>()"),
        TypeRef::Primitive(Primitive::Bool) => format!("{expr}.get<bool>()"),
        TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => {
            format!("{expr}.get<float>()")
        }
        TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => {
            format!("{expr}.get<double>()")
        }
        TypeRef::Primitive(Primitive::Tstr) => format!("{expr}.get<std::string>()"),
        TypeRef::Primitive(Primitive::Bstr) => {
            format!("b64url_decode({expr}.get<std::string>())")
        }
        TypeRef::Primitive(Primitive::Any) => expr.to_owned(),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => "std::monostate{}".into(),
        // `json_bytes` serializes once; building a vector from `dump().begin()` and
        // `dump().end()` used iterators of two different strings (dgtlrift/cddlc#54).
        TypeRef::Named(n) => match json_named_kind(n, module) {
            JsonNamed::TextEnum => format!("{expr}.get<std::string>()"),
            JsonNamed::Array => format!("decode_{}(json_bytes({expr}))", to_pascal_case(n)),
            JsonNamed::Object => format!("{}::decode(json_bytes({expr}))", to_pascal_case(n)),
        },
        TypeRef::Tagged(_, inner) => cpp_json_from_val(inner, expr, opts, module),
        TypeRef::Choice(_) => expr.to_owned(),
    }
}

fn emit_cpp_json_impl(
    w: &mut IndentWriter,
    def: &TypeDef,
    opts: &CodegenOptions,
    module: &IrModule,
) {
    match def {
        // A record with a repeated member (`[primary, * extension, payload]`):
        // its items go inline, and the array's length says how many there
        // are (dgtlrift/cddlc#134).
        TypeDef::Struct(s) if cddlc_codegen::record_repeated(s).is_some() => {
            let ri = cddlc_codegen::record_repeated(s).expect("checked");
            let ctype = to_pascal_case(&s.name);
            let rf = &s.fields[ri];
            let rname = cddlc_codegen::cpp_ident(&rf.name);
            let fixed = s.fields.len() - 1;
            let (min, max) = cddlc_codegen::repeat_bounds(&rf.occurrence);
            let bounds = |w: &mut IndentWriter, n: &str| {
                if min > 0 {
                    w.line(&format!(
                        "if ({n} < {min}) throw std::runtime_error({:?});",
                        format!("{ctype}.{}: needs at least {min} items", rf.name)
                    ));
                }
                if let Some(max) = max {
                    w.line(&format!(
                        "if ({n} > {max}) throw std::runtime_error({:?});",
                        format!("{ctype}.{}: has more than {max} items", rf.name)
                    ));
                }
            };
            w.line(&format!("std::vector<uint8_t> {ctype}::encode() const {{"));
            w.indent();
            bounds(w, &format!("this->{rname}.size()"));
            w.line("nlohmann::json j = nlohmann::json::array();");
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                if f.occurrence.is_repeated() {
                    w.line(&format!(
                        "for (const auto& v : this->{fname}) j.push_back({});",
                        cpp_json_to_val(&f.ty, "v", module)
                    ));
                } else {
                    w.line(&format!(
                        "j.push_back({});",
                        cpp_json_to_val(&f.ty, &json_member(&s.name, f), module)
                    ));
                }
            }
            w.line("auto s = j.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();
            w.line(&format!(
                "{ctype} {ctype}::decode(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!("if (!j.is_array() || j.size() < {fixed}) {{"));
            w.line(&format!(
                "    throw std::runtime_error({:?});",
                format!("{ctype}: expected an array of at least {fixed} items")
            ));
            w.line("}");
            w.line(&format!("std::size_t reps = j.size() - {fixed};"));
            bounds(w, "reps");
            w.line(&format!("{ctype} out{{}};"));
            for (i, f) in s.fields.iter().enumerate() {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                if f.occurrence.is_repeated() {
                    let conv = cpp_json_from_val(&f.ty, &format!("j.at({i} + k)"), opts, module);
                    w.line(&format!(
                        "for (std::size_t k = 0; k < reps; k++) out.{fname}.push_back({conv});"
                    ));
                } else {
                    let index = if i < ri {
                        i.to_string()
                    } else {
                        format!("{} + reps", i - 1)
                    };
                    let conv = cpp_json_from_val(&f.ty, &format!("j.at({index})"), opts, module);
                    w.line(&format!("out.{fname} = {conv};"));
                }
            }
            w.line("return out;");
            w.dedent();
            w.line("}");
        }
        TypeDef::Struct(s) if s.encoding == cddlc_ir::StructEncoding::Array => {
            // A record (RFC 8610 §3.4): a JSON array of the members' values, in
            // order; optional members come last, and one present after an absent
            // one can't be written.
            let ctype = to_pascal_case(&s.name);
            let optional: Vec<_> = s
                .fields
                .iter()
                .filter(|f| matches!(f.occurrence, Occurrence::Optional))
                .collect();
            w.line(&format!("std::vector<uint8_t> {ctype}::encode() const {{"));
            w.indent();
            for pair in optional.windows(2) {
                w.line(&format!(
                    "if (this->{}.has_value() && !this->{}.has_value()) throw std::runtime_error({:?});",
                    cddlc_codegen::cpp_ident(&pair[1].name), cddlc_codegen::cpp_ident(&pair[0].name),
                    format!("{ctype}.{}: set, but an optional member before it isn't", pair[1].name)
                ));
            }
            w.line("nlohmann::json j = nlohmann::json::array();");
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                if matches!(f.occurrence, Occurrence::Optional) {
                    w.line(&format!(
                        "if (this->{fname}.has_value()) j.push_back({});",
                        cpp_json_to_val(&f.ty, &format!("this->{fname}.value()"), module)
                    ));
                } else {
                    w.line(&format!(
                        "j.push_back({});",
                        cpp_json_to_val(&f.ty, &json_member(&s.name, f), module)
                    ));
                }
            }
            w.line("auto s = j.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();

            let required = s.fields.len() - optional.len();
            w.line(&format!(
                "{ctype} {ctype}::decode(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!(
                "if (!j.is_array() || j.size() < {required} || j.size() > {}) {{",
                s.fields.len()
            ));
            w.line(&format!(
                "    throw std::runtime_error({:?});",
                format!(
                    "{ctype}: expected an array of {required} to {} items",
                    s.fields.len()
                )
            ));
            w.line("}");
            w.line(&format!("{ctype} out{{}};"));
            // One optional member before required ones: present when the array
            // holds every member; those after it move down one when it's absent.
            let before = cddlc_codegen::record_optionals_before(s);
            for (i, f) in s.fields.iter().enumerate() {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                let index = match &before {
                    Some(b) if b[i] > 0 => {
                        let x = required + b[i];
                        format!("j.size() >= {x} ? {i} : {i} - ({x} - j.size())")
                    }
                    _ => i.to_string(),
                };
                let conv = cpp_json_from_val(&f.ty, &format!("j.at({index})"), opts, module);
                if let (Occurrence::Optional, Some(b)) = (&f.occurrence, &before) {
                    w.line(&format!(
                        "if (j.size() > {}) out.{fname} = {conv};",
                        required + b[i]
                    ));
                } else if matches!(f.occurrence, Occurrence::Optional) {
                    w.line(&format!("if (j.size() > {i}) out.{fname} = {conv};"));
                } else {
                    w.line(&format!("out.{fname} = {conv};"));
                }
            }
            w.line("return out;");
            w.dedent();
            w.line("}");
        }
        TypeDef::Struct(s) => {
            let ctype = to_pascal_case(&s.name);
            w.line(&format!("std::vector<uint8_t> {ctype}::encode() const {{"));
            w.indent();
            w.line("nlohmann::json j = nlohmann::json::object();");
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                let key_text = cddlc_codegen::field_key_json(&f.key);
                let key = key_text.as_str();
                let _val = cpp_json_to_val(&f.ty, &fname, module);
                match &f.occurrence {
                    Occurrence::Optional => {
                        w.line(&format!(
                            "if ({fname}.has_value()) j[{key:?}] = {val};",
                            fname = format!("this->{fname}"),
                            val = cpp_json_to_val(&f.ty, &format!("this->{fname}.value()"), module)
                        ));
                    }
                    _ => {
                        w.line(&format!(
                            "j[{key:?}] = {};",
                            cpp_json_to_val(&f.ty, &json_member(&s.name, f), module)
                        ));
                    }
                }
            }
            if let Some(r) = &s.rest {
                let named: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("{:?}", cddlc_codegen::field_key_json(&f.key)))
                    .collect();
                w.line(&format!(
                    "for (const auto& [key, value] : this->{}) {{",
                    cddlc_codegen::cpp_ident(&r.name)
                ));
                w.indent();
                if !named.is_empty() {
                    w.line(&format!(
                        "for (const char* n : {{{}}}) if (key == n) throw std::invalid_argument(\"{ctype}: other entry \" + key + \" is a named member's key\");",
                        named.join(", ")
                    ));
                }
                w.line(&format!(
                    "j[key] = {};",
                    cpp_json_to_val(&r.val_ty, "value", module)
                ));
                w.dedent();
                w.line("}");
            }
            w.line("auto s = j.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();

            w.line(&format!(
                "{ctype} {ctype}::decode(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!("{ctype} out{{}};"));
            // A map has only the members its schema gives it (dgtlrift/cddlc#102).
            if s.rest.is_none() {
                let named: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("key == {:?}", cddlc_codegen::field_key_json(&f.key)))
                    .collect();
                w.line(&format!(
                    "for (const auto& [key, value] : j.items()) if (!({})) throw std::invalid_argument(\"{ctype}: unknown member \" + key);",
                    if named.is_empty() { "false".into() } else { named.join(" || ") }
                ));
            }
            for f in &s.fields {
                let fname = cddlc_codegen::cpp_ident(&f.name);
                let key_text = cddlc_codegen::field_key_json(&f.key);
                let key = key_text.as_str();
                match &f.occurrence {
                    Occurrence::Optional => {
                        w.line(&format!(
                            "if (j.contains({key:?})) out.{fname} = {};",
                            cpp_json_from_val(&f.ty, &format!("j[{key:?}]"), opts, module)
                        ));
                    }
                    _ => {
                        w.line(&format!(
                            "out.{fname} = {};",
                            cpp_json_from_val(&f.ty, &format!("j.at({key:?})"), opts, module)
                        ));
                    }
                }
                // Its constraints, as CBOR checks them on decode.
                emit_cpp_json_checks(w, &ctype, f);
            }
            // Keys no member has are the map's other entries.
            if let Some(r) = &s.rest {
                let named: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("key == {:?}", cddlc_codegen::field_key_json(&f.key)))
                    .collect();
                w.line("for (const auto& [key, value] : j.items()) {");
                w.indent();
                if !named.is_empty() {
                    w.line(&format!("if ({}) continue;", named.join(" || ")));
                }
                w.line(&format!(
                    "out.{}.emplace(key, {});",
                    cddlc_codegen::cpp_ident(&r.name),
                    cpp_json_from_val(&r.val_ty, "value", opts, module)
                ));
                w.dedent();
                w.line("}");
            }
            w.line("return out;");
            w.dedent();
            w.line("}");
        }
        TypeDef::Enum(e) => {
            let ctype = to_pascal_case(&e.name);
            let all_str = e
                .variants
                .iter()
                .all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));

            if all_str {
                // string enum — type alias, no methods needed
            } else {
                w.line(&format!("std::vector<uint8_t> {ctype}::encode() const {{"));
                w.indent();
                w.line("auto s = value.dump();");
                w.line("return std::vector<uint8_t>(s.begin(), s.end());");
                w.dedent();
                w.line("}");
                w.blank();

                w.line(&format!(
                    "{ctype} {ctype}::decode(const std::vector<uint8_t>& data) {{"
                ));
                w.indent();
                w.line(&format!("{ctype} out{{}};"));
                w.line("out.value = nlohmann::json::parse(data.begin(), data.end());");
                w.line("return out;");
                w.dedent();
                w.line("}");
            }
        }
        TypeDef::Array(a) => {
            let ctype = to_pascal_case(&a.name);
            let elem_ty = cpp_json_field_type(&a.element, opts);
            w.line(&format!(
                "std::vector<uint8_t> encode_{ctype}(const {ctype}& val) {{"
            ));
            w.indent();
            // Element by element, so named elements use their own conversion.
            w.line("nlohmann::json arr = nlohmann::json::array();");
            let flat = flat_record(a, module).map(|r| r.fields.len());
            if flat.is_some() {
                // A repeated group: each record's members, inline (dgtlrift/cddlc#90).
                w.line(&format!(
                    "for (const auto& item : val) for (auto& m : {}) arr.push_back(m);",
                    cpp_json_to_val(&a.element, "item", module)
                ));
            } else {
                w.line(&format!(
                    "for (const auto& item : val) arr.push_back({});",
                    cpp_json_to_val(&a.element, "item", module)
                ));
            }
            w.line("auto s = arr.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();

            w.line(&format!(
                "{ctype} decode_{ctype}(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!("{ctype} out;"));
            if let Some(n) = flat {
                w.line(&format!(
                    "if (!j.is_array() || j.size() % {n} != 0) throw std::runtime_error(\"{ctype}: a partial group\");"
                ));
                w.line(&format!(
                    "for (std::size_t i = 0; i < j.size(); i += {n}) {{"
                ));
                w.indent();
                w.line("nlohmann::json item = nlohmann::json::array();");
                w.line(&format!(
                    "for (std::size_t k = 0; k < {n}; k++) item.push_back(j[i + k]);"
                ));
                w.line(&format!(
                    "out.push_back({});",
                    cpp_json_from_val(&a.element, "item", opts, module)
                ));
                w.dedent();
                w.line("}");
            } else {
                w.line(&format!(
                    "for (const auto& item : j) out.push_back({});",
                    cpp_json_from_val(&a.element, "item", opts, module)
                ));
            }
            w.line("return out;");
            w.dedent();
            w.line("}");

            let _ = elem_ty;
        }
        TypeDef::Alias(a) => {
            let ctype = to_pascal_case(&a.name);
            w.line(&format!("std::vector<uint8_t> {ctype}::encode() const {{"));
            w.indent();
            let val = cpp_json_to_val(&a.ty, "this->value", module);
            w.line(&format!("nlohmann::json j = {val};"));
            w.line("auto s = j.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();

            w.line(&format!(
                "{ctype} {ctype}::decode(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!("{ctype} out{{}};"));
            w.line(&format!(
                "out.value = {};",
                cpp_json_from_val(&a.ty, "j", opts, module)
            ));
            w.line("return out;");
            w.dedent();
            w.line("}");
        }
        TypeDef::Map(m) => {
            let ctype = to_pascal_case(&m.name);
            w.line(&format!(
                "std::vector<uint8_t> encode_{ctype}(const {ctype}& val) {{"
            ));
            w.indent();
            w.line("nlohmann::json obj = nlohmann::json::object();");
            w.line(&format!(
                "for (const auto& [key, value] : val) obj[key] = {};",
                cpp_json_to_val(&m.val_ty, "value", module)
            ));
            w.line("auto s = obj.dump();");
            w.line("return std::vector<uint8_t>(s.begin(), s.end());");
            w.dedent();
            w.line("}");
            w.blank();

            w.line(&format!(
                "{ctype} decode_{ctype}(const std::vector<uint8_t>& data) {{"
            ));
            w.indent();
            w.line("auto j = nlohmann::json::parse(data.begin(), data.end());");
            w.line(&format!("if (!j.is_object()) throw std::runtime_error(\"{ctype}: expected a JSON object\");"));
            w.line(&format!("{ctype} out;"));
            w.line(&format!(
                "for (const auto& [key, value] : j.items()) out.emplace(key, {});",
                cpp_json_from_val(&m.val_ty, "value", opts, module)
            ));
            w.line("return out;");
            w.dedent();
            w.line("}");
        }
    }
}

fn emit_tests_json_cpp(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let ns = cddlc_codegen::cpp_ident(&name);
    let mut w = IndentWriter::new(4);

    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
    w.line(&format!("#include \"{name}.hpp\""));
    w.line("#include <cassert>");
    w.line("#include <cstdio>");
    w.blank();
    w.line(&format!("using namespace {ns};"));
    w.blank();

    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                let ctype = to_pascal_case(&s.name);
                w.line(&format!("void test_{ctype}() {{"));
                w.indent();
                w.line(&format!("{ctype} original{{}};"));
                if let Some(r) = &s.rest {
                    w.line(&format!(
                        "original.{}[\"other-key\"] = {{}};",
                        cddlc_codegen::cpp_ident(&r.name)
                    ));
                }
                // A required boxed member holds a value (dgtlrift/cddlc#156).
                for f in s
                    .fields
                    .iter()
                    .filter(|f| f.occurrence.is_required() && is_boxed(&s.name, &f.name))
                {
                    if let Some(n) = boxed_named(&f.ty, opts) {
                        w.line(&format!(
                            "original.{} = {};",
                            cddlc_codegen::cpp_ident(&f.name),
                            cpp_filled_json(n, module)
                        ));
                    }
                }
                // A required member with a range, `.size` or `.regexp` holds a
                // value it allows: JSON checks them on decode.
                for f in s.fields.iter().filter(|f| f.occurrence.is_required()) {
                    // A member pinned to one value holds it.
                    if let Some(lit) = f.constraints.iter().find_map(|c| match c {
                        Constraint::Eq(LiteralValue::Bool(b)) => Some(b.to_string()),
                        Constraint::Eq(LiteralValue::Uint(n)) => Some(format!("{n}ULL")),
                        Constraint::Eq(LiteralValue::Int(n)) => Some(format!("{n}LL")),
                        Constraint::Eq(LiteralValue::Text(t)) => {
                            Some(format!("std::string({t:?})"))
                        }
                        _ => None,
                    }) {
                        w.line(&format!(
                            "original.{} = {lit};",
                            cddlc_codegen::cpp_ident(&f.name)
                        ));
                        continue;
                    }
                    if let TypeRef::Primitive(p) = &f.ty {
                        if let Some(v) = cddlc_codegen::constrained_sample(p, &f.constraints) {
                            w.line(&format!(
                                "original.{} = {};",
                                cddlc_codegen::cpp_ident(&f.name),
                                cpp_sample_lit(&v)
                            ));
                        }
                    }
                }
                // A repeated member gets as many (default) items as it needs
                // (dgtlrift/cddlc#134).
                for f in s.fields.iter().filter(|f| f.occurrence.is_repeated()) {
                    let min = cddlc_codegen::repeat_bounds(&f.occurrence).0;
                    if min > 0 {
                        w.line(&format!(
                            "original.{}.resize({min});",
                            cddlc_codegen::cpp_ident(&f.name)
                        ));
                    }
                }
                w.line("auto encoded = original.encode();");
                w.line(&format!("auto decoded = {ctype}::decode(encoded);"));
                w.line("assert(decoded == original);");
                w.dedent();
                w.line("}");
                w.blank();
            }
            TypeDef::Enum(e) => {
                let ctype = to_pascal_case(&e.name);
                let all_str = e
                    .variants
                    .iter()
                    .all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
                if !all_str {
                    w.line(&format!("void test_{ctype}() {{"));
                    w.indent();
                    w.line(&format!("{ctype} original{{}};"));
                    w.line("auto encoded = original.encode();");
                    w.line(&format!("auto decoded = {ctype}::decode(encoded);"));
                    w.line("assert(decoded == original);");
                    w.dedent();
                    w.line("}");
                    w.blank();
                }
            }
            TypeDef::Array(a) => {
                let ctype = to_pascal_case(&a.name);
                w.line(&format!("void test_{ctype}() {{"));
                w.indent();
                w.line(&format!("{ctype} original{{}};"));
                w.line(&format!("auto encoded = encode_{ctype}(original);"));
                w.line(&format!("auto decoded = decode_{ctype}(encoded);"));
                w.line("assert(decoded == original);");
                w.dedent();
                w.line("}");
                w.blank();
            }
            TypeDef::Alias(a) => {
                let ctype = to_pascal_case(&a.name);
                w.line(&format!("void test_{ctype}() {{"));
                w.indent();
                w.line(&format!("{ctype} original{{}};"));
                w.line("auto encoded = original.encode();");
                w.line(&format!("auto decoded = {ctype}::decode(encoded);"));
                w.line("assert(decoded == original);");
                w.dedent();
                w.line("}");
                w.blank();
            }
            TypeDef::Map(m) => {
                let ctype = to_pascal_case(&m.name);
                w.line(&format!("void test_{ctype}() {{"));
                w.indent();
                w.line(&format!("{ctype} original{{}};"));
                // A value that holds the table again has no valid default
                // (dgtlrift/cddlc#156).
                if !cddlc_codegen::is_indirect(&m.name, &m.val_ty, module) {
                    w.line("original[\"k\"] = {};");
                }
                w.line(&format!("auto encoded = encode_{ctype}(original);"));
                w.line(&format!("auto decoded = decode_{ctype}(encoded);"));
                w.line("assert(decoded == original);");
                w.dedent();
                w.line("}");
                w.blank();
            }
        }
        let _ = opts;
    }

    w.line("int main() {");
    w.indent();
    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => {
                let ctype = to_pascal_case(&s.name);
                w.line(&format!("test_{ctype}();"));
            }
            TypeDef::Enum(e) => {
                let ctype = to_pascal_case(&e.name);
                let all_str = e
                    .variants
                    .iter()
                    .all(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
                if !all_str {
                    w.line(&format!("test_{ctype}();"));
                }
            }
            TypeDef::Array(a) => {
                let ctype = to_pascal_case(&a.name);
                w.line(&format!("test_{ctype}();"));
            }
            TypeDef::Alias(a) => {
                let ctype = to_pascal_case(&a.name);
                w.line(&format!("test_{ctype}();"));
            }
            TypeDef::Map(m) => {
                let ctype = to_pascal_case(&m.name);
                w.line(&format!("test_{ctype}();"));
            }
        }
    }
    w.line("std::printf(\"All C++ JSON roundtrip tests passed.\\n\");");
    w.line("return 0;");
    w.dedent();
    w.line("}");

    w.finish()
}
