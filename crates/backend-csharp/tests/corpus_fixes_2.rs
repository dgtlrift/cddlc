//! More C# fixes from building the published schemas (RFC 9164, 9171,
//! 9594).

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
fn a_tagged_enum_writes_its_tag() {
    let g = generate(
        "ip = #6.54(addr / prefix)\naddr = bytes .size 16\nprefix = [uint, bytes]",
        Format::Cbor,
    );
    assert!(g.contains("writer.WriteTag((CborTag)54);"), "{g}");
}

#[test]
fn a_named_null_variant_takes_json_null() {
    let g = generate(
        "info = req / resp\nreq = null\nresp = [+ uint]",
        Format::Json,
    );
    assert!(
        !g.contains("? throw new JsonException(\"null\") : new Req("),
        "{g}"
    );
    assert!(
        g.contains("? throw new JsonException(\"null\") : new Resp("),
        "{g}"
    );
}

#[test]
fn a_named_decode_is_qualified() {
    let g = generate(
        "bundle = [block, * extension-block]\nblock = uint\nextension-block = [uint]",
        Format::Cbor,
    );
    assert!(g.contains("global::T.ExtensionBlock.Decode(reader)"), "{g}");
}
