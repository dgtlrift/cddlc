//! `any` holds the runtime's raw value of the item (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_java::JavaBackend;
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
    let out = JavaBackend
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
fn any_is_the_raw_value() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(g.contains("Object extra"), "{g}");
        assert!(!g.contains("Cddl.asBytes(m.get(\"extra\")"), "{g}");
        assert!(g.contains("java.util.List.of(1L, \"a\")"), "{g}");
    }
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("return new Any(v);"), "{g}");
}
