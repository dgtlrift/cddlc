//! A record with one repeated member (`[primary, * extension, payload]`): its
//! items go inline, and the array's length says how many (dgtlrift/cddlc#134).

use std::path::PathBuf;

use cddlc_ir::{lower, IrError, IrModule, Occurrence, StructEncoding, TypeDef};
use cddlc_parser::parse_cddl;

fn lower_src(src: &str) -> Result<IrModule, IrError> {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse failed");
    lower(&parsed.value, 16, 64).map(|r| r.module)
}

#[test]
fn one_repeated_member_in_the_middle() {
    let m = lower_src("bundle = [primary: uint, * extension: block, payload: bstr]\nblock = [kind: uint, data: bstr]").unwrap();
    let Some(TypeDef::Struct(s)) = m.get("bundle") else { panic!("bundle: expected a struct") };
    assert_eq!(s.encoding, StructEncoding::Array);
    let occ: Vec<_> = s.fields.iter().map(|f| f.occurrence.is_repeated()).collect();
    assert_eq!(occ, vec![false, true, false]);
}

#[test]
fn bounds_of_a_repeated_member() {
    let m = lower_src("few = [id: uint, 1*3 tag: uint, note: tstr]").unwrap();
    let Some(TypeDef::Struct(s)) = m.get("few") else { panic!() };
    assert!(matches!(s.fields[1].occurrence, Occurrence::Bounded { min: 1, max: Some(3), .. }));
}

#[test]
fn a_repeated_member_beside_an_optional_one_is_refused() {
    // RFC 8990's message-structure: the length can't say which items are whose.
    let e = lower_src("m = [kind: uint, ? initiator: tstr, * option: uint]").unwrap_err();
    assert!(e.to_string().contains("can't repeat a member beside another repeated or optional one"), "{e}");
}

#[test]
fn two_repeated_members_are_refused() {
    let e = lower_src("m = [* a: uint, sep: tstr, * b: uint]").unwrap_err();
    assert!(e.to_string().contains("can't repeat a member beside another repeated or optional one"), "{e}");
}
