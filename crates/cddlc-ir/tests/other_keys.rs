//! A map's other entries with integer keys, or keys that are a choice of
//! integers and text (COSE's `* label => values`, dgtlrift/cddlc#136).

use std::path::PathBuf;

use cddlc_ir::{lower, IrError, IrModule, Primitive, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

fn lower_src(src: &str) -> Result<IrModule, IrError> {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse failed");
    lower(&parsed.value, 16, 64).map(|r| r.module)
}

/// The key type of `rule`'s other entries.
fn rest_key(m: &IrModule, rule: &str) -> TypeRef {
    match m.get(rule) {
        Some(TypeDef::Struct(s)) => s.rest.as_ref().expect("other entries").key_ty.clone(),
        other => panic!("{rule}: expected a struct, got {other:?}"),
    }
}

#[test]
fn integer_keys() {
    let m = lower_src("m = { ? 1 => int, * int => tstr }").unwrap();
    assert_eq!(rest_key(&m, "m"), TypeRef::Primitive(Primitive::Int));
}

#[test]
fn named_choice_of_integers_and_text() {
    let m = lower_src(
        "header_map = { ? 1 => int / tstr, ? 4 => bstr, * label => values }\n\
         label = int / tstr\n\
         values = any",
    )
    .unwrap();
    assert_eq!(rest_key(&m, "header_map"), TypeRef::Named("label".into()));
}

#[test]
fn parenthesised_choice_becomes_an_enum() {
    // RFC 9203's OSCORE_Input_Material.
    let m = lower_src("m = { ? 0 => bstr, * (int / tstr) => any }").unwrap();
    let TypeRef::Named(n) = rest_key(&m, "m") else { panic!("expected a named key type") };
    let Some(TypeDef::Enum(e)) = m.get(&n) else { panic!("{n}: expected an enum") };
    let tys: Vec<_> = e.variants.iter().map(|v| v.ty.clone()).collect();
    assert_eq!(tys, vec![TypeRef::Primitive(Primitive::Int), TypeRef::Primitive(Primitive::Tstr)]);
}

#[test]
fn alias_of_text_is_text() {
    let m = lower_src("m = { a: uint, * name => uint }\nname = tstr").unwrap();
    assert_eq!(rest_key(&m, "m"), TypeRef::Primitive(Primitive::Tstr));
}

#[test]
fn byte_string_keys_are_refused() {
    let e = lower_src("m = { a: uint, * bstr => uint }").unwrap_err();
    assert!(
        e.to_string().contains("a computed key other than text or integers alongside named members"),
        "{e}"
    );
}
