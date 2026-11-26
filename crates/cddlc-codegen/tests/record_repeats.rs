//! `record_repeated`, `repeat_bounds` and `default_is_encodable`
//! (dgtlrift/cddlc#134).

use std::path::PathBuf;

use cddlc_codegen::{default_is_encodable, record_repeated, repeat_bounds};
use cddlc_ir::{lower, StructDef, TypeDef};
use cddlc_parser::parse_cddl;

fn record(src: &str, rule: &str) -> StructDef {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    match m.get(rule) {
        Some(TypeDef::Struct(s)) => s.clone(),
        _ => panic!("not a struct"),
    }
}

#[test]
fn the_repeated_member() {
    assert_eq!(
        record_repeated(&record("r = [a: uint, * b: uint, c: tstr]", "r")),
        Some(1)
    );
    assert_eq!(
        record_repeated(&record("r = [a: uint, + b: tstr]", "r")),
        Some(1)
    );
    assert_eq!(
        record_repeated(&record("r = [a: uint, b: tstr]", "r")),
        None
    );
    // A map's repeated member isn't a record's.
    assert_eq!(
        record_repeated(&record("r = { a: uint, b: tstr }", "r")),
        None
    );
}

#[test]
fn bounds() {
    for (src, want) in [
        ("r = [x: tstr, * a: uint]", (0, None)),
        ("r = [x: tstr, + a: uint]", (1, None)),
        ("r = [x: tstr, 2*4 a: uint]", (2, Some(4))),
        ("r = [x: tstr, 3* a: uint]", (3, None)),
    ] {
        let s = record(src, "r");
        assert_eq!(repeat_bounds(&s.fields[1].occurrence), want, "{src}");
    }
}

#[test]
fn a_default_with_no_items_encodes_only_without_a_minimum() {
    assert!(default_is_encodable(&record(
        "r = [x: tstr, * a: uint]",
        "r"
    )));
    assert!(!default_is_encodable(&record(
        "r = [x: tstr, + a: uint]",
        "r"
    )));
    assert!(default_is_encodable(&record("r = [x: tstr, a: uint]", "r")));
}
