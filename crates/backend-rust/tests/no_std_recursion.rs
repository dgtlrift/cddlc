//! With `--no-std`, an array or table holds its items inline, so a type that
//! holds itself through one is boxed where it does (dgtlrift/cddlc#245).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The generated lib.rs.
fn lib_rs(src: &str, no_std: bool) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        no_std,
        ..CodegenOptions::default()
    };
    RustBackend
        .generate(&lowered.module, &opts)
        .expect("generate")
        .files[1]
        .content
        .clone()
}

const NODE: &str = "node = { name: tstr, kids: [* node], ? attrs: { * tstr => node } }";

#[test]
fn no_std_boxes_recursion_through_an_array_or_table() {
    let g = lib_rs(NODE, true);
    assert!(g.contains("pub kids: Box<NodeKids>,"), "{g}");
    assert!(g.contains("pub attrs: Option<Box<NodeAttrs>>,"), "{g}");
}

#[test]
fn std_needs_no_box_there() {
    let g = lib_rs(NODE, false);
    assert!(g.contains("pub kids: NodeKids,"), "{g}");
    assert!(!g.contains("Box<NodeKids>"), "{g}");
}
