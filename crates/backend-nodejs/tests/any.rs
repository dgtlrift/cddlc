//! `any` holds the value cborg or JSON.parse decodes the item to
//! (dgtlrift/cddlc#178).

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

const SRC: &str = "hdr = { alg: int, ? extra: any }\nvalue = int / any";

#[test]
fn any_is_the_decoded_value() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("readonly extra?: unknown;"), "{g}");
    assert!(g.contains("enc.writeBytes(cborgEncode("), "{g}");
    // A map in it may have integer keys.
    assert!(g.contains("useMaps: true"), "{g}");
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("return { kind: \"Any\", value: inner };"), "{g}");
    assert!(
        g.contains("{ kind: \"Any\", value: [1, 'a'] } as Value"),
        "{g}"
    );
}

#[test]
fn json_holds_the_parsed_value() {
    let g = generate(SRC, Format::Json);
    assert!(g.contains("readonly extra?: unknown;"), "{g}");
    assert!(!g.contains("fromBase64Url(String("), "{g}");
}
