//! An enum's `Default` is a variant that doesn't hold the enum again, and a
//! named variant isn't JSON null (dgtlrift/cddlc#224).

use std::path::PathBuf;

use backend_csharp::CSharpBackend;
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
    let out = CSharpBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const LINK: &str = "link = next / null\nnext = { value: int, link: link }";

#[test]
fn a_recursive_first_variant_isnt_the_default() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(LINK, format);
        assert!(
            g.contains("public static Link Default => Link.Null.Instance;"),
            "{g}"
        );
    }
}

#[test]
fn a_named_variant_isnt_json_null() {
    let g = generate(LINK, Format::Json);
    assert!(g.contains("j => j.ValueKind == JsonValueKind.Null ? throw new JsonException(\"null\") : new Next("), "{g}");
}
