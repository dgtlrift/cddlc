//! `{ ... } .and other` has the other map's members, and a choice of maps
//! tries the more specific alternative first (WebDriver BiDi, dgtlrift/cddlc#157).

use std::path::PathBuf;

use cddlc_ir::{lower, IrModule, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

fn module(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    lower(&parsed.value, 16, 64).expect("lower").module
}

#[test]
fn and_merges_the_other_maps_members() {
    // script.DateRemoteValue; the other map may come later in the schema.
    let m = module("date-remote = { ? handle: tstr, ? internalId: tstr } .and date-local\ndate-local = { type: \"date\", value: tstr }");
    let Some(TypeDef::Struct(s)) = m.get("date-remote") else {
        panic!()
    };
    let names: Vec<_> = s.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["handle", "internalId", "type", "value"]);
}

#[test]
fn a_member_both_maps_have_is_kept_once() {
    let m = module("a = { x: uint, y: tstr } .and b\nb = { y: tstr, z: bool }");
    let Some(TypeDef::Struct(s)) = m.get("a") else {
        panic!()
    };
    let names: Vec<_> = s.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["x", "y", "z"]);
}

#[test]
fn the_alternative_with_more_literals_comes_first() {
    // log.Entry: `type: text` also takes "javascript".
    let m = module(
        r#"
entry = generic / javascript / console
generic = { level: tstr, type: tstr }
javascript = { level: tstr, type: "javascript" }
console = { level: tstr, type: "console", method: tstr }
"#,
    );
    let Some(TypeDef::Enum(e)) = m.get("entry") else {
        panic!()
    };
    let order: Vec<_> = e
        .variants
        .iter()
        .map(|v| match &v.ty {
            TypeRef::Named(n) => n.to_string(),
            _ => String::new(),
        })
        .collect();
    // Literals first, in the order written; the generic one last.
    assert_eq!(order, ["javascript", "console", "generic"]);
}

#[test]
fn choices_without_literals_keep_their_order() {
    let m = module("r = shot / print\nshot = { data: tstr }\nprint = { data: tstr }");
    let Some(TypeDef::Enum(e)) = m.get("r") else {
        panic!()
    };
    let order: Vec<_> = e
        .variants
        .iter()
        .map(|v| match &v.ty {
            TypeRef::Named(n) => n.to_string(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(order, ["shot", "print"]);
}
