//! A member whose type is an inline choice holds any alternative, in an enum
//! of its own (dgtlrift/cddlc#146).

use std::path::PathBuf;

use backend_rust::RustBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn lib_rs(src: &str, opts: CodegenOptions) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    RustBackend.generate(&lowered.module, &opts).expect("generate").files[1].content.clone()
}

#[test]
fn a_choice_member_gets_an_enum() {
    // RFC 9360's COSE_CertHash.
    let src = "cert = [hashAlg: (int / tstr), hashValue: bstr]";
    for opts in [CodegenOptions::default(), CodegenOptions { format: cddlc_codegen::Format::Json, ..Default::default() }] {
        let lib = lib_rs(src, opts);
        assert!(lib.contains("pub enum CertHashAlg"), "{lib}");
        assert!(lib.contains("Tstr(String)"), "{lib}");
        assert!(lib.contains("pub hashAlg: CertHashAlg,"), "{lib}");
    }
}

#[test]
fn tagged_optional_and_other_entries() {
    let lib = lib_rs("r = { stamp: #6.1(int / float), ? flag: bool / uint, * tstr => int / tstr }", CodegenOptions::default());
    assert!(lib.contains("pub stamp: RStamp,"), "{lib}");
    assert!(lib.contains("pub flag: Option<RFlag>,"), "{lib}");
    assert!(lib.contains("pub enum RExtraValue"), "{lib}");
}

#[test]
fn a_name_already_taken_gets_another() {
    let lib = lib_rs("r = { v: int / tstr }\nr-v = uint", CodegenOptions::default());
    assert!(lib.contains("pub v: RVChoice,"), "{lib}");
}
