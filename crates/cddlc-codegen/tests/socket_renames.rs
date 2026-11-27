//! A socket beside a rule of its name (`$eid` and `eid`, RFC 9171) is
//! renamed `<name>-socket`, as both would have one type name.

use std::path::PathBuf;

use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

#[test]
fn a_socket_beside_its_rule_is_renamed() {
    let src = "eid = $eid .within eid-structure\neid-structure = [uint, any]\n$eid /= [1, tstr]\n$eid /= [2, uint]";
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let r = cddlc_codegen::cpp_reserved_renames(&m);
    assert_eq!(
        r.get("$eid").map(|s| s.as_str()),
        Some("eid-socket"),
        "{r:?}"
    );
    assert!(!r.contains_key("eid"), "{r:?}");
}
