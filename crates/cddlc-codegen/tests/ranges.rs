//! Range checks: a CDDL range always includes its lower bound; `..` includes
//! the upper bound and `...` excludes it (RFC 8610 §2.2.2.1).

use cddlc_codegen::{constraint_to_c_check, constraint_to_rust_check};
use cddlc_ir::Constraint;

#[test]
fn exclusive_range_keeps_lower_bound() {
    let c = Constraint::ValueRangeUint { min: Some(1), max: Some(101), inclusive: false };
    assert_eq!(constraint_to_rust_check(&c, "v").unwrap(), "v >= 1 && v < 101");
    assert_eq!(constraint_to_c_check(&c, "v").unwrap(), "v >= 1U && v < 101U");

    let c = Constraint::ValueRangeInt { min: Some(-8), max: Some(8), inclusive: false };
    assert_eq!(constraint_to_rust_check(&c, "v").unwrap(), "v >= -8 && v < 8");
    assert_eq!(constraint_to_c_check(&c, "v").unwrap(), "v >= -8 && v < 8");

    let c = Constraint::ValueRangeF64 { min: Some(0.0), max: Some(1.0), inclusive: false };
    assert_eq!(constraint_to_rust_check(&c, "v").unwrap(), "v >= 0.0 && v < 1.0");
}

#[test]
fn inclusive_range_keeps_both_bounds() {
    let c = Constraint::ValueRangeUint { min: Some(1), max: Some(5), inclusive: true };
    assert_eq!(constraint_to_rust_check(&c, "v").unwrap(), "v >= 1 && v <= 5");
    assert_eq!(constraint_to_c_check(&c, "v").unwrap(), "v >= 1U && v <= 5U");
}
