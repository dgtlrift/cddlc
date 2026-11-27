//! A choice's alternative that's a generic's instance is a type of its own
//! (RFC 9171's `$extension-block /= extension-block-use<6, ...>`).

use std::path::PathBuf;

use cddlc_ir::{lower, TypeDef, TypeRef};
use cddlc_parser::parse_cddl;

#[test]
fn a_generic_alternative_is_instantiated() {
    let src = "block = use<1, uint> / use<2, tstr>\nuse<C, D> = [code: C, data: D]";
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let Some(TypeDef::Enum(e)) = m.get("block") else {
        panic!("{:?}", m.get("block"))
    };
    for v in &e.variants {
        let TypeRef::Named(n) = &v.ty else {
            panic!("{v:?}")
        };
        assert_ne!(n.as_str(), "use", "{v:?}");
        assert!(matches!(m.get(n), Some(TypeDef::Struct(_))), "{n}");
    }
}
