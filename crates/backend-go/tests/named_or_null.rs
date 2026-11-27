//! A choice of one type and `null` is a pointer to the type, nil when null,
//! and test values stop nesting at the `null` (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_go::GoBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The package source and its tests.
fn generate(src: &str) -> (String, String) {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = GoBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    (out.files[1].content.clone(), out.files[2].content.clone())
}

#[test]
fn a_member_of_a_type_or_null_is_a_pointer() {
    let (src, tests) =
        generate("info = { id: tstr, children: info-list / null }\ninfo-list = [* info]");
    assert!(
        src.lines()
            .any(|l| l.split_whitespace().eq(["Children", "*InfoList"])),
        "{src}"
    );
    assert!(
        src.contains("if v.Children == nil { return encNull() }"),
        "{src}"
    );
    assert!(src.contains("if kindOf(r) == kindNull {"), "{src}");
    assert!(tests.contains("Children: ptr("), "{tests}");
}

#[test]
fn a_recursive_choice_stops_nesting_at_null() {
    let (_, tests) = generate("link = next / null\nnext = { value: int, link: link }");
    assert!(tests.contains("NewLinkNull()"), "{tests}");
}
