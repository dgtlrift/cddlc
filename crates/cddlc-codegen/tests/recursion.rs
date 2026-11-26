//! `is_indirect` and `inline_decl_order`: which members a language that holds
//! values inline (C, C++) must hold through a pointer, and the order its types
//! are declared in (dgtlrift/cddlc#156).

use std::path::PathBuf;

use cddlc_codegen::{inline_decl_order, is_indirect};
use cddlc_ir::{lower, IrModule, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

fn module(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    lower(&parsed.value, 16, 64).expect("lower").module
}

fn member_ty(m: &IrModule, rule: &str, member: &str) -> TypeRef {
    let Some(TypeDef::Struct(s)) = m.get(rule) else { panic!("{rule}: not a struct") };
    s.fields.iter().find(|f| f.name == member).expect("member").ty.clone()
}

#[test]
fn a_member_that_holds_its_own_type() {
    let m = module("tree = { value: int, ? left: tree }");
    assert!(is_indirect("tree", &member_ty(&m, "tree", "left"), &m));
    assert!(!is_indirect("tree", &member_ty(&m, "tree", "value"), &m));
}

#[test]
fn a_cycle_through_a_choice_and_a_record() {
    let m = module("expr = int / binop\nbinop = [op: tstr, lhs: expr, rhs: expr]");
    assert!(is_indirect("binop", &member_ty(&m, "binop", "lhs"), &m));
    let Some(TypeDef::Enum(e)) = m.get("expr") else { panic!() };
    let binop = e.variants.iter().find(|v| v.ty == TypeRef::Named("binop".into())).expect("variant");
    assert!(is_indirect("expr", &binop.ty, &m));
}

#[test]
fn a_cycle_through_an_array_or_a_table() {
    let m = module("node = { kids: [* node], ? attrs: { * tstr => node } }");
    assert!(is_indirect("node", &member_ty(&m, "node", "kids"), &m));
    assert!(is_indirect("node", &member_ty(&m, "node", "attrs"), &m));
}

#[test]
fn no_cycle_no_pointer() {
    let m = module("outer = { a: inner, b: [* inner] }\ninner = { x: uint }");
    assert!(!is_indirect("outer", &member_ty(&m, "outer", "a"), &m));
    assert!(!is_indirect("outer", &member_ty(&m, "outer", "b"), &m));
}

#[test]
fn declared_after_what_they_hold_by_value() {
    let m = module("outer = { a: inner, t: tree }\ninner = { x: uint }\ntree = { v: inner, ? left: tree }");
    let order: Vec<String> = inline_decl_order(&m).into_iter().map(|s| s.to_string()).collect();
    let at = |n: &str| order.iter().position(|x| x == n).expect(n);
    assert!(at("inner") < at("outer"));
    assert!(at("inner") < at("tree"));
    assert!(at("tree") < at("outer"));
    assert_eq!(order.len(), m.types.len());
}
