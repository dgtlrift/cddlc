//! Recursive types in C++: a member or alternative that holds its own type
//! again is a `Box<T>` (dgtlrift/cddlc#156).

use std::path::PathBuf;

use backend_cpp::CppBackend;
use cddlc_codegen::{AllocStrategy, Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn generate(src: &str, opts: &CodegenOptions) -> Result<String, String> {
    let parsed = parse_cddl(src, PathBuf::from("rec.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CppBackend.generate(&m, opts).map_err(|e| e.to_string())?;
    Ok(out
        .files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n"))
}

const SRC: &str = "tree = { value: int, ? left: tree }\nexpr = int / binop\nbinop = [op: tstr, lhs: expr, rhs: expr]";

#[test]
fn members_and_alternatives_are_boxes() {
    let c = generate(SRC, &CodegenOptions::default()).unwrap();
    assert!(c.contains("class Box {"), "{c}");
    assert!(c.contains("Box<Tree> left{};"), "{c}");
    assert!(c.contains("Box<Expr> lhs{};"), "{c}");
    assert!(
        c.contains("std::variant<std::monostate, int64_t, Box<Binop>>"),
        "{c}"
    );
}

#[test]
fn without_heap_allocation_memory_comes_from_the_callbacks() {
    let c = generate(SRC, &CodegenOptions::default()).unwrap();
    assert!(
        c.contains("void *(*alloc)(void *ctx, std::size_t size){nullptr};"),
        "{c}"
    );
    assert!(c.contains("AllocFailed,"), "{c}");
    assert!(
        c.contains("out = Box<T>::in(cb.alloc, cb.release, ctx);"),
        "{c}"
    );
}

#[test]
fn with_heap_allocation_boxes_use_the_heap() {
    let c = generate(
        SRC,
        &CodegenOptions {
            alloc: AllocStrategy::Heap,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(c.contains("out = Box<T>::heap();"), "{c}");
}

#[test]
fn json_boxes_an_array_through_the_type_it_stands_for() {
    let c = generate(
        "node = { name: tstr, kids: [* node] }",
        &CodegenOptions {
            format: Format::Json,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(c.contains("Box<std::vector<Node>> kids{};"), "{c}");
}

#[test]
fn a_repeated_recursive_member_is_refused() {
    let e = generate("r = [a: uint, * kid: r]", &CodegenOptions::default()).unwrap_err();
    assert!(
        e.contains("C++ can't hold a repeated member that holds 'r' again yet"),
        "{e}"
    );
}

#[test]
fn types_without_recursion_have_no_box() {
    let c = generate(
        "p = { x: uint, q: q }\nq = { y: tstr }",
        &CodegenOptions::default(),
    )
    .unwrap();
    assert!(!c.contains("class Box"), "{c}");
}

#[test]
fn a_recursive_choice_with_null_is_a_box_that_may_be_empty() {
    // WebDriver BiDi's browsingContext.Info.children.
    let c = generate(
        "info = { id: tstr, children: info-list / null }\ninfo-list = [* info]",
        &CodegenOptions::default(),
    )
    .unwrap();
    assert!(c.contains("Box<InfoList> children{};"), "{c}");
    assert!(c.contains("if (val.children) { if (encode(enc, val.children) < 0) return -1; } else { nanocbor_fmt_null(enc); }"), "{c}");
    assert!(c.contains("if (nanocbor_get_null(&map) < 0) { if (decode(&map, out.children, cb, ctx) < 0) return -1; }"), "{c}");
}
