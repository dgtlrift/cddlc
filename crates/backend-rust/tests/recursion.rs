//! Recursive types hold a `Box` where they would otherwise contain themselves
//! (dgtlrift/cddlc#139).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// (Cargo.toml, src/lib.rs, tests/roundtrip.rs)
fn generate(src: &str, opts: CodegenOptions) -> (String, String, String) {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = RustBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    (
        out.files[0].content.clone(),
        out.files[1].content.clone(),
        out.files[2].content.clone(),
    )
}

const SRC: &str = "
tree = { value: int, ? left: tree }
expr = int / binop
binop = [op: tstr, lhs: expr, rhs: expr]
forest = { trees: [* tree] }
";

#[test]
fn a_type_that_holds_itself_is_boxed() {
    let (_, lib, tests) = generate(SRC, CodegenOptions::default());
    assert!(lib.contains("pub left: Option<Box<Tree>>,"), "{lib}");
    assert!(lib.contains("Some(Box::new(val))"), "{lib}");
    // The record and the enum hold each other: both edges of the cycle.
    assert!(lib.contains("pub lhs: Box<Expr>,"), "{lib}");
    assert!(lib.contains("Binop(Box<Binop>),"), "{lib}");
    assert!(lib.contains("Box::new(inner)"), "{lib}");
    assert!(tests.contains("lhs: Box::new("), "{tests}");
}

#[test]
fn a_list_needs_no_box() {
    let (_, lib, _) = generate(SRC, CodegenOptions::default());
    assert!(lib.contains("pub trees: ForestTrees,"), "{lib}");
    assert!(lib.contains("pub type ForestTrees = Vec<Tree>;"), "{lib}");
}

#[test]
fn no_std_takes_box_from_alloc() {
    let (cargo, lib, _) = generate(
        SRC,
        CodegenOptions {
            no_std: true,
            ..CodegenOptions::default()
        },
    );
    assert!(cargo.contains("no-std  = [\"alloc\"]"), "{cargo}");
    assert!(lib.contains("use alloc::boxed::Box;"), "{lib}");
}

#[test]
fn a_schema_without_recursion_has_no_box() {
    let (cargo, lib, _) = generate(
        "r = { a: uint, b: [* r2] }\nr2 = { c: tstr }",
        CodegenOptions::default(),
    );
    assert!(!lib.contains("Box"), "{lib}");
    assert!(cargo.contains("no-std  = [\"heapless\"]"), "{cargo}");
}
