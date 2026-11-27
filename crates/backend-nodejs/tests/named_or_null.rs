//! A choice of one type and `null` converts through the type unless it's
//! null, a member that may be null is missing only when absent, and test
//! values stop nesting at the `null` (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_nodejs::NodeJsBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, format: Format) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    let out = NodeJsBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const INFO: &str = "info = { id: tstr, children: info-list / null }\ninfo-list = [* info]";
const LINK: &str = "link = next / null\nnext = { value: int, link: link }";

#[test]
fn a_member_of_a_type_or_null_converts_through_the_type() {
    let g = generate(INFO, Format::Cbor);
    assert!(
        g.contains("if (val.children === null) { enc.null(); } else {"),
        "{g}"
    );
    assert!(
        g.contains("if (m[\"children\"] === undefined) throw new Error('Missing field children');"),
        "{g}"
    );
    assert!(!g.contains("cborgEncode(val.children)"), "{g}");
}

#[test]
fn a_member_of_a_nullable_enum_may_be_null() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(LINK, format);
        assert!(
            g.contains("if (m[\"link\"] === undefined) throw new Error('Missing field link');"),
            "{g}"
        );
        assert!(!g.contains("{} as Next"), "{g}");
    }
}
