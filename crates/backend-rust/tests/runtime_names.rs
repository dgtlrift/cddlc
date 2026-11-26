//! A rule named like one of cddlc's runtime types (`ErrorCode`, `FieldError`,
//! `BitsCheck`) doesn't collide with it (dgtlrift/cddlc#149).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn lib(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    RustBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate")
        .files[1]
        .content
        .clone()
}

#[test]
fn runtime_types_live_in_their_own_module() {
    let out = lib("r = { a: uint }");
    assert!(out.contains("pub mod cddlc_runtime {"), "{out}");
    // Re-exported at the root when no rule takes the names, as before.
    assert!(
        out.contains("pub use cddlc_runtime::{ErrorCode, FieldError};"),
        "{out}"
    );
}

#[test]
fn a_rule_takes_the_name_at_the_root() {
    // WebDriver BiDi's `ErrorCode`.
    let out = lib("ErrorCode = \"invalid argument\" / \"no such frame\"\nr = { e: ErrorCode }");
    assert!(out.contains("pub enum ErrorCode {"), "{out}");
    assert!(
        out.contains("pub use cddlc_runtime::{FieldError};"),
        "{out}"
    );
    assert!(!out.contains("pub use cddlc_runtime::{ErrorCode"), "{out}");
}

#[test]
fn bits_check_is_imported_anonymously() {
    let out = lib("flags = uint .bits names\nnames = &( a: 0, b: 1 )\nbits-check = { x: uint }");
    assert!(out.contains("pub mod cddlc_bits {"), "{out}");
    assert!(out.contains("use cddlc_bits::BitsCheck as _;"), "{out}");
    assert!(!out.contains("pub use cddlc_bits::BitsCheck;"), "{out}");
}
