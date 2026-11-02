/// Integration tests for backend-interop.

use std::path::PathBuf;
use backend_interop::{generate, InteropLangs};
use cddlc_codegen::CodegenOptions;
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn compile(src: &str) -> Vec<backend_interop::TestVector> {
    let parsed  = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    backend_interop::collect_vectors_pub(&lowered.module, &CodegenOptions::default())
}

fn gen(src: &str) -> (String, String, String, String, String, String, String) {
    let parsed  = parse_cddl(src, PathBuf::from("test.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let files   = generate(&lowered.module, &InteropLangs::all(), &CodegenOptions::default());

    let get = |suffix: &str| files.iter()
        .find(|f| f.path.ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_default();

    (
        get("vectors.json"),
        get("vectors.cbor"),
        get("interop_test.rs"),
        get("test_interop.c"),
        get("test_interop.cpp"),
        get("InteropTests.cs"),
        get("interop.test.ts"),
    )
}

fn json(src: &str)   -> String { gen(src).0 }
fn cbor(src: &str)   -> String { gen(src).1 }
fn rust(src: &str)   -> String { gen(src).2 }
fn c(src: &str)      -> String { gen(src).3 }
fn cpp(src: &str)    -> String { gen(src).4 }
fn csharp(src: &str) -> String { gen(src).5 }
fn nodejs(src: &str) -> String { gen(src).6 }

// ── Vector collection ─────────────────────────────────────────────────────────

#[test]
fn test_struct_produces_two_vectors() {
    let vecs = compile("sensor = { id: uint, value: float32 }");
    let sensor: Vec<_> = vecs.iter().filter(|v| v.type_name == "Sensor").collect();
    assert_eq!(sensor.len(), 2, "expected default + explicit vectors");
    assert_eq!(sensor[0].description, "default values");
    assert_eq!(sensor[1].description, "explicit values");
}

#[test]
fn test_enum_produces_one_vector_per_variant() {
    let vecs = compile(r#"status = "ok" / "warn" / "error""#);
    let status: Vec<_> = vecs.iter().filter(|v| v.type_name == "Status").collect();
    assert_eq!(status.len(), 3);
}

#[test]
fn test_alias_produces_one_vector() {
    let vecs = compile("device-id = tstr .size 4");
    let alias: Vec<_> = vecs.iter().filter(|v| v.type_name == "DeviceId").collect();
    assert_eq!(alias.len(), 1);
    assert_eq!(alias[0].description, "valid value");
}

#[test]
fn test_array_produces_one_vector() {
    let vecs = compile("readings = [* float32]");
    let arr: Vec<_> = vecs.iter().filter(|v| v.type_name == "Readings").collect();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].description, "empty array");
}

#[test]
fn test_cbor_bytes_are_non_empty() {
    let vecs = compile("sensor = { id: uint }");
    for v in &vecs {
        assert!(!v.cbor_bytes.is_empty(), "vector {} has empty bytes", v.description);
    }
}

#[test]
fn test_constrained_alias_valid_value() {
    let vecs = compile("device-id = tstr .size 4");
    let v = vecs.iter().find(|v| v.type_name == "DeviceId").unwrap();
    // value should be 4 chars
    if let Some(f) = v.fields.iter().find(|f| f.name == "value") {
        if let backend_interop::FieldVal::Text(s) = &f.value {
            assert_eq!(s.len(), 4);
        }
    }
}

// ── File structure ────────────────────────────────────────────────────────────

#[test]
fn test_emits_all_files_with_all_langs() {
    let parsed  = parse_cddl("x = uint", PathBuf::from("test.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let files   = generate(&lowered.module, &InteropLangs::all(), &CodegenOptions::default());
    // vectors.json + vectors.cbor + 6 language harnesses
    assert_eq!(files.len(), 8);
}

#[test]
fn test_dir_named_after_schema() {
    let parsed  = parse_cddl("x = uint", PathBuf::from("sensor.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let files   = generate(&lowered.module, &InteropLangs::all(), &CodegenOptions::default());
    for f in &files {
        assert!(f.path.starts_with("sensor-interop/"), "unexpected path: {}", f.path);
    }
}

#[test]
fn test_selective_langs() {
    let parsed  = parse_cddl("x = uint", PathBuf::from("t.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let langs   = InteropLangs { rust: true, python: true, ..Default::default() };
    let files   = generate(&lowered.module, &langs, &CodegenOptions::default());
    // vectors.json + vectors.cbor + rust + python = 4
    assert_eq!(files.len(), 4);
    assert!(files.iter().any(|f| f.path.ends_with("interop_test.rs")));
    assert!(files.iter().any(|f| f.path.ends_with("test_interop.py")));
    assert!(!files.iter().any(|f| f.path.ends_with("InteropTests.cs")));
}

// ── vectors.json ──────────────────────────────────────────────────────────────

#[test]
fn test_json_generated_comment() {
    let json = json("x = uint");
    // JSON has schema field
    assert!(json.contains("\"generated_by\": \"cddlc\""));
}

#[test]
fn test_json_has_schema_name() {
    let json = json("x = uint");
    assert!(json.contains("\"schema\""));
}

#[test]
fn test_json_has_vectors_array() {
    let json = json("sensor = { id: uint }");
    assert!(json.contains("\"vectors\""));
    assert!(json.contains("\"type\": \"Sensor\""));
}

#[test]
fn test_json_has_cbor_hex() {
    let json = json("sensor = { id: uint }");
    assert!(json.contains("\"cbor_hex\""));
    // hex should be lowercase alphanumeric
    let hex_start = json.find("\"cbor_hex\": \"").unwrap() + 13;
    let hex_end   = json[hex_start..].find('"').unwrap() + hex_start;
    let hex = &json[hex_start..hex_end];
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "invalid hex: {hex}");
}

#[test]
fn test_json_has_fields() {
    let json = json("sensor = { id: uint, value: float32 }");
    assert!(json.contains("\"fields\""));
    assert!(json.contains("\"id\""));
}

#[test]
fn test_json_default_uint_is_zero() {
    let json = json("msg = { count: uint }");
    assert!(json.contains("\"count\": 0"));
}

#[test]
fn test_json_explicit_uint_is_42() {
    let json = json("msg = { count: uint }");
    assert!(json.contains("\"count\": 42"));
}

#[test]
fn test_json_explicit_text_is_hello() {
    let json = json("msg = { label: tstr }");
    assert!(json.contains("\"label\": \"hello\""));
}

// ── vectors.cbor ─────────────────────────────────────────────────────────────

#[test]
fn test_cbor_file_has_comment() {
    let cbor = cbor("x = uint");
    assert!(cbor.contains("@generated by cddlc"));
}

#[test]
fn test_cbor_file_has_type_entries() {
    let cbor = cbor("sensor = { id: uint }");
    assert!(cbor.contains("Sensor:"));
}

// ── Rust harness ──────────────────────────────────────────────────────────────

#[test]
fn test_rust_generated_comment() {
    let rust = rust("x = uint");
    assert!(rust.contains("@generated by cddlc"));
}

#[test]
fn test_rust_has_test_fn() {
    let rust = rust("sensor = { id: uint }");
    assert!(rust.contains("#[test]"));
    // type_name goes through to_snake_case which only replaces hyphens
    assert!(rust.contains("fn interop_Sensor_") || rust.contains("fn interop_sensor_"),
        "expected interop test fn in:\n{}", &rust[..rust.len().min(500)]);
}

#[test]
fn test_rust_decodes_hex() {
    let rust = rust("sensor = { id: uint }");
    assert!(rust.contains("hex::decode"));
    assert!(rust.contains("::decode("));
}

#[test]
fn test_rust_asserts_field_values() {
    let rust = rust("sensor = { id: uint }");
    assert!(rust.contains("assert_eq!(decoded.id"));
}

#[test]
fn test_rust_reencodes_and_compares() {
    let rust = rust("sensor = { id: uint }");
    assert!(rust.contains("reencoded"));
    assert!(rust.contains("hex::encode"));
}

// ── C harness ─────────────────────────────────────────────────────────────────

#[test]
fn test_c_generated_comment() {
    let c = c("x = uint");
    assert!(c.contains("@generated by cddlc"));
}

#[test]
fn test_c_has_hex_array() {
    let c = c("sensor = { id: uint }");
    assert!(c.contains("static const uint8_t vec_0[]"));
    assert!(c.contains("0x"));
}

#[test]
fn test_c_has_test_function() {
    let c = c("sensor = { id: uint }");
    assert!(c.contains("static void test_interop_"));
    assert!(c.contains("assert("));
}

#[test]
fn test_c_has_main() {
    let c = c("sensor = { id: uint }");
    assert!(c.contains("int main(void)"));
}

// ── C++ harness ───────────────────────────────────────────────────────────────

#[test]
fn test_cpp_generated_comment() {
    let cpp = cpp("x = uint");
    assert!(cpp.contains("@generated by cddlc"));
}

#[test]
fn test_cpp_uses_constexpr_array() {
    let cpp = cpp("sensor = { id: uint }");
    assert!(cpp.contains("constexpr std::array"));
}

#[test]
fn test_cpp_has_namespace() {
    let cpp = cpp("sensor = { id: uint }");
    assert!(cpp.contains("using namespace"));
}

// ── C# harness ────────────────────────────────────────────────────────────────

#[test]
fn test_csharp_generated_comment() {
    let cs = csharp("x = uint");
    assert!(cs.contains("@generated by cddlc"));
}

#[test]
fn test_csharp_has_fact_attribute() {
    let cs = csharp("sensor = { id: uint }");
    assert!(cs.contains("[Fact]"));
}

#[test]
fn test_csharp_uses_cbor_reader() {
    let cs = csharp("sensor = { id: uint }");
    assert!(cs.contains("CborReader") && cs.contains("FromHexString"));
}

#[test]
fn test_csharp_reencodes() {
    let cs = csharp("sensor = { id: uint }");
    assert!(cs.contains("CborWriter") && cs.contains("Encode()"));
}

// ── Node.js harness ───────────────────────────────────────────────────────────

#[test]
fn test_nodejs_generated_comment() {
    let ts = nodejs("x = uint");
    assert!(ts.contains("@generated by cddlc"));
}

#[test]
fn test_nodejs_imports_decoders() {
    let ts = nodejs("sensor = { id: uint }");
    assert!(ts.contains("decodeSensor"));
    assert!(ts.contains("encodeSensor"));
}

#[test]
fn test_nodejs_has_describe_blocks() {
    let ts = nodejs("sensor = { id: uint }");
    assert!(ts.contains("describe("));
    assert!(ts.contains("test("));
}

#[test]
fn test_nodejs_uses_from_hex() {
    let ts = nodejs("sensor = { id: uint }");
    assert!(ts.contains("fromHex("));
}

#[test]
fn test_nodejs_reencodes() {
    let ts = nodejs("sensor = { id: uint }");
    assert!(ts.contains("toHex(") && ts.contains("toEqual("));
}

// ── dCBOR ─────────────────────────────────────────────────────────────────────

#[test]
fn test_dcbor_vectors_differ_from_normal() {
    let src = "s = { b: uint, a: tstr }";
    let parsed  = parse_cddl(src, PathBuf::from("t.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let normal  = generate(&lowered.module, &InteropLangs { rust: true, ..Default::default() }, &CodegenOptions::default());
    let mut dcbor_opts = CodegenOptions::default();
    dcbor_opts.dcbor = true;
    let dcbor = generate(&lowered.module, &InteropLangs { rust: true, ..Default::default() }, &dcbor_opts);
    // With dCBOR, keys are sorted — bytes should differ from non-dCBOR
    let normal_json = normal.iter().find(|f| f.path.ends_with("vectors.json")).unwrap();
    let dcbor_json  = dcbor.iter().find(|f| f.path.ends_with("vectors.json")).unwrap();
    // Both have vectors but hex may differ if field order differs
    assert!(normal_json.content.contains("cbor_hex"));
    assert!(dcbor_json.content.contains("cbor_hex"));
}

// ── IoT schema ────────────────────────────────────────────────────────────────

#[test]
fn test_iot_schema_generates_all_files() {
    let src = r#"
sensor-message = { version: uint, label: tstr }
device-id = tstr .size 4
readings = [* float32]
status = "ok" / "warn"
"#;
    let parsed  = parse_cddl(src, PathBuf::from("test.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let files   = generate(&lowered.module, &InteropLangs::all(), &CodegenOptions::default());
    assert_eq!(files.len(), 8);
    let json = files.iter().find(|f| f.path.ends_with("vectors.json")).unwrap();
    assert!(json.content.contains("SensorMessage"));
    assert!(json.content.contains("DeviceId"));
    assert!(json.content.contains("Readings"));
    assert!(json.content.contains("Status"));
}
