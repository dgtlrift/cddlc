//! `any` holds the runtime's item (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_lua::LuaBackend;
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
    let out = LuaBackend
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
fn any_is_the_item() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(g.contains("R.want_item(v.extra, "), "{g}");
        assert!(g.contains("m[#m + 1] = { \"extra\", v.extra }"), "{g}");
        assert!(
            g.contains(
                "{ k = \"array\", v = { { k = \"uint\", v = 1 }, { k = \"text\", v = \"a\" } } }"
            ),
            "{g}"
        );
    }
}

#[test]
fn an_any_variant_takes_what_the_others_dont() {
    let g = generate(SRC, Format::Cbor);
    assert!(!g.contains("no variant matches"), "{g}");
}

#[test]
fn cbor_keeps_float16_bits_and_undefined() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("w = 16, bits = h"), "{g}");
    assert!(
        g.contains("item.undefined and \"\\xf7\" or \"\\xf6\""),
        "{g}"
    );
}
