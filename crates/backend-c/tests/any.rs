//! `any` holds the encoded data item in C (dgtlrift/cddlc#178).

use std::path::PathBuf;

use backend_c::CBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, opts: &CodegenOptions) -> String {
    let parsed = parse_cddl(src, PathBuf::from("any.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CBackend.generate(&m, opts).expect("generate");
    out.files.iter().map(|f| f.content.clone()).collect::<Vec<_>>().join("\n")
}

const SRC: &str = "hdr = { alg: int, ? extra: any }\nvalue = int / tstr / any";

fn json() -> CodegenOptions {
    CodegenOptions { format: Format::Json, ..CodegenOptions::default() }
}

#[test]
fn a_member_reads_and_writes_the_whole_item() {
    let g = generate(SRC, &CodegenOptions::default());
    assert!(g.contains("cbor_any_t extra;"), "{g}");
    assert!(g.contains("nanocbor_get_subcbor(&map, &out->extra.ptr, &out->extra.len)"), "{g}");
    assert!(g.contains("cddlc_put_item(enc, val->extra.ptr, val->extra.len)"), "{g}");
    assert!(g.contains("static inline int cddlc_put_item("), "{g}");
}

#[test]
fn an_any_alternative_takes_what_the_others_dont() {
    let g = generate(SRC, &CodegenOptions::default());
    // Text is read from a probe, so text no alternative takes is left for `any`.
    assert!(g.contains("nanocbor_value_t _t = *dec;"), "{g}");
    assert!(g.contains("if (nanocbor_get_subcbor(dec, &out->value.Any.ptr, &out->value.Any.len) < 0) return -1;"), "{g}");
    assert!(g.contains("if (cddlc_put_item(enc, val->value.Any.ptr, val->value.Any.len) < 0) return -1;"), "{g}");
}

#[test]
fn json_holds_the_value_as_its_text() {
    let g = generate(SRC, &json());
    assert!(g.contains("json_any_t extra;"), "{g}");
    assert!(g.contains("cddlc_json_any_to(&(obj->extra))"), "{g}");
    assert!(g.contains("if (!cddlc_json_any_from("), "{g}");
    assert!(g.contains("return cddlc_json_any_to(&obj->value.Any);"), "{g}");
}
