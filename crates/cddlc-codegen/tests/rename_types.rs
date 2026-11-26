//! `rename_types`: a type renamed at its definition and everywhere it's used
//! (dgtlrift/cddlc#154).

use std::collections::HashMap;
use std::path::PathBuf;

use cddlc_codegen::rename_types;
use cddlc_ir::{lower, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

#[test]
fn definitions_and_uses_follow() {
    let src =
        "file = { mode: mode, ? m2: mode / tstr, list: [* mode], stamp: #6.1(mode) }\nmode = uint";
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let renames = HashMap::from([("mode".into(), "t-mode".into())]);
    let out = rename_types(&m, &renames);
    assert!(out.get("mode").is_none());
    assert!(matches!(out.get("t-mode"), Some(TypeDef::Alias(a)) if a.name == "t-mode"));
    let Some(TypeDef::Struct(s)) = out.get("file") else {
        panic!()
    };
    assert_eq!(s.fields[0].ty, TypeRef::Named("t-mode".into()));
    let text = format!("{out:?}");
    assert!(!text.contains("Named(\"mode\")"), "{text}");
    // The order is kept: a renamed type stays where it was.
    let names: Vec<_> = out.types.keys().map(|k| k.to_string()).collect();
    let before: Vec<_> = m
        .types
        .keys()
        .map(|k| {
            if k == "mode" {
                "t-mode".to_string()
            } else {
                k.to_string()
            }
        })
        .collect();
    assert_eq!(names, before);
}

#[test]
fn the_root_follows() {
    let parsed = parse_cddl("mode = { a: uint }", PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = rename_types(&m, &HashMap::from([("mode".into(), "t-mode".into())]));
    assert_eq!(out.root, "t-mode");
}
