//! A member of a named type or `null` is NULL, or flagged, when null; and
//! a pointer test target is filled so it encodes (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_c::CBackend;
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
    let out = CBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_pointer_member_is_null_when_null() {
    let g = generate(
        "info = { id: tstr, children: info-list / null }\ninfo-list = [* info]",
        Format::Cbor,
    );
    assert!(
        g.contains("if (!val->children) { nanocbor_fmt_null(enc); } else {"),
        "{g}"
    );
    assert!(
        g.contains("if (nanocbor_get_null(&map) >= 0) { out->children = NULL; } else {"),
        "{g}"
    );
    assert!(!g.contains("/* 'children' is required */"), "{g}");
}

#[test]
fn a_value_member_has_a_null_flag() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate("b = { s: sensor / null }\nsensor = { id: uint }", format);
        assert!(g.contains("bool s_null;"), "{g}");
    }
    let g = generate(
        "b = { s: sensor / null }\nsensor = { id: uint }",
        Format::Cbor,
    );
    assert!(
        g.contains("if (val->s_null) { nanocbor_fmt_null(enc); } else {"),
        "{g}"
    );
}

#[test]
fn a_pointer_target_takes_a_variant_that_doesnt_nest() {
    let g = generate(
        "link = next / null\nnext = { value: int, link: link }",
        Format::Cbor,
    );
    assert!(g.contains(".tag = LINK_NULL;"), "{g}");
}
