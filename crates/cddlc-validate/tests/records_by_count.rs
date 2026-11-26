//! A record whose optional members aren't all at the end: an array of n items
//! holds the first n - required optional members (dgtlrift/cddlc#134).

use std::path::PathBuf;

use cddlc_ir::IrModule;
use cddlc_validate::{validate, Value};

fn lower(src: &str) -> IrModule {
    let parsed = cddlc_parser::parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    cddlc_ir::lower(&parsed.value, 16, 64)
        .expect("lower")
        .module
}

fn check(m: &IrModule, ty: &str, json: &str) -> bool {
    let v: serde_json::Value = serde_json::from_str(json).expect("json");
    validate(m, Some(ty), &Value::from(v)).is_ok()
}

// COSE's Sig_structure (RFC 9052) and Countersign_structure (RFC 9338), with
// text where they have byte strings so JSON can hold them.
const SRC: &str = r#"
sig = [ context: "Signature" / "Signature1", body: tstr, ? sign: tstr, aad: tstr, payload: tstr ]
countersig = [ context: tstr, body: tstr, ? sign: tstr, aad: tstr, payload: tstr, ? other: [+ tstr] ]
"#;

#[test]
fn one_optional_member_in_the_middle() {
    let m = lower(SRC);
    assert!(check(&m, "sig", r#"["Signature1", "b", "a", "p"]"#));
    assert!(check(&m, "sig", r#"["Signature", "b", "s", "a", "p"]"#));
    assert!(!check(&m, "sig", r#"["Signature", "b", "p"]"#));
    assert!(!check(
        &m,
        "sig",
        r#"["Signature", "b", "s", "a", "p", "x"]"#
    ));
}

#[test]
fn the_first_optional_members_are_present() {
    let m = lower(SRC);
    assert!(check(&m, "countersig", r#"["c", "b", "a", "p"]"#));
    assert!(check(&m, "countersig", r#"["c", "b", "s", "a", "p"]"#));
    assert!(check(
        &m,
        "countersig",
        r#"["c", "b", "s", "a", "p", ["o"]]"#
    ));
    // Five items hold `sign`, not `other`: the fifth item is `payload`, a tstr.
    assert!(!check(&m, "countersig", r#"["c", "b", "a", "p", ["o"]]"#));
}

#[test]
fn a_repeated_member_holds_the_items_the_others_leave() {
    // RFC 9171's bundle: a repeated member in the middle, its items inline.
    let m = lower("bundle = [a: uint, * b: tstr, c: uint]\npath = [n: tstr, + s: tstr]\nfew = [i: uint, 1*3 t: uint, n: tstr]");
    assert!(check(&m, "bundle", r#"[1, 2]"#));
    assert!(check(&m, "bundle", r#"[1, "x", "y", 2]"#));
    assert!(!check(&m, "bundle", r#"[1, "x", "y"]"#));
    assert!(!check(&m, "bundle", r#"[1]"#));
    assert!(check(&m, "path", r#"["a", "b"]"#));
    assert!(!check(&m, "path", r#"["a"]"#));
    assert!(check(&m, "few", r#"[1, 2, 3, "n"]"#));
    assert!(!check(&m, "few", r#"[1, "n"]"#));
    assert!(!check(&m, "few", r#"[1, 2, 3, 4, 5, "n"]"#));
}
