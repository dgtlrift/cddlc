//! Where a test value's nesting stops, an array or table is empty, not
//! null, as a required one must be there (dgtlrift/cddlc#245).

use std::path::PathBuf;

use backend_java::JavaBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = JavaBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const NODE: &str = "node = { name: tstr, kids: [* node], ? attrs: { * tstr => node } }";

#[test]
fn a_nested_value_ends_with_empty_arrays_and_tables() {
    let g = generate(NODE);
    assert!(!g.contains(NULL_ENTRY), "{g}");
    assert!(g.contains(EMPTY_TABLE), "{g}");
}

const NULL_ENTRY: &str = "Map.of(\"hello\", null)";
const EMPTY_TABLE: &str = "new NodeAttrs(Map.of())";
