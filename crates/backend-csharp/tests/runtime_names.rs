//! The generated helper class is `Cddl_`, a name no rule can produce, so a rule
//! named `cddl` gets `Cddl` (dgtlrift/cddlc#149).

use std::path::PathBuf;

use backend_csharp::CSharpBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

#[test]
fn a_rule_named_cddl_doesnt_collide_with_the_helper_class() {
    for format in [Format::Cbor, Format::Json] {
        let parsed = parse_cddl("cddl = { a: uint }", PathBuf::from("t.cddl")).expect("parse");
        let lowered = lower(&parsed.value, 16, 64).expect("lower");
        let opts = CodegenOptions { format, ..CodegenOptions::default() };
        let out = CSharpBackend.generate(&lowered.module, &opts).expect("generate");
        let src: String = out.files.iter().filter(|f| f.path.ends_with(".cs")).map(|f| f.content.as_str()).collect();
        assert!(src.contains("internal static class Cddl_"), "{src}");
        assert!(src.contains("public sealed class Cddl"), "{src}");
        assert!(!src.contains("internal static class Cddl\n"), "{src}");
    }
}
