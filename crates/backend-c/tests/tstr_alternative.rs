//! A choice's `tstr` alternative holds its text in C's JSON output, apart
//! from its text literals (dgtlrift/cddlc#234).

use std::path::PathBuf;

use backend_c::CBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate_json(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let opts = CodegenOptions {
        format: Format::Json,
        ..CodegenOptions::default()
    };
    let out = CBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn json_writes_and_reads_the_held_text() {
    let g = generate_json("label = \"none\" / tstr");
    assert!(g.contains("return cJSON_CreateString(\"none\");"), "{g}");
    assert!(
        g.contains("return cJSON_CreateString(obj->value.Tstr);"),
        "{g}"
    );
    assert!(!g.contains("cJSON_CreateString(\"tstr\")"), "{g}");
    assert!(
        g.contains("if (strcmp(s, \"none\") == 0) { out->tag = LABEL_NONE; return true; }"),
        "{g}"
    );
    assert!(
        g.contains("strcpy(out->value.Tstr, s); out->tag = LABEL_TSTR; return true;"),
        "{g}"
    );
}
