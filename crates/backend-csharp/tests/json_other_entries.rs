//! In C#'s JSON output a map's other entries hold their type, as in CBOR,
//! converted around the serializer's extension data.

use std::path::PathBuf;

use backend_csharp::CSharpBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format: Format::Json,
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

#[test]
fn other_entries_hold_their_type() {
    let g = generate("r = { id: uint, * tstr => uint }");
    assert!(
        g.contains("public sealed class R : IJsonOnSerializing, IJsonOnDeserialized"),
        "{g}"
    );
    assert!(
        g.contains("public Dictionary<string, ulong> Extra { get; set; } = new();"),
        "{g}"
    );
    assert!(
        g.contains("internal Dictionary<string, JsonElement>? ExtraJson { get; set; }"),
        "{g}"
    );
    assert!(
        g.contains("void IJsonOnDeserialized.OnDeserialized()"),
        "{g}"
    );
}

#[test]
fn a_map_without_other_entries_has_no_callbacks() {
    let g = generate("r = { id: uint }");
    assert!(!g.contains("IJsonOnDeserialized"), "{g}");
}
