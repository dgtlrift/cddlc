//! C++ fixes found by building the published schemas (RFC 9052, 8927, 9321,
//! WebDriver BiDi).

use std::path::PathBuf;

use backend_cpp::CppBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, format: Format) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    let out = CppBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_keyword_member_gets_an_underscore() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate("r = { protected: uint, this: tstr }", format);
        assert!(g.contains("uint64_t protected_{};"), "{g}");
        assert!(!g.contains(" protected{};"), "{g}");
    }
}

#[test]
fn a_rule_named_like_a_runtime_type_is_renamed() {
    let g = generate(
        "holder = { e: error-code }\nerror-code = uint",
        Format::Cbor,
    );
    assert!(g.contains("TErrorCode"), "{g}");
}

#[test]
fn another_choice_is_written_and_read_as_its_first_alternative() {
    let g = generate("r = { ? d: (float .gt 0.0) / null }", Format::Cbor);
    assert!(!g.contains("encode(enc, (*val.d));"), "{g}");
    assert!(!g.contains("// choice"), "{g}");
}

#[test]
fn a_pinned_member_holds_its_value_by_default() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate("r = { type: \"start\", n: uint }", format);
        assert!(g.contains(" type{\"start\"};"), "{g}");
    }
}

#[test]
fn an_any_table_test_value_is_an_item() {
    let g = generate("t = { * tstr => any }", Format::Cbor);
    assert!(g.contains("std::string_view(\"\\xf6\", 1)"), "{g}");
}

#[test]
fn a_choice_test_compares_its_encoding() {
    let g = generate("c = { a: uint } / { ? b: uint }", Format::Cbor);
    assert!(
        g.contains("assert(nanocbor_encoded_len(&enc2) == enc_len"),
        "{g}"
    );
}
