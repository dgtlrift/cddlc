//! A rule named like a type the generated Dart uses is renamed
//! `<module>-<rule>` (dgtlrift/cddlc#222).

use backend_dart::DartBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;
use std::path::PathBuf;

/// All generated files, concatenated.
fn compile(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = DartBackend.generate(&lowered.module, &CodegenOptions::default()).expect("generate");
    out.files.iter().map(|f| f.content.as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_rule_named_like_a_core_type_is_renamed() {
    let g = compile("holder = { a: object, b: error }\nobject = { x: int }\nerror = { code: int }");
    assert!(g.contains("class TestObject"), "{g}");
    assert!(g.contains("class TestError"), "{g}");
    assert!(!g.contains("class Object"), "{g}");
    assert!(!g.contains("class Error"), "{g}");
}

#[test]
fn a_rule_named_like_a_cbor_or_test_type_is_renamed() {
    let g = compile("holder = { a: cbor-value, b: timeout }\ncbor-value = { x: int }\ntimeout = { s: uint }");
    assert!(g.contains("class TestCborValue"), "{g}");
    assert!(g.contains("class TestTimeout"), "{g}");
}

#[test]
fn other_rules_keep_their_names() {
    let g = compile("reading = { x: int }");
    assert!(g.contains("class Reading"), "{g}");
}
