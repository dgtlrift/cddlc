//! An enum that keys a table is ordered (dgtlrift/cddlc#140).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn lib(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    RustBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate")
        .files[1]
        .content
        .clone()
}

#[test]
fn primitive_enums_are_ordered() {
    // RFC 9597's `{ * Claim-Label => any }`.
    let out =
        lib("claims = { * claim-label => any }\nclaim-label = int / text\nkind = \"a\" / \"b\"");
    assert!(
        out.contains(
            "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]\npub enum ClaimLabel"
        ),
        "{out}"
    );
    assert!(
        out.contains("Eq, PartialOrd, Ord)]\npub enum Kind"),
        "{out}"
    );
}

#[test]
fn enums_with_floats_or_types_are_not() {
    let out = lib("n = int / float\nm = int / r\nr = { a: uint }");
    assert!(
        out.contains("#[derive(Debug, Clone, PartialEq)]\npub enum N "),
        "{out}"
    );
    assert!(
        out.contains("#[derive(Debug, Clone, PartialEq)]\npub enum M "),
        "{out}"
    );
}
