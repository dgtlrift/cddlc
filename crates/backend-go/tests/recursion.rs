//! Recursive types in Go (dgtlrift/cddlc#221).

use std::path::PathBuf;

use backend_go::GoBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// The generated package source.
fn generate(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = GoBackend.generate(&lowered.module, &CodegenOptions::default()).expect("generate");
    out.files[1].content.clone()
}

/// Whether a line of `g` is the field `name ty` (gofmt pads between them).
fn has_field(g: &str, name: &str, ty: &str) -> bool {
    g.lines().any(|l| l.split_whitespace().eq([name, ty]))
}

#[test]
fn a_member_and_variant_that_hold_their_type_by_value_are_pointers() {
    let g = generate("expr = int / binop\nbinop = [op: tstr, lhs: expr, rhs: expr]");
    assert!(has_field(&g, "Lhs", "*Expr"), "{g}");
    assert!(has_field(&g, "Binop", "*Binop"), "{g}");
    assert!(g.contains("func NewExprBinop(x Binop) Expr { return Expr{Kind: ExprKindBinop, Binop: &x} }"), "{g}");
    assert!(g.contains("if v.Lhs == nil {"), "{g}");
}

#[test]
fn an_optional_member_stays_a_single_pointer() {
    let g = generate("tree = { value: int, ? left: tree }");
    assert!(has_field(&g, "Left", "*Tree"), "{g}");
    assert!(!g.contains("**Tree"), "{g}");
}

#[test]
fn a_slice_ends_the_cycle() {
    let g = generate("node = { name: tstr, kids: [* node] }");
    assert!(!g.contains("Kids *"), "{g}");
    assert!(!g.contains("if v.Kids == nil"), "{g}");
}
