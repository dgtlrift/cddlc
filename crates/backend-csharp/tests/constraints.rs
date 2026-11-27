//! C# checks `.regexp` with .NET's Regex, its JSON output checks member
//! constraints on decode, and test values meet a `.regexp`.

use std::path::PathBuf;

use backend_csharp::CSharpBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, format: Format) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    let out = CSharpBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const SRC: &str = "r = { n: uint .le 10, s: tstr .size 3, id: tstr .regexp \"[a-z]+\" }";

#[test]
fn regexp_matches_the_whole_text() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(
            g.contains(r#"System.Text.RegularExpressions.Regex.IsMatch("#),
            "{g}"
        );
        assert!(g.contains(r#"@"\A(?:[a-z]+)\z")"#), "{g}");
    }
}

#[test]
fn json_checks_member_constraints_on_decode() {
    let g = generate(SRC, Format::Json);
    assert!(
        g.contains("var v = JsonSerializer.Deserialize<R>(data)!;"),
        "{g}"
    );
    assert!(g.contains("throw new JsonValidationException(\"n\""), "{g}");
}

#[test]
fn a_regexp_test_value_matches() {
    let g = generate(SRC, Format::Cbor);
    assert!(g.contains("Id = \"a\","), "{g}");
}
