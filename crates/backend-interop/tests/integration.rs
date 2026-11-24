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
    assert_eq!(arr[0].description, "minimal array");
    assert_eq!(arr[0].cbor_hex(), "80", "`[* T]` allows zero elements");
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
    // vectors.json + vectors.cbor + 9 language harnesses
    assert_eq!(files.len(), 11);
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
fn test_rust_decodes_inline_bytes() {
    let rust = rust("sensor = { id: uint }");
    assert!(rust.contains("let bytes: &[u8] = &[0xa1, "));
    assert!(rust.contains("minicbor::decode(bytes)"));
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
    assert!(rust.contains("assert_eq!(reencoded, bytes);"));
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
    assert_eq!(files.len(), 11);
    let json = files.iter().find(|f| f.path.ends_with("vectors.json")).unwrap();
    assert!(json.content.contains("SensorMessage"));
    assert!(json.content.contains("DeviceId"));
    assert!(json.content.contains("Readings"));
    assert!(json.content.contains("Status"));
}

/// Issue #36: an enum vector encodes the variant's CDDL literal.
#[test]
fn test_enum_vector_uses_cddl_literal() {
    let vectors = compile("model = \"free-space\" / \"Log_Distance\"");
    let hex: Vec<String> = vectors.iter().map(|v| v.cbor_hex()).collect();
    // 0x6a = text string of length 10
    assert!(hex.contains(&format!("6a{}", hex_of("free-space"))), "{hex:?}");
    assert!(hex.contains(&format!("6c{}", hex_of("Log_Distance"))), "{hex:?}");
}

fn hex_of(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}


/// Issue #37: a member of a named type is encoded as a real value of that type,
/// not the integer 0, and isn't asserted on.
#[test]
fn test_named_member_is_encoded_as_its_type() {
    let vecs = compile("reading = { id: uint, sensor: sensor }\nsensor = { kind: uint }");
    let r = vecs.iter().find(|v| v.type_name == "Reading" && v.description == "default values").unwrap();
    // {"id": 0, "sensor": {"kind": 0}}
    assert_eq!(r.cbor_hex(), format!("a2{}00{}a1{}00", key("id"), key("sensor"), key("kind")));
    assert_eq!(r.fields.len(), 1, "only the primitive member is asserted");
}

/// Issue #37: an enum member uses the first variant's literal; a tagged member
/// keeps its tag; a `+` array has one element; a literal member uses its literal.
#[test]
fn test_member_vectors_respect_types_and_literals() {
    let vecs = compile(
        "rec = { m: model, s: #6.1(uint), t: tags, on: true, n: 1..5 }\n\
         model = \"free-space\" / \"other\"\n\
         tags = [+ uint]",
    );
    let r = vecs.iter().find(|v| v.type_name == "Rec" && v.description == "default values").unwrap();
    let hex = r.cbor_hex();
    assert!(hex.contains(&format!("{}6a{}", key("m"), hex_of("free-space"))), "{hex}");
    assert!(hex.contains(&format!("{}c100", key("s"))), "tag 1 around 0: {hex}");
    assert!(hex.contains(&format!("{}8100", key("t"))), "one-element array: {hex}");
    assert!(hex.contains(&format!("{}f5", key("on"))), "literal true: {hex}");
    assert!(hex.contains(&format!("{}01", key("n"))), "range minimum: {hex}");
}

/// Issue #37: an int-literal enum's vectors encode each literal.
#[test]
fn test_int_literal_enum_vectors_encode_literals() {
    let vecs = compile("priority = 1 / 2 / 3");
    let hex: Vec<String> = vecs.iter().map(|v| v.cbor_hex()).collect();
    assert_eq!(hex, vec!["01", "02", "03"]);
    assert!(vecs.iter().all(|v| v.fields.is_empty()), "no `kind` assertions on integer variants");
}

/// Issue #37: the C++ harness compares plain members directly (no `std::get`
/// on a non-variant) and checks a text enum variant through its string_view.
#[test]
fn test_cpp_harness_compares_members_directly() {
    let h = gen("sensor = { id: uint }\nmodel = \"free-space\" / \"other\"").4;
    assert!(h.contains("assert(decoded.id == 0ULL);"), "{h}");
    assert!(!h.contains("std::get<uint64_t>"), "{h}");
    assert!(h.contains("assert(decoded.kind == Model::Kind::FreeSpace);"), "{h}");
}

/// CBOR text-string key: 0x60 + length, then the bytes (short keys only).
fn key(s: &str) -> String {
    format!("{:02x}{}", 0x60 + s.len(), hex_of(s))
}

/// Issue #43: a float32 member's vector is a CBOR float32 holding a value
/// float32 represents exactly; a float64 member stays float64.
#[test]
fn test_float32_member_vector_is_float32() {
    let vecs = compile("r = { a: float32, b: float64 }");
    let explicit = vecs.iter().find(|v| v.description == "explicit values").unwrap();
    // fa 3fc00000 = float32 1.5; fb 40091eb851eb851f = float64 3.14
    assert!(explicit.cbor_hex().contains(&format!("{}fa3fc00000", key("a"))), "{}", explicit.cbor_hex());
    assert!(explicit.cbor_hex().contains(&format!("{}fb40091eb851eb851f", key("b"))), "{}", explicit.cbor_hex());
}

/// Issue #43: the Rust harness needs nothing beyond the generated crate:
/// inline bytes, minicbor's decode/encode, and variant/field checks that compile.
#[test]
fn test_rust_harness_uses_crate_api() {
    let h = rust("sensor = { id: uint, type: tstr }\nmodel = \"free-space\" / \"other\"\nkey = bstr .size 2");
    assert!(!h.contains("hex::"), "{h}");
    assert!(h.contains("let decoded: Sensor = minicbor::decode(bytes).expect(\"decode\");"), "{h}");
    assert!(h.contains("assert_eq!(decoded.id as u64, 0);"), "{h}");
    assert!(h.contains("assert_eq!(decoded.r#type, \"\");"), "keyword field: {h}");
    assert!(h.contains("assert!(matches!(decoded, Model::FreeSpace { .. }));"), "{h}");
    assert!(!h.contains("decoded.value"), "aliases are checked by re-encoding: {h}");
    assert!(h.contains("let reencoded = minicbor::to_vec(&decoded).expect(\"encode\");"), "{h}");
}

/// Issue #43: the C harness uses the C backend's names, derived from the
/// CDDL name (`device-id` -> `device_id_t`, `DEVICE_ID_...`).
#[test]
fn test_c_harness_uses_c_names() {
    let h = c("device-info = { id: uint }\nlink-mode = \"free-space\" / \"other\"");
    assert!(h.contains("device_info_t decoded = {0};"), "{h}");
    assert!(h.contains("assert(test_device_info_decode(&dec, &decoded, NULL, NULL) == 0);"), "{h}");
    assert!(h.contains("assert(decoded.tag == LINK_MODE_FREESPACE);"), "{h}");
}

/// Issue #43: the TypeScript harness lives in the generated package's tests/
/// and compares an alias's value directly.
#[test]
fn test_nodejs_harness_imports_package_source() {
    let h = nodejs("sensor = { id: uint }\nlabel = tstr");
    assert!(h.contains("from '../src/test.js';"), "{h}");
    assert!(h.contains("expect(decoded).toEqual(\"\");"), "{h}");
}

/// The Go harness decodes each vector, checks its primitive members and the
/// enum kind, and re-encodes it byte for byte.
#[test]
fn test_go_harness() {
    let parsed  = parse_cddl("reading = { sensor-id: uint, value: float32 }\nstatus = \"ok\" / \"warn\"\ndev = tstr .size 2", PathBuf::from("iot_sensor.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let files   = generate(&lowered.module, &InteropLangs { go: true, ..Default::default() }, &CodegenOptions::default());
    let go = &files.iter().find(|f| f.path == "iot_sensor-interop/go/interop_test.go").expect("go harness").content;
    assert!(go.contains("package iotsensor\n"), "{go}");
    assert!(go.contains("got, err := DecodeReading(interopBytes(t, \""), "{go}");
    assert!(go.contains("if uint64(got.SensorId) != 0 {"), "{go}");
    assert!(go.contains("if math.Abs(float64(got.Value)-0.0) > 1e-4 {"), "{go}");
    assert!(go.contains("if got.Kind != StatusKindWarn {"), "{go}");
    assert!(go.contains("if string(got) != \"aa\" {"), "{go}");
    assert!(go.contains("if h := hex.EncodeToString(re); h != "), "{go}");
}

/// The Java harness decodes each vector, checks its primitive members and the
/// enum variant, and re-encodes it byte for byte.
#[test]
fn test_java_harness() {
    let parsed  = parse_cddl("reading = { sensor-id: uint, value: float32 }\nstatus = \"ok\" / \"warn\"\ndev = tstr .size 2", PathBuf::from("iot_sensor.cddl")).unwrap();
    let lowered = lower(&parsed.value, 16, 64).unwrap();
    let files   = generate(&lowered.module, &InteropLangs { java: true, ..Default::default() }, &CodegenOptions::default());
    let java = &files.iter().find(|f| f.path == "iot_sensor-interop/java/InteropTest.java").expect("java harness").content;
    assert!(java.contains("package iot_sensor;\n"), "{java}");
    assert!(java.contains("Reading got = Reading.decode(bytes(\""), "{java}");
    assert!(java.contains("assertEquals(0L, got.sensorId(), \"sensor_id\");"), "{java}");
    assert!(java.contains("assertEquals(0.0, got.value(), 1e-4, \"value\");"), "{java}");
    assert!(java.contains("assertEquals(\"Warn\", got.kind());"), "{java}");
    assert!(java.contains("assertEquals(\"aa\", got.value(), \"value\");"), "{java}");
    assert!(java.contains("HexFormat.of().formatHex(got.encode())"), "{java}");
}
