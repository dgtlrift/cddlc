/// Integration tests for backend-dart.
use backend_dart::DartBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;
use std::path::PathBuf;

/// All generated files, concatenated.
fn compile(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let out = DartBackend.generate(&lowered.module, &CodegenOptions::default()).expect("generate");
    out.files.iter().map(|f| f.content.as_str()).collect::<Vec<_>>().join("\n")
}

/// Issue #36: a text enum is written and read as its CDDL literal, not a string
/// rebuilt from the PascalCase variant name (`FreeSpace` -> "freespace").
#[test]
fn test_text_enum_uses_cddl_literal() {
    let out = compile("model = \"free-space\" / \"Log_Distance\"");
    assert!(out.contains("Model._(\"free-space\")"), "{out}");
    assert!(out.contains("case \"free-space\": return Model.FreeSpace;"), "{out}");
    assert!(out.contains("\"Log_Distance\""), "{out}");
    assert!(!out.contains("\"freespace\""), "{out}");
}
