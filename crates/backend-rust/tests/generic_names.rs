//! Rules named like the generated code's generic parameters (`w`, `c`, `s`,
//! `d`, `v`) don't clash with them (dgtlrift/cddlc#145).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

const SRC: &str = "w = \"a\" / tstr\nc = { x: uint, ? y: w }\ns = [ a: uint, b: tstr ]\nd = { + tstr => uint }\nv = [ x: c, y: s ]\n";

fn lib(format: Format) -> String {
    let parsed = parse_cddl(SRC, PathBuf::from("test.cddl")).expect("parse");
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

#[test]
fn cbor_impls_use_generic_names_no_rule_has() {
    let out = lib(Format::Cbor);
    assert!(out.contains("impl<CddlcC> Encode<CddlcC> for W {"), "{out}");
    assert!(
        out.contains("fn encode<CddlcW: Write>(&self, e: &mut Encoder<CddlcW>, ctx: &mut CddlcC)"),
        "{out}"
    );
    assert!(
        !out.contains("<W: Write>") && !out.contains("impl<C> "),
        "{out}"
    );
}

#[test]
fn json_impls_use_generic_names_no_rule_has() {
    let out = lib(Format::Json);
    assert!(
        !out.contains("<S: serde::Serializer>")
            && !out.contains("<D: serde::Deserializer")
            && !out.contains("'de, D:"),
        "{out}"
    );
    assert!(out.contains("CddlcD: serde::Deserializer"), "{out}");
    assert!(!out.contains("struct V;"), "{out}");
}
