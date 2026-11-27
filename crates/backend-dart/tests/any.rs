//! `any` holds package:cbor's value of the item, or the JSON value
//! (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_dart::DartBackend;
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
    let out = DartBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const SRC: &str = "hdr = { alg: int, ? extra: any }\nvalue = int / any";

#[test]
fn cbor_holds_the_cbor_value() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("final CborValue? extra;"), "{g}");
    assert!(!g.contains(".toObject()"), "{g}");
    // The helper a choice's `any` variant writes through, though no tag is used.
    assert!(
        g.contains("CborValue _withTags(List<int> tags, CborValue v) {"),
        "{g}"
    );
    assert!(g.contains("import 'package:cbor/cbor.dart';"), "{g}");
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(g.contains("return Value._(\"any\", "), "{g}");
    }
}

#[test]
fn json_holds_the_json_value() {
    let g = generate(SRC, Format::Json);
    assert!(
        g.contains("final dynamic extra;") || g.contains("final dynamic? extra;"),
        "{g}"
    );
}
