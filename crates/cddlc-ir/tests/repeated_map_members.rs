//! A map holds a key at most once, so a map member that may repeat is
//! optional, and one that must appear is required (dgtlrift/cddlc#184).

use std::path::PathBuf;

use cddlc_ir::{lower, IrWarningKind, LowerResult, Occurrence, TypeDef};
use cddlc_parser::parse_cddl;

fn lower_src(src: &str) -> Result<LowerResult, cddlc_ir::IrError> {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse failed");
    lower(&parsed.value, 16, 64)
}

fn occurrence(r: &LowerResult, rule: &str, member: &str) -> Occurrence {
    let Some(TypeDef::Struct(s)) = r.module.get(rule) else { panic!("{rule}: expected a struct") };
    s.fields.iter().find(|f| f.name == member).expect("member").occurrence.clone()
}

#[test]
fn zero_or_more_is_optional() {
    let r = lower_src("m = { a: uint, * b: tstr }").unwrap();
    assert!(matches!(occurrence(&r, "m", "b"), Occurrence::Optional));
    assert!(r.warnings.iter().any(|w| w.kind == IrWarningKind::RepeatedMapMember && w.message.contains("optional")));
}

#[test]
fn one_or_more_is_required() {
    let r = lower_src("m = { a: uint, + b: tstr, 1*3 c: uint, 0*2 d: uint }").unwrap();
    assert!(matches!(occurrence(&r, "m", "b"), Occurrence::Required));
    assert!(matches!(occurrence(&r, "m", "c"), Occurrence::Required));
    assert!(matches!(occurrence(&r, "m", "d"), Occurrence::Optional));
}

#[test]
fn two_or_more_is_refused() {
    let Err(e) = lower_src("m = { a: uint, 2*3 b: tstr }") else { panic!("expected an error") };
    assert!(e.to_string().contains("must appear 2 times, but a map holds a key once"), "{e}");
}

#[test]
fn records_keep_their_repeated_member() {
    let r = lower_src("r = [a: uint, * b: tstr]").unwrap();
    assert!(occurrence(&r, "r", "b").is_repeated());
    assert!(r.warnings.iter().all(|w| w.kind != IrWarningKind::RepeatedMapMember));
}
