//! A rule named like the generated package's `Null` type gets `Null_`
//! (dgtlrift/cddlc#149).

use std::path::PathBuf;

use backend_go::GoBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

#[test]
fn a_rule_named_null_gets_a_trailing_underscore() {
    let parsed = parse_cddl(
        "Null = { b: tstr }\nr = { n: Null, z: null }",
        PathBuf::from("t.cddl"),
    )
    .expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = GoBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    let src = out
        .files
        .iter()
        .find(|f| f.path.ends_with(".go") && !f.path.ends_with("_test.go"))
        .unwrap()
        .content
        .clone();
    assert!(src.contains("type Null_ struct {"), "{src}");
    assert!(src.contains("type Null struct{}"), "{src}");
    assert!(src.contains("N Null_"), "{src}");
    assert!(src.contains("Z Null"), "{src}");
}
