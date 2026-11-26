//! Rules named like a type a backend's output already has are renamed
//! `<module>-<rule>` (dgtlrift/cddlc#154, #191).

use std::path::PathBuf;

use cddlc_codegen::{c_reserved_renames, csharp_reserved_renames, rust_reserved_renames};
use cddlc_ir::{lower, IrModule};
use cddlc_parser::parse_cddl;

fn module(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("fs.cddl")).expect("parse");
    lower(&parsed.value, 16, 64).expect("lower").module
}

const SRC: &str = "file = { a: result, b: mode, c: write, d: type, e: other }\nresult = uint\nmode = uint\nwrite = uint\ntype = uint\nother = uint";

#[test]
fn each_backend_renames_its_own() {
    let m = module(SRC);
    let rust = rust_reserved_renames(&m);
    assert_eq!(rust.get("result").map(|s| s.as_str()), Some("fs-result"));
    assert_eq!(rust.get("write").map(|s| s.as_str()), Some("fs-write"));
    assert!(!rust.contains_key("mode") && !rust.contains_key("other"));

    let c = c_reserved_renames(&m);
    assert_eq!(c.get("mode").map(|s| s.as_str()), Some("fs-mode"));
    assert!(!c.contains_key("result") && !c.contains_key("other"));

    let cs = csharp_reserved_renames(&m);
    assert_eq!(cs.get("write").map(|s| s.as_str()), Some("fs-write"));
    assert_eq!(cs.get("type").map(|s| s.as_str()), Some("fs-type"));
    assert!(!cs.contains_key("result") && !cs.contains_key("other"));
}

#[test]
fn a_taken_new_name_gets_a_suffix() {
    let m = module("file = { a: result, b: fs-result }\nresult = uint\nfs-result = tstr");
    assert_eq!(
        rust_reserved_renames(&m).get("result").map(|s| s.as_str()),
        Some("fs-result-type")
    );
}
