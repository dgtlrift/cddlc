/// `cddlc sample` (dgtlrift/cddlc#110): every type of every reference schema has
/// an example, and the example validates, as JSON and as CBOR.

use std::path::PathBuf;
use cddlc_validate::sample::{sample, to_cbor, to_diag, to_json, SampleOptions};
use cddlc_validate::{validate_from, Source, Value};

fn module(src: &str) -> cddlc_ir::IrModule {
    let parsed = cddlc_parser::parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    cddlc_ir::lower(&parsed.value, 16, 64).expect("lower").module
}

/// Every type's sample, checked as the JSON and CBOR a reader would get.
fn check_all(name: &str, src: &str, opts: SampleOptions) {
    let m = module(src);
    for ty in m.types.keys() {
        let v = sample(&m, Some(ty), opts).unwrap_or_else(|e| panic!("{name}: {ty}: {e}"));
        let json = Value::from(to_json(&v));
        if let Err(e) = validate_from(&m, Some(ty), &json, Source::Json) {
            panic!("{name}: {ty}: JSON {}: {e:?}", to_json(&v));
        }
        // Read back as `cddlc validate --cbor` does: float widths count.
        let cbor = cddlc_validate::cbor::decode(&to_cbor(&v)).expect("cbor");
        if let Err(e) = validate_from(&m, Some(ty), &cbor, Source::Cbor) {
            panic!("{name}: {ty}: CBOR {}: {e:?}", to_diag(&v));
        }
    }
}

#[test]
fn every_reference_type_has_a_valid_sample() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../backend-buildtest/schemas");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cddl") {
            let src = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            check_all(&name, &src, SampleOptions::default());
            check_all(&name, &src, SampleOptions { minimal: true });
            n += 1;
        }
    }
    assert!(n >= 14, "{n} schemas");
}

fn diag(src: &str, ty: &str, opts: SampleOptions) -> String {
    let m = module(src);
    to_diag(&sample(&m, Some(ty), opts).unwrap())
}

#[test]
fn samples_use_literals_defaults_and_constraints() {
    let src = r#"
r = {
  kind: "reading",
  ? unit: tstr .default "celsius",
  id: uint .ge 10,
  name: tstr .size 2,
  key: bstr .size (3..8),
  level: -5..-1,
  1 => bool,
}
"#;
    assert_eq!(
        diag(src, "r", SampleOptions::default()),
        r#"{"kind": "reading", "unit": "celsius", "id": 10, "name": "te", "key": h'000000', "level": -1, 1: false}"#,
    );
    // Minimal: no optional members.
    assert!(!diag(src, "r", SampleOptions { minimal: true }).contains("unit"));
}

#[test]
fn recursive_types_end() {
    let src = "tree = { value: uint, ? left: tree, ? right: tree, children: [* tree] }\n";
    let d = diag(src, "tree", SampleOptions::default());
    assert!(d.starts_with(r#"{"value": 0, "left": {"value": 0"#), "{d}");
    // A choice takes the first alternative, then once deep the one that ends.
    let d = diag("list = pair / null\npair = [uint, list]\n", "list", SampleOptions::default());
    assert_eq!(d, "[0, [0, null]]");
}

#[test]
fn tags_and_embedded_cbor() {
    let d = diag("r = { t: #6.1(uint), e: bstr .cbor inner }\ninner = [uint, tstr]\n", "r", SampleOptions::default());
    // 82 00 64 "text": [0, "text"].
    assert_eq!(d, r#"{"t": 1(0), "e": h'82006474657874'}"#);
}

#[test]
fn a_type_that_cannot_end_is_an_error() {
    let m = module("r = [uint, r]\n");
    let err = sample(&m, Some("r"), SampleOptions::default()).unwrap_err();
    assert!(err.contains("doesn't end"), "{err}");
}

#[test]
fn text_encodings_hold_an_encoded_sample() {
    // RFC 9741 (dgtlrift/cddlc#109): the sample is the operand's sample, encoded.
    let src = "r = { k: tstr .b64u (bstr .size 4), h: tstr .hexuc (bstr .size 2), b: tstr .b32 (bstr .size 3), \
               z: tstr .b45 (bstr .size 3), n: tstr .base10 (5..9), j: tstr .json claims }\nclaims = { iss: tstr }\n";
    let m = module(src);
    let v = sample(&m, Some("r"), SampleOptions::default()).unwrap();
    assert_eq!(
        to_diag(&v),
        r#"{"k": "AAAAAA", "h": "0000", "b": "AAAAA", "z": "00000", "n": "5", "j": "{\"iss\":\"text\"}"}"#
    );
    assert!(validate_from(&m, Some("r"), &v, Source::Json).is_ok());
}

#[test]
fn floats_are_written_at_their_width() {
    let m = module("r = [float16, float32, float64, float]\n");
    let bytes = to_cbor(&sample(&m, Some("r"), SampleOptions::default()).unwrap());
    assert_eq!(bytes, [0x84, 0xf9, 0, 0, 0xfa, 0, 0, 0, 0, 0xfb, 0, 0, 0, 0, 0, 0, 0, 0, 0xfb, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(cddlc_validate::cbor::decode(&[0xf9, 0x3c, 0x00]).map(|v| to_cbor(&v)), Ok(vec![0xf9, 0x3c, 0x00]));
}

#[test]
fn repeated_group_members_sit_inline() {
    check_all("groups", "r = [ + ( a: uint, b: tstr ) ]", SampleOptions::default());
    assert_eq!(diag("r = [ + ( a: uint, b: tstr ) ]", "r", SampleOptions::default()), r#"[0, "text"]"#);
}
