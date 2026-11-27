//! `any` holds the encoded data item in C# (dgtlrift/cddlc#178), and a rule
//! named like a generated class's member is renamed.

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

const SRC: &str = "hdr = { alg: int, ? extra: any }\nchoice = int / any";

#[test]
fn cbor_holds_the_items_encoding() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("public byte[]? Extra { get; set; }"), "{g}");
    assert!(g.contains("reader.ReadEncodedValue().ToArray()"), "{g}");
    assert!(g.contains("writer.WriteEncodedValue("), "{g}");
    // A test value is an item that isn't a byte string.
    assert!(g.contains("new byte[] { 0x82, 0x01, 0x61, 0x61 }"), "{g}");
}

#[test]
fn json_holds_any_json_value() {
    let g = generate(SRC, Format::Json);
    assert!(g.contains("public JsonElement? Extra { get; set; }"), "{g}");
    assert!(
        !g.contains("[JsonConverter(typeof(Base64UrlJsonConverter))]\n    public JsonElement"),
        "{g}"
    );
}

#[test]
fn a_rule_named_like_a_generated_member_is_renamed() {
    let g = generate("holder = { a: items }\nitems = [* uint]", Format::Cbor);
    assert!(g.contains("public sealed class TItems"), "{g}");
    assert!(!g.contains("public sealed class Items"), "{g}");
}
