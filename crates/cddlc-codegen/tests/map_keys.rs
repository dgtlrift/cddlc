//! Map keys: JSON text and dCBOR order (dgtlrift/cddlc#176).

use std::path::PathBuf;

use cddlc_codegen::{dcbor_key_order, field_key_json, has_int_keys};
use cddlc_ir::{lower, TypeDef};
use cddlc_parser::parse_cddl;

fn fields(src: &str) -> cddlc_ir::StructDef {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    match lower(&parsed.value, 16, 64).expect("lower").module.get("r") {
        Some(TypeDef::Struct(s)) => s.clone(),
        _ => panic!("not a struct"),
    }
}

#[test]
fn json_keys_and_names() {
    let s = fields("r = { 1 => tstr, -1 => int, name: tstr }");
    let keys: Vec<String> = s.fields.iter().map(|f| field_key_json(&f.key)).collect();
    assert_eq!(keys, ["1", "-1", "name"]);
    let names: Vec<&str> = s.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["field_1", "field_minus1", "name"]);
    assert!(has_int_keys(&s));
    assert!(!has_int_keys(&fields("r = { a: uint }")));
}

#[test]
fn dcbor_orders_by_encoded_key() {
    let s = fields("r = { aa: uint, b: uint, -1 => uint, 24 => uint, 1 => uint }");
    let mut sorted: Vec<_> = s.fields.iter().collect();
    sorted.sort_by(|a, b| dcbor_key_order(a, b));
    let keys: Vec<String> = sorted.iter().map(|f| field_key_json(&f.key)).collect();
    // 0x01, 0x18 0x18, 0x20, 0x61 "b", 0x62 "aa"
    assert_eq!(keys, ["1", "24", "-1", "b", "aa"]);
}
