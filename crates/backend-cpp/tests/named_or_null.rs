//! A required boxed member's test value is an enum variant that doesn't
//! nest, not a default with an empty `Box` (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_cpp::CppBackend;
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
    let out = CppBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const LINK: &str = "link = next / null\nnext = { value: int, link: link }";

#[test]
fn a_boxed_enum_member_takes_a_variant_that_doesnt_nest() {
    let g = generate(LINK, Format::Cbor);
    assert!(g.contains("original.link = Link::make_Null();"), "{g}");
    assert!(!g.contains("original.link = Link{};"), "{g}");
}

#[test]
fn json_keeps_the_enums_default() {
    // A JSON enum holds an `nlohmann::json`, and has no `make_` functions.
    let g = generate(LINK, Format::Json);
    assert!(!g.contains("make_Null"), "{g}");
}
