//! `any` holds the encoded data item in Go (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_go::GoBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The package source and its tests.
fn generate(src: &str, format: Format) -> (String, String) {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    let out = GoBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    (out.files[1].content.clone(), out.files[2].content.clone())
}

const SRC: &str = "hdr = { alg: int, ? extra: any }\nvalue = int / any";

#[test]
fn any_is_the_raw_item() {
    let (src, tests) = generate(SRC, Format::Cbor);
    assert!(src.contains("type RawItem = cbor.RawMessage"), "{src}");
    assert!(
        src.contains("func decItem(r []byte, at string) (RawItem, error) {"),
        "{src}"
    );
    assert!(
        tests.contains("func sampleItem() RawItem { return RawItem{0x82, 0x01, 0x61, 0x61} }"),
        "{tests}"
    );
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    let (src, _) = generate(SRC, Format::Cbor);
    assert!(src.contains("x, _ := decItem(r, "), "{src}");
}

#[test]
fn json_holds_the_raw_json_value() {
    let (src, tests) = generate(SRC, Format::Json);
    assert!(src.contains("type RawItem = json.RawMessage"), "{src}");
    assert!(
        tests.contains("func nullItem() RawItem { return RawItem(\"null\") }"),
        "{tests}"
    );
}

#[test]
fn no_raw_item_without_any() {
    let (src, _) = generate("r = { a: bstr }", Format::Cbor);
    assert!(!src.contains("RawItem"), "{src}");
}
