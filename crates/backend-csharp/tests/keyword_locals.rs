//! A decoder's local named after a member that's a C# keyword gets an `@`
//! (RFC 8927's `ref`, RFC 9052's `protected`, WebDriver BiDi's `params`).

use std::path::PathBuf;

use backend_csharp::CSharpBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

#[test]
fn a_keyword_local_is_escaped() {
    let parsed =
        parse_cddl("r = { ref: tstr, params: uint }", PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = CSharpBackend
        .generate(&lowered.module, &CodegenOptions::default())
        .expect("generate");
    let g: String = out
        .files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(g.contains("string @ref = string.Empty;"), "{g}");
    assert!(g.contains("@params"), "{g}");
    assert!(!g.contains("string ref ="), "{g}");
}
