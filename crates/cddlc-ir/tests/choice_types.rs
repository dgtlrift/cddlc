//! Inline arrays and maps in a type choice (dgtlrift/cddlc#138).

use std::path::PathBuf;

use cddlc_ir::{lower, IrModule, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

fn compile(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse failed");
    lower(&parsed.value, 16, 64).expect("lower failed").module
}

#[test]
fn inline_array_and_map_in_type_choice_are_types() {
    // RFC 9594's `get_creds`, RFC 9321's `Extension`.
    let m = compile("g = null / [ uint, tstr ]\ne = { + text => text } / null");
    let TypeDef::Enum(g) = m.get("g").unwrap() else { panic!("not an enum") };
    assert!(matches!(&g.variants[1].ty, TypeRef::Named(n) if n == "g-alt1"));
    assert!(matches!(m.get("g-alt1"), Some(TypeDef::Struct(_))));
    let TypeDef::Enum(e) = m.get("e").unwrap() else { panic!("not an enum") };
    assert!(matches!(&e.variants[0].ty, TypeRef::Named(n) if n == "e-alt0"));
    assert!(matches!(m.get("e-alt0"), Some(TypeDef::Map(_))));
}
