//! A choice of one type and `null` converts through the type unless it's
//! None, and test values stop nesting at the `null` (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_python::PythonBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = PythonBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_member_of_a_type_or_null_converts_through_the_type() {
    let g = generate("info = { id: tstr, children: info-list / null }\ninfo-list = [* info]");
    assert!(
        g.contains("(None if self.children is None else self.children._to_raw())"),
        "{g}"
    );
    assert!(g.contains("InfoList._from_raw("), "{g}");
}

#[test]
fn a_recursive_choice_stops_nesting_at_null() {
    let g = generate("link = next / null\nnext = { value: int, link: link }");
    assert!(!g.contains("make_Next(None)"), "{g}");
    assert!(g.contains("Link.make_Null()"), "{g}");
}
