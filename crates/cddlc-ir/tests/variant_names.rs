//! Variant names made from text literals (dgtlrift/cddlc#141).

use std::path::PathBuf;

use cddlc_ir::{lower, IrModule, TypeDef};
use cddlc_parser::parse_cddl;

fn compile(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse failed");
    lower(&parsed.value, 16, 64).expect("lower failed").module
}

#[test]
fn literal_variant_names_are_identifiers_and_unique() {
    // WebDriver BiDi's `script.SpecialNumber`.
    let m = compile(r#"n = "NaN" / "-0" / "Infinity" / "-Infinity"
t = "text/plain" / "1st" / "" / "a-b" / "a_b""#);
    let names = |r: &str| match m.get(r).unwrap() {
        TypeDef::Enum(e) => e.variants.iter().map(|v| v.name.to_string()).collect::<Vec<_>>(),
        _ => panic!("not an enum"),
    };
    assert_eq!(names("n"), ["NaN", "Minus0", "Infinity", "MinusInfinity"]);
    assert_eq!(names("t"), ["TextPlain", "V1st", "Empty", "AB", "AB4"]);
}
