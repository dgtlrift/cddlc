//! C++ checks `.regexp` with std::regex, and its JSON output checks every
//! member's constraints on decode; test values meet them.

use std::path::PathBuf;

use backend_cpp::CppBackend;
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

/// All generated files, concatenated.
fn generate(src: &str, format: Format) -> String {
    let parsed = parse_cddl(src, PathBuf::from("t.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let opts = CodegenOptions {
        format,
        ..CodegenOptions::default()
    };
    let out = CppBackend.generate(&m, &opts).expect("generate");
    out.files
        .iter()
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

const SRC: &str = "r = { n: uint .le 10, s: tstr .size 3, id: tstr .regexp \"[a-z]+\" }";

#[test]
fn regexp_is_checked_with_std_regex() {
    for format in [Format::Cbor, Format::Json] {
        let g = generate(SRC, format);
        assert!(g.contains("#include <regex>"), "{g}");
        assert!(g.contains("static const std::regex re(R\"re([a-z]+)re\"); return std::regex_match(v.begin(), v.end(), re);"), "{g}");
    }
}

#[test]
fn json_checks_member_constraints_on_decode() {
    let g = generate(SRC, Format::Json);
    assert!(g.contains("if (!(out.n <= 10)) throw std::invalid_argument(\"R.n: value breaks its constraint\");"), "{g}");
    assert!(g.contains("if (!(out.s.size() == 3)) throw"), "{g}");
}

#[test]
fn test_values_meet_the_constraints() {
    let cbor = generate(SRC, Format::Cbor);
    assert!(cbor.contains("original.n = 10ULL;"), "{cbor}");
    assert!(
        cbor.contains("static const char _s_str[] = \"aaa\";"),
        "{cbor}"
    );
    assert!(
        cbor.contains("static const char _id_str[] = \"a\";"),
        "{cbor}"
    );
    let json = generate(SRC, Format::Json);
    assert!(json.contains("original.id = std::string(\"a\");"), "{json}");
}

#[test]
fn no_regex_header_without_regexp() {
    let g = generate("r = { s: tstr .size 3 }", Format::Cbor);
    assert!(!g.contains("<regex>"), "{g}");
}
