//! Recursive types in C: members that hold their own type again are pointers,
//! decoded into memory from the caller's allocator and given back by
//! `<type>_release` (dgtlrift/cddlc#156).

use std::path::PathBuf;

use backend_c::CBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn generate(src: &str, opts: &CodegenOptions) -> Result<String, String> {
    let parsed = parse_cddl(src, PathBuf::from("rec.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CBackend.generate(&m, opts).map_err(|e| e.to_string())?;
    Ok(out.files.iter().map(|f| f.content.clone()).collect::<Vec<_>>().join("\n"))
}

const SRC: &str = "tree = { value: int, ? left: tree }\nexpr = int / binop\nbinop = [op: tstr, lhs: expr, rhs: expr]";

#[test]
fn cbor() {
    let c = generate(SRC, &CodegenOptions::default()).unwrap();
    // Declared ahead, so a pointer can come first.
    assert!(c.contains("typedef struct tree_s tree_t;"), "{c}");
    assert!(c.contains("struct tree_s {"), "{c}");
    assert!(c.contains("tree_t * left;") || c.contains("tree_t *left;"), "{c}");
    assert!(c.contains("expr_t * lhs;") || c.contains("expr_t *lhs;"), "{c}");
    // The caller's allocator, and a way to give the memory back.
    assert!(c.contains("void *(*alloc)(void *ctx, size_t size);"), "{c}");
    assert!(c.contains("void (*release)(void *ctx, void *ptr);"), "{c}");
    assert!(c.contains("REC_ERR_ALLOC"), "{c}");
    assert!(c.contains("void rec_tree_release(tree_t *val, const rec_callbacks_t *cb, void *ctx)"), "{c}");
    assert!(c.contains("cb->alloc(ctx, sizeof *out->left)"), "{c}");
}

#[test]
fn json_uses_the_heap() {
    let c = generate(SRC, &CodegenOptions { format: Format::Json, ..Default::default() }).unwrap();
    assert!(c.contains("void rec_tree_release(tree_t *val)"), "{c}");
    assert!(c.contains("out->left = calloc(1, sizeof *out->left);"), "{c}");
}

#[test]
fn a_repeated_recursive_member_is_refused() {
    let e = generate("r = [a: uint, * kid: r]", &CodegenOptions::default()).unwrap_err();
    assert!(e.contains("C can't hold a repeated member that holds 'r' again yet"), "{e}");
}

#[test]
fn types_without_recursion_are_unchanged() {
    let c = generate("p = { x: uint, q: q }\nq = { y: tstr }", &CodegenOptions::default()).unwrap();
    assert!(!c.contains("rec_p_release") && !c.contains("rec_q_release"), "{c}");
    assert!(c.contains("typedef struct {"), "{c}");
}
