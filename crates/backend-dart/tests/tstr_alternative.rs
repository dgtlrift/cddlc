//! A choice's `tstr` alternative holds its text, and an enum is equal by kind
//! and value, so it can key a map (dgtlrift/cddlc#180).

use std::path::PathBuf;

use backend_dart::DartBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn lib(src: &str, opts: CodegenOptions) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = DartBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .filter(|f| f.path.contains("/lib/"))
        .map(|f| f.content.clone())
        .collect()
}

const SRC: &str = "values = int / tstr\npower = \"on\" / \"off\" / tstr";

#[test]
fn cbor() {
    let l = lib(SRC, CodegenOptions::default());
    assert!(
        l.contains("static Values Tstr(String value) => Values._(\"tstr\", value);"),
        "{l}"
    );
    assert!(
        l.contains("case \"tstr\": return CborString(value as String, tags: tags);"),
        "{l}"
    );
    assert!(
        l.contains("default: return Values.Tstr(v.toString());"),
        "{l}"
    );
    // Literals stay constants, matched first.
    assert!(l.contains("static const On = Power._(\"on\");"), "{l}");
    assert!(l.contains("case \"on\": return Power.On;"), "{l}");
    assert!(
        l.contains("bool operator ==(Object other) => other is Values"),
        "{l}"
    );
    assert!(
        l.contains("int get hashCode => Object.hash(kind, value);"),
        "{l}"
    );
}

#[test]
fn json() {
    let l = lib(
        SRC,
        CodegenOptions {
            format: Format::Json,
            ..Default::default()
        },
    );
    assert!(l.contains("default: return Values.Tstr(raw);"), "{l}");
    assert!(l.contains("case \"tstr\": return value;"), "{l}");
}
