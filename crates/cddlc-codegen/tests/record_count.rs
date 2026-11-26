//! `record_optionals_before` (dgtlrift/cddlc#134).

use std::path::PathBuf;

use cddlc_codegen::record_optionals_before;
use cddlc_ir::{lower, TypeDef};
use cddlc_parser::parse_cddl;

fn before(src: &str, rule: &str) -> Option<Vec<usize>> {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    match m.get(rule) {
        Some(TypeDef::Struct(s)) => record_optionals_before(s),
        _ => panic!("not a struct"),
    }
}

#[test]
fn optional_members_before_each_member() {
    assert_eq!(
        before("r = [a: uint, ? b: uint, c: uint, d: uint, ? e: uint]", "r"),
        Some(vec![0, 0, 1, 1, 1])
    );
    // Optional members only at the end: read in order until the array ends.
    assert_eq!(before("r = [a: uint, b: uint, ? c: uint]", "r"), None);
    assert_eq!(before("r = [a: uint, b: uint]", "r"), None);
    // A map isn't a record.
    assert_eq!(before("r = { ? a: uint, b: uint }", "r"), None);
}
