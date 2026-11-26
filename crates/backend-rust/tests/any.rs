//! `any` holds the encoded data item (dgtlrift/cddlc#178).

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;
use std::path::PathBuf;

/// The generated lib.rs.
fn lib_rs(src: &str, opts: CodegenOptions) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    RustBackend
        .generate(&lowered.module, &opts)
        .expect("generate")
        .files[1]
        .content
        .clone()
}

const SCHEMA: &str = "hdr = { alg: int, ? extra: any, * tstr => any }\nvalue = int / any";

#[test]
fn any_is_the_runtime_item_type() {
    let g = lib_rs(SCHEMA, CodegenOptions::default());
    assert!(g.contains("pub struct AnyItem(pub Vec<u8>);"), "{g}");
    assert!(g.contains("pub extra: Option<AnyItem>,"), "{g}");
    assert!(
        g.contains("std::collections::BTreeMap<String, AnyItem>"),
        "{g}"
    );
    // Decoding keeps the item's bytes; encoding writes them back.
    assert!(g.contains("d.skip()?;"), "{g}");
    assert!(g.contains("e.writer_mut().write_all(&self.0)"), "{g}");
}

#[test]
fn an_any_alternative_takes_what_the_others_dont() {
    let g = lib_rs(SCHEMA, CodegenOptions::default());
    assert!(
        g.contains("_ => Ok(Value::Any(d.decode::<AnyItem>()?)),"),
        "{g}"
    );
}

#[test]
fn no_std_holds_the_item_in_a_fixed_capacity_vector() {
    let g = lib_rs(
        SCHEMA,
        CodegenOptions {
            no_std: true,
            ..CodegenOptions::default()
        },
    );
    assert!(
        g.contains("pub struct AnyItem(pub heapless::Vec<u8, "),
        "{g}"
    );
}

#[test]
fn json_holds_any_json_value() {
    let g = lib_rs(
        SCHEMA,
        CodegenOptions {
            format: Format::Json,
            ..CodegenOptions::default()
        },
    );
    assert!(g.contains("pub extra: Option<serde_json::Value>,"), "{g}");
    assert!(!g.contains("AnyItem"), "{g}");
}

#[test]
fn no_runtime_type_without_any() {
    let g = lib_rs("r = { a: bstr }", CodegenOptions::default());
    assert!(!g.contains("AnyItem"), "{g}");
}
