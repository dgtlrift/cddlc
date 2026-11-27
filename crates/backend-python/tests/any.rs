//! `any` holds the value cbor2 or json decodes the item to (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_python::PythonBackend;
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
    let out = PythonBackend
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
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(g.contains("extra: Optional[Any] = None"), "{g}");
        assert!(!g.contains("_bytes(raw.get(\"extra\")"), "{g}");
        // A test value that isn't a byte string.
        assert!(g.contains("[1, \"a\"]"), "{g}");
    }
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("return cls(kind=\"Any\", value=raw)"), "{g}");
    assert!(!g.contains("no variant matches"), "{g}");
}
