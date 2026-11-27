//! An enum's `Default` is a variant that doesn't hold the enum again, and a
//! boxed variant's default is boxed (dgtlrift/cddlc#228).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The generated lib.rs.
fn lib_rs(src: &str, format: Format) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    RustBackend
        .generate(&lowered.module, &opts)
        .expect("generate")
        .files[1]
        .content
        .clone()
}

const LINK: &str = "link = next / null\nnext = { value: int, link: link }";

#[test]
fn a_recursive_first_variant_isnt_the_default() {
    for format in [Format::Cbor, Format::Json] {
        let g = lib_rs(LINK, format);
        assert!(
            g.contains("impl Default for Link {\n    fn default() -> Self {\n        Link::Null\n"),
            "{g}"
        );
    }
}

#[test]
fn a_boxed_default_is_boxed() {
    // Every variant holds the enum again, so the first is the default.
    let g = lib_rs(
        "expr = neg / not\nneg = { x: expr }\nnot = { y: expr }",
        Format::Cbor,
    );
    assert!(g.contains("Expr::Neg(Box::new(Neg::default()))"), "{g}");
}

#[test]
fn any_isnt_the_default_beside_another_variant() {
    // `any`'s default item is `null`, which the `nil` variant would take.
    let g = lib_rs("ratio = any / null\nr = { ? ratio: ratio }", Format::Cbor);
    assert!(
        g.contains("impl Default for Ratio {\n    fn default() -> Self {\n        Ratio::Null\n"),
        "{g}"
    );
}
