//! `any` holds the encoded data item in C++ (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_cpp::CppBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, opts: &CodegenOptions) -> String {
    let parsed = parse_cddl(src, PathBuf::from("any.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CppBackend.generate(&m, opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const SRC: &str = "hdr = { alg: int, ? extra: any }\nvalue = int / tstr / any";

#[test]
fn a_member_reads_and_writes_the_whole_item() {
    let g = generate(SRC, &CodegenOptions::default());
    assert!(
        g.contains("std::optional<std::string_view> extra{};"),
        "{g}"
    );
    assert!(g.contains("nanocbor_get_subcbor("), "{g}");
    assert!(
        g.contains("inline int cddlc_put_item(nanocbor_encoder_t *enc, std::string_view item) {"),
        "{g}"
    );
}

#[test]
fn an_any_alternative_takes_what_the_others_dont() {
    let g = generate(SRC, &CodegenOptions::default());
    assert!(g.contains("const nanocbor_value_t _start = *dec;"), "{g}");
    assert!(g.contains("goto cddlc_any;"), "{g}");
    assert!(g.contains("cddlc_any:\n    *dec = _start;"), "{g}");
    assert!(
        g.contains(
            "if (cddlc_put_item(enc, std::get<std::string_view>(val.value)) < 0) return -1;"
        ),
        "{g}"
    );
    // Its test value is an item no other alternative takes.
    assert!(
        g.contains(r#"Value::make_Any(std::string_view("\xf5", 1))"#),
        "{g}"
    );
}

#[test]
fn no_helper_without_any() {
    let g = generate("r = { a: bstr }", &CodegenOptions::default());
    assert!(!g.contains("cddlc_put_item"), "{g}");
}

#[test]
fn json_holds_any_json_value() {
    let g = generate(
        SRC,
        &CodegenOptions {
            format: Format::Json,
            ..CodegenOptions::default()
        },
    );
    assert!(g.contains("std::optional<nlohmann::json> extra{};"), "{g}");
}
