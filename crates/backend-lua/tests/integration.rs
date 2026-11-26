//! Integration tests for backend-lua: the shape of the generated package. The
//! generated code runs under busted in backend-buildtest (`test_build_lua`,
//! `test_build_lua_json`).

use std::path::PathBuf;

use backend_lua::{lua_bytes, lua_member_name, lua_module_name, LuaBackend};
use cddlc_codegen::{Backend, CodegenOptions, Format};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

struct Package {
    paths: Vec<String>,
    rockspec: String,
    module: String,
    spec: String,
}

fn gen(src: &str, format: Format) -> Package {
    let parsed = parse_cddl(src, PathBuf::from("my-schema.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts = CodegenOptions {
        format,
        ..Default::default()
    };
    let out = LuaBackend
        .generate(&lowered.module, &opts)
        .expect("generate");
    assert_eq!(out.files.len(), 3, "rockspec, module, spec");
    Package {
        paths: out.files.iter().map(|f| f.path.clone()).collect(),
        rockspec: out.files[0].content.clone(),
        module: out.files[1].content.clone(),
        spec: out.files[2].content.clone(),
    }
}

fn cbor(src: &str) -> Package {
    gen(src, Format::Cbor)
}

#[test]
fn test_package_layout() {
    let p = cbor("x = uint");
    assert_eq!(
        p.paths,
        [
            "my-schema-cbor/my-schema-cbor-0.1.0-1.rockspec",
            "my-schema-cbor/src/my_schema.lua",
            "my-schema-cbor/spec/my_schema_spec.lua",
        ]
    );
    assert!(
        p.rockspec.contains("package = \"my-schema-cbor\""),
        "{}",
        p.rockspec
    );
    assert!(p.rockspec.contains("\"lua >= 5.3\""), "{}", p.rockspec);
    assert!(
        p.rockspec
            .contains("[\"my_schema\"] = \"src/my_schema.lua\""),
        "{}",
        p.rockspec
    );
    assert!(
        p.module.contains("local M = {}") && p.module.trim_end().ends_with("return M"),
        "{}",
        p.module
    );
    assert!(
        p.spec.contains("local M = require(\"my_schema\")"),
        "{}",
        p.spec
    );
}

#[test]
fn test_names() {
    assert_eq!(lua_module_name("iot_sensor", None), "iot_sensor");
    assert_eq!(lua_module_name("simple-values", None), "simple_values");
    assert_eq!(lua_module_name("x", Some("acme.End")), "acme.end_");
    assert_eq!(lua_member_name("sensor-id"), "sensor_id");
    assert_eq!(lua_member_name("end"), "end_");
    assert_eq!(lua_member_name("encode"), "encode_");
    assert_eq!(lua_bytes(&[0xca, 0xfe]), "\"\\202\\254\"");
    assert_eq!(
        lua_bytes(&[1, b'2']),
        "\"\\0012\"",
        "a digit after an escape needs three digits"
    );
}

#[test]
fn test_codec_is_embedded_per_format() {
    let c = cbor("x = uint");
    assert!(
        c.module.contains("-- ── CBOR codec") && c.module.contains("local function head(major, n)"),
        "{}",
        c.module
    );
    assert!(!c.module.contains("require(\"cbor\")") && !c.module.contains("dkjson"));
    let j = gen("x = uint", Format::Json);
    assert!(
        j.module.contains("-- ── JSON codec") && j.module.contains("local function b64url(s)"),
        "{}",
        j.module
    );
    assert!(
        j.module.contains("function R.tag(_, item) return item end"),
        "{}",
        j.module
    );
}

#[test]
fn test_struct_members_in_schema_order() {
    let p = cbor("reading = { sensor-id: uint, value: float32, ? unit: tstr .size 4, raw: bstr }");
    let m = &p.module;
    assert!(m.contains("M.Reading.__index = M.Reading"), "{m}");
    assert!(
        m.contains("function M.Reading.new(t) return setmetatable(t or {}, M.Reading) end"),
        "{m}"
    );
    assert!(m.contains("  m[#m + 1] = { \"sensor-id\", R.uint(v.sensor_id) }\n  m[#m + 1] = { \"value\", R.f32(v.value) }\n  if v.unit ~= nil then m[#m + 1] = { \"unit\", R.text(v.unit) } end\n  m[#m + 1] = { \"raw\", R.bytes(v.raw) }"), "{m}");
    assert!(m.contains("v.sensor_id = R.as_uint(R.required(m, \"sensor-id\", \"Reading\"), \"Reading.sensor-id\")"), "{m}");
    assert!(
        m.contains(
            "if R.present(m, \"unit\") then v.unit = R.as_text(m[\"unit\"], \"Reading.unit\") end"
        ),
        "{m}"
    );
    // Lua values are type-checked before encoding; # counts UTF-8 bytes.
    assert!(
        m.contains("R.want_integer(v.sensor_id, \"Reading.sensor-id\")"),
        "{m}"
    );
    assert!(
        m.contains("if not (#v.unit == 4) then R.fail(\"Reading.unit\", \"length must be 4\") end"),
        "{m}"
    );
}

#[test]
fn test_enum_kinds_and_literals() {
    let p = cbor("level = uint / \"off\"\nprio = 1 / -2");
    let m = &p.module;
    assert!(m.contains("function M.Level.Uint(value) return setmetatable({ kind = \"Uint\", value = value }, M.Level) end"), "{m}");
    assert!(
        m.contains("function M.Level.Off() return setmetatable({ kind = \"Off\" }, M.Level) end"),
        "{m}"
    );
    assert!(
        m.contains("elseif v.kind == \"Off\" then raw = R.text(\"off\")"),
        "{m}"
    );
    assert!(
        m.contains("if item.v == \"off\" then return M.Level.Off() end"),
        "{m}"
    );
    assert!(
        m.contains("elseif v.kind == \"Variant2\" then raw = R.int(-2)"),
        "{m}"
    );
    assert!(
        m.contains("if item.v == -2 then return M.Prio.Variant2() end"),
        "{m}"
    );
    assert!(
        p.spec
            .contains("assert.has_error(function() M.Prio.decode(string.char(0)) end)"),
        "{}",
        p.spec
    );
}

#[test]
fn test_uint_is_unsigned() {
    let p = cbor("small = uint .le 10\nbig = uint .ge 9223372036854775808");
    assert!(
        p.module.contains("if not (R.ule(v, 10)) then"),
        "{}",
        p.module
    );
    assert!(p
        .module
        .contains("function R.ule(a, b) return a == b or math.ult(a, b) end"));
}

#[test]
fn test_tags_and_nil() {
    let p = cbor("stamp = #6.1(uint)\nmsg = { at: #6.1(uint), ? gone: nil }");
    let m = &p.module;
    assert!(m.contains("return R.tag(1, R.uint(v))"), "{m}");
    assert!(
        m.contains("local v = R.as_uint(R.untag(raw, 1, \"Stamp\"), \"Stamp\")"),
        "{m}"
    );
    assert!(
        m.contains("m[#m + 1] = { \"at\", R.tag(1, R.uint(v.at)) }"),
        "{m}"
    );
    // A present nil member is M.null; absent is nil.
    assert!(
        m.contains("if m[\"gone\"] ~= nil then v.gone = R.as_null(m[\"gone\"], \"Msg.gone\") end"),
        "{m}"
    );
    assert!(m.contains("M.null = NULL"), "{m}");
}

#[test]
fn test_specs_roundtrip_with_deep_equality() {
    let p = cbor("s = { a: uint, ? b: tstr }\ndev = tstr .size 2");
    assert!(
        p.spec.contains("local original = M.S.new({ a = 0 })"),
        "{}",
        p.spec
    );
    assert!(
        p.spec
            .contains("local original = M.S.new({ a = 42, b = \"hello\" })"),
        "{}",
        p.spec
    );
    assert!(
        p.spec
            .contains("assert.same(original, M.S.decode(M.S.encode(original)))"),
        "{}",
        p.spec
    );
    assert!(
        p.spec
            .contains("assert.has_error(function() M.Dev.encode(string.rep(\"a\", 3)) end)"),
        "{}",
        p.spec
    );
}
