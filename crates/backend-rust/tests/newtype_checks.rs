//! A member's constraints on a newtype alias are checked on its inner value
//! (dgtlrift/cddlc#150).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

#[test]
fn constraints_on_a_newtype_member_check_its_value() {
    // WebDriver BiDi's `? maxNodeCount: (js-uint .ge 1)`.
    let src = "js-uint = 0..9007199254740991\nname = tstr .size (1..10)\nlabel = name\n\
               opts = { ? max: (js-uint .ge 1), ? short: (label .size (1..3)), ? plain: (uint .le 5) }";
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let lib = RustBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate")
        .files[1]
        .content
        .clone();
    assert!(lib.contains("if !(val.0 >= 1)"), "{lib}");
    // Through a plain alias (`label = name`) to the newtype.
    assert!(
        lib.contains("if !(val.0.len() >= 1 && val.0.len() <= 3)"),
        "{lib}"
    );
    assert!(lib.contains("if !(val <= 5)"), "{lib}");
}
