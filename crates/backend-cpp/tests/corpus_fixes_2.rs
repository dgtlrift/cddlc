//! More C++ fixes from building the published schemas (RFC 9052, 9393,
//! 9164, 8927): defaults and test values that encode.

use std::path::PathBuf;

use backend_cpp::CppBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated (CBOR).
fn generate(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CppBackend
        .generate(&m, &CodegenOptions::default())
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn an_any_defaults_to_null() {
    let g = generate("values = any\nr = { v: any }");
    assert!(g.contains("std::string_view value{\"\\xf6\", 1};"), "{g}");
    assert!(g.contains("std::string_view v{\"\\xf6\", 1};"), "{g}");
}

#[test]
fn a_sized_byte_string_defaults_to_zero_bytes() {
    let g = generate("addr = bytes .size 4");
    assert!(
        g.contains("value{std::string_view(\"\\x00\\x00\\x00\\x00\", 4)};"),
        "{g}"
    );
}

#[test]
fn an_all_valued_recursive_choice_is_filled() {
    let g = generate("t = { * tstr => v }\nv = a / b\na = { x: v }\nb = { y: v }");
    assert!(g.contains("V::make_A(Box<A>("), "{g}");
}
