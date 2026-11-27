//! C# fixes found by building the published schemas (RFC 9597, 9594,
//! WebDriver BiDi).

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

#[test]
fn a_map_missing_a_required_member_is_rejected() {
    let cbor = generate("r = { a: tstr, ? b: uint }", Format::Cbor);
    assert!(
        cbor.contains("if (!have0) throw new CborContentException(\"R: missing member a\");"),
        "{cbor}"
    );
    let json = generate("r = { a: tstr, ? b: uint }", Format::Json);
    assert!(json.contains("[JsonRequired]"), "{json}");
}

#[test]
fn a_named_table_key_goes_through_its_json() {
    let g = generate(
        "claims = { * label => any }\nlabel = int / tstr",
        Format::Json,
    );
    assert!(g.contains("Label.FromJson(Cddl_.KeyJson(p.Name))"), "{g}");
    assert!(g.contains("Cddl_.KeyText(kv.Key.ToJson())"), "{g}");
}

#[test]
fn a_variant_named_like_a_member_gets_an_underscore() {
    let g = generate(
        "action = \"default\" / \"cancel\"\nr = { a: action }",
        Format::Cbor,
    );
    assert!(g.contains("public sealed class Default_ : Action"), "{g}");
}

#[test]
fn a_nested_choice_is_flattened_in_a_switch() {
    let g = generate(
        "r = { v: (int / float) / tstr / sensor }\nsensor = { id: uint }",
        Format::Cbor,
    );
    assert!(!g.contains("case object "), "{g}");
}
