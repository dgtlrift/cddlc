//! A choice's `tstr` alternative holds its text, apart from its text
//! literals (dgtlrift/cddlc#234).

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

#[test]
fn a_tstr_alternative_holds_its_text() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate("label = \"none\" / tstr", format);
        assert!(
            g.contains("{ readonly kind: \"Tstr\"; readonly value: string }"),
            "{g}"
        );
        assert!(
            g.contains("case \"none\": return { kind: \"None\" };"),
            "{g}"
        );
        assert!(
            g.contains("default: return { kind: \"Tstr\", value: inner };"),
            "{g}"
        );
        assert!(!g.contains("\"tstr\""), "{g}");
    }
}

#[test]
fn cbor_writes_the_held_text() {
    let g = generate("label = \"none\" / tstr", Format::Cbor);
    assert!(g.contains("enc.text((val as any).value); break;"), "{g}");
}
