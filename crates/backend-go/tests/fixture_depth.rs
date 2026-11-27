//! Where a test value's nesting stops, it decodes back to itself: required
//! slices, maps and byte strings are empty, not nil (dgtlrift/cddlc#226).

use std::path::PathBuf;

use backend_go::GoBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The generated tests.
fn tests(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = GoBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    out.files[2].content.clone()
}

#[test]
fn a_nested_value_ends_with_empty_slices_not_nil() {
    let t = tests("node = { name: tstr, kids: [* node], ? attrs: { * tstr => node } }");
    // The innermost Node has its kids, an empty slice, and no attrs.
    assert!(t.contains("Node{Kids: NodeKids{}}"), "{t}");
    assert!(!t.contains("Node{}"), "{t}");
}

#[test]
fn a_nested_struct_fills_its_own_slices() {
    let t = tests("a = { b: b, ? next: a }\nb = { items: [* uint], raw: bstr }");
    // The innermost A holds a B whose slice and bytes are empty, not nil.
    assert!(t.contains("A{B: B{Items: BItems{}, Raw: []byte{}}}"), "{t}");
    assert!(!t.contains("A{}"), "{t}");
}
