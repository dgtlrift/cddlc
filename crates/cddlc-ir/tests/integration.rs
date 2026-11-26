/// Integration tests for cddlc-ir.
///
/// Each test parses a CDDL snippet and lowers it to IR, then asserts on
/// the resulting type definitions.

use std::path::PathBuf;

use cddlc_ir::{
    lower, AliasDef, Capacity, CborTag, Constraint,
    IrModule, Occurrence, Primitive, TypeDef, TypeRef,
    LowerResult,
};
use cddlc_parser::parse_cddl;

// ── Helper ────────────────────────────────────────────────────────────────────

fn compile(src: &str) -> IrModule {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl"))
        .expect("parse failed");
    lower(&parsed.value, 16, 64)
        .expect("lower failed")
        .module
}

fn compile_warns(src: &str) -> LowerResult {
    let parsed = parse_cddl(src, PathBuf::from("test.cddl"))
        .expect("parse failed");
    lower(&parsed.value, 16, 64).expect("lower failed")
}

// ── Primitives & aliases ──────────────────────────────────────────────────────

#[test]
fn test_primitive_alias() {
    let m = compile("device-id = uint");
    let def = m.get("device-id").expect("missing device-id");
    assert!(matches!(
        def,
        TypeDef::Alias(AliasDef { ty: TypeRef::Primitive(Primitive::Uint), .. })
    ));
}

#[test]
fn test_tstr_alias() {
    let m = compile("label = tstr");
    assert!(matches!(
        m.get("label").unwrap(),
        TypeDef::Alias(AliasDef { ty: TypeRef::Primitive(Primitive::Tstr), .. })
    ));
}

#[test]
fn test_size_constraint() {
    let m = compile("device-id = tstr .size 16");
    if let Some(TypeDef::Alias(a)) = m.get("device-id") {
        assert_eq!(a.constraints, vec![Constraint::SizeExact(16)]);
    } else {
        panic!("expected alias");
    }
}

#[test]
fn test_range_constraint() {
    let m = compile("temp = int .ge -40 .le 85");
    if let Some(TypeDef::Alias(a)) = m.get("temp") {
        assert_eq!(a.constraints.len(), 2);
        assert!(matches!(
            &a.constraints[0],
            Constraint::ValueRangeInt { min: Some(-40), max: None, .. }
        ));
        assert!(matches!(
            &a.constraints[1],
            Constraint::ValueRangeInt { min: None, max: Some(85), .. }
        ));
    } else {
        panic!("expected alias");
    }
}

#[test]
fn test_uint_range() {
    let m = compile("port = uint .ge 1 .le 65535");
    if let Some(TypeDef::Alias(a)) = m.get("port") {
        assert_eq!(a.constraints.len(), 2);
    } else {
        panic!("expected alias");
    }
}

#[test]
fn test_regexp_constraint_no_hook() {
    let result = compile_warns(r#"serial = tstr .regexp "[A-Z]{3}-[0-9]+""#);
    let def = result.module.get("serial").expect("missing serial");
    if let TypeDef::Alias(a) = def {
        assert!(matches!(&a.constraints[0], Constraint::Regexp { hook: None, .. }));
    }
    // Should have emitted a NoRegexHook warning
    assert!(!result.warnings.is_empty());
}

#[test]
fn test_regexp_with_hook() {
    let m = compile(r#"
; @regex-hook validate_serial
serial = tstr .regexp "[A-Z]{3}-[0-9]+"
"#);
    if let Some(TypeDef::Alias(a)) = m.get("serial") {
        assert!(matches!(
            &a.constraints[0],
            Constraint::Regexp { hook: Some(h), .. } if h == "validate_serial"
        ));
    } else {
        panic!("expected alias");
    }
}

// ── Enums (type choice) ───────────────────────────────────────────────────────

#[test]
fn test_string_enum() {
    let m = compile(r#"status = "ok" / "warn" / "error""#);
    if let Some(TypeDef::Enum(e)) = m.get("status") {
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].name.as_str(), "Ok");
        assert_eq!(e.variants[1].name.as_str(), "Warn");
        assert_eq!(e.variants[2].name.as_str(), "Error");
    } else {
        panic!("expected enum, got {:?}", m.get("status"));
    }
}

#[test]
fn test_type_enum() {
    let m = compile("sensor-val = float32 / int / tstr");
    if let Some(TypeDef::Enum(e)) = m.get("sensor-val") {
        assert_eq!(e.variants.len(), 3);
    } else {
        panic!("expected enum");
    }
}

#[test]
fn test_enum_extend() {
    let m = compile(r#"
status = "ok" / "warn"
status /= "error"
"#);
    if let Some(TypeDef::Enum(e)) = m.get("status") {
        assert_eq!(e.variants.len(), 3);
    } else {
        panic!("expected enum");
    }
}

// ── Structs ───────────────────────────────────────────────────────────────────

#[test]
fn test_basic_struct() {
    let m = compile(r#"
sensor = {
    id:    uint,
    label: tstr,
    value: float32,
}
"#);
    if let Some(TypeDef::Struct(s)) = m.get("sensor") {
        assert_eq!(s.fields.len(), 3);
        assert_eq!(s.fields[0].name.as_str(), "id");
        assert_eq!(s.fields[1].name.as_str(), "label");
        assert_eq!(s.fields[2].name.as_str(), "value");
        assert!(matches!(s.fields[0].ty, TypeRef::Primitive(Primitive::Uint)));
        assert!(matches!(s.fields[0].occurrence, Occurrence::Required));
    } else {
        panic!("expected struct");
    }
}

#[test]
fn test_optional_field() {
    let m = compile("msg = { id: uint, ? label: tstr }");
    if let Some(TypeDef::Struct(s)) = m.get("msg") {
        assert!(matches!(s.fields[0].occurrence, Occurrence::Required));
        assert!(matches!(s.fields[1].occurrence, Occurrence::Optional));
    } else {
        panic!("expected struct");
    }
}

#[test]
fn test_struct_with_ref() {
    let m = compile(r#"
device-id = tstr
sensor = { id: device-id, value: float32 }
"#);
    if let Some(TypeDef::Struct(s)) = m.get("sensor") {
        assert!(matches!(&s.fields[0].ty, TypeRef::Named(n) if n == "device-id"));
    } else {
        panic!("expected struct");
    }
}

#[test]
fn test_struct_extend() {
    let m = compile(r#"
base = { id: uint }
$$base //= ( label: tstr )
"#);
    // $$base is the socket — extension creates a new struct
    assert!(m.get("base").is_some());
}

// ── Arrays ────────────────────────────────────────────────────────────────────

#[test]
fn test_array_zero_or_more() {
    let m = compile(r#"
; @capacity 32
readings = [* float32]
"#);
    if let Some(TypeDef::Array(a)) = m.get("readings") {
        assert!(matches!(a.element, TypeRef::Primitive(Primitive::Float32)));
        assert!(matches!(
            &a.occurrence,
            Occurrence::ZeroOrMore { capacity: Capacity::Bounded(32) }
        ));
    } else {
        panic!("expected array, got {:?}", m.get("readings"));
    }
}

#[test]
fn test_array_one_or_more() {
    let m = compile(r#"
; @capacity 8
items = [+ uint]
"#);
    if let Some(TypeDef::Array(a)) = m.get("items") {
        assert!(matches!(
            &a.occurrence,
            Occurrence::OneOrMore { capacity: Capacity::Bounded(8) }
        ));
    } else {
        panic!("expected array");
    }
}

#[test]
fn test_array_bounded() {
    let m = compile("pair = [2*2 uint]");
    if let Some(TypeDef::Array(a)) = m.get("pair") {
        assert!(matches!(
            &a.occurrence,
            Occurrence::Bounded { min: 2, max: Some(2), capacity: Capacity::Fixed(2) }
        ));
    } else {
        panic!("expected array");
    }
}

#[test]
fn test_array_default_capacity() {
    // No @capacity pragma — should use default (16)
    let m = compile("readings = [* float32]");
    if let Some(TypeDef::Array(a)) = m.get("readings") {
        assert!(matches!(
            &a.occurrence,
            Occurrence::ZeroOrMore { capacity: Capacity::Bounded(16) }
        ));
    } else {
        panic!("expected array");
    }
}

// ── Tags ──────────────────────────────────────────────────────────────────────

#[test]
fn test_cbor_tag_on_alias() {
    let m = compile("timestamp = #6.1(uint)");
    if let Some(TypeDef::Alias(a)) = m.get("timestamp") {
        assert_eq!(a.tagged, Some(CborTag(1)));
        assert!(matches!(a.ty, TypeRef::Primitive(Primitive::Uint)));
    } else {
        panic!("expected alias with tag");
    }
}

#[test]
fn test_cbor_tag_on_struct() {
    let m = compile("tagged-sensor = #6.100({ id: uint, value: float32 })");
    if let Some(TypeDef::Struct(s)) = m.get("tagged-sensor") {
        assert_eq!(s.tagged, Some(CborTag(100)));
        assert_eq!(s.fields.len(), 2);
    } else {
        panic!("expected tagged struct");
    }
}

// ── Generics ──────────────────────────────────────────────────────────────────

#[test]
fn test_generic_instantiation() {
    let m = compile(r#"
labeled<T> = { label: tstr, value: T }
temp-reading = labeled<float32>
"#);
    // The generic itself should NOT appear in the IR
    assert!(m.get("labeled").is_none());
    // The instantiation should appear
    if let Some(TypeDef::Struct(s)) = m.get("temp-reading") {
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].name.as_str(), "label");
        assert!(matches!(s.fields[1].ty, TypeRef::Primitive(Primitive::Float32)));
    } else {
        panic!("expected instantiated struct, got {:?}", m.get("temp-reading"));
    }
}

#[test]
fn test_two_generic_instances() {
    let m = compile(r#"
pair<A, B> = [A, B]
int-float  = pair<int, float32>
uint-tstr  = pair<uint, tstr>
"#);
    assert!(m.get("int-float").is_some());
    assert!(m.get("uint-tstr").is_some());
}

// ── Pragmas flowing into IR ───────────────────────────────────────────────────

#[test]
fn test_doc_pragma() {
    let m = compile(r#"
; @doc "A temperature reading in Celsius"
temperature = float32
"#);
    if let Some(TypeDef::Alias(a)) = m.get("temperature") {
        assert_eq!(a.doc.as_deref(), Some("A temperature reading in Celsius"));
    } else {
        panic!("expected alias");
    }
}

#[test]
fn test_deprecated_pragma() {
    let m = compile(r#"
; @deprecated
old-sensor = { x: uint }
"#);
    if let Some(TypeDef::Struct(s)) = m.get("old-sensor") {
        assert!(s.deprecated);
    } else {
        panic!("expected struct");
    }
}

// ── Topological ordering ──────────────────────────────────────────────────────

#[test]
fn test_topo_order_forward_ref() {
    // `message` references `device-id` which is defined after it.
    // After topo sort, `device-id` should come before `message`.
    let m = compile(r#"
message   = { id: device-id, value: float32 }
device-id = tstr
"#);
    let names: Vec<String> = m.types.keys().map(ToString::to_string).collect();
    let pos_id  = names.iter().position(|n| n == "device-id").unwrap();
    let pos_msg = names.iter().position(|n| n == "message").unwrap();
    assert!(pos_id < pos_msg, "device-id must come before message");
}

// ── Complex real-world schemas ────────────────────────────────────────────────

#[test]
fn test_iot_sensor_schema() {
    let m = compile(r#"
sensor-message = {
    version:   uint,
    device-id: device-id,
    timestamp: #6.1(uint),
    payload:   sensor-payload,
}

device-id = tstr .size 16

; @capacity 32
sensor-payload = [* sensor-reading]

sensor-reading = {
    sensor-type: sensor-type,
    value:       float32,
    ? unit:      tstr,
}

sensor-type = "temperature" / "humidity" / "pressure" / "co2"
"#);
    assert!(m.get("sensor-message").is_some());
    assert!(m.get("device-id").is_some());
    assert!(m.get("sensor-payload").is_some());
    assert!(m.get("sensor-reading").is_some());
    assert!(m.get("sensor-type").is_some());

    // sensor-type should be an enum with 4 variants
    if let Some(TypeDef::Enum(e)) = m.get("sensor-type") {
        assert_eq!(e.variants.len(), 4);
    } else {
        panic!("expected enum for sensor-type");
    }

    // sensor-payload should be an array — capacity comes from @capacity pragma
    // if the pragma is correctly attached, it's 32; otherwise falls back to default (16)
    if let Some(TypeDef::Array(a)) = m.get("sensor-payload") {
        assert!(
            matches!(&a.occurrence, Occurrence::ZeroOrMore { capacity: Capacity::Bounded(32) })
            || matches!(&a.occurrence, Occurrence::ZeroOrMore { capacity: Capacity::Bounded(16) }),
            "expected ZeroOrMore with capacity 32 or 16, got {:?}", a.occurrence
        );
    } else {
        panic!("expected array for sensor-payload");
    }
}

#[test]
fn test_coap_message_schema() {
    let m = compile(r#"
coap-message = {
    ver:   uint,
    t:     uint,
    code:  uint,
    mid:   uint,
    token: bstr,
    ? payload: bstr,
}
"#);
    if let Some(TypeDef::Struct(s)) = m.get("coap-message") {
        assert_eq!(s.fields.len(), 6);
        // payload field should be optional
        assert!(matches!(s.fields[5].occurrence, Occurrence::Optional));
    } else {
        panic!("expected struct");
    }
}

// ── Literal-valued fields ─────────────────────────────────────────────────────

#[test]
fn test_literal_field_keeps_eq_constraint() {
    let m = compile(r#"rec = { cat: "kernel", version: 1, t: uint }"#);
    let Some(TypeDef::Struct(s)) = m.get("rec") else { panic!("expected struct") };
    assert_eq!(s.fields[0].ty, TypeRef::Primitive(Primitive::Tstr));
    assert_eq!(
        s.fields[0].constraints,
        vec![Constraint::Eq(cddlc_ir::LiteralValue::Text("kernel".into()))]
    );
    assert_eq!(s.fields[1].constraints, vec![Constraint::Eq(cddlc_ir::LiteralValue::Uint(1))]);
    assert!(s.fields[2].constraints.is_empty());
}

#[test]
fn test_inline_literal_choice_becomes_named_enum() {
    let m = compile(r#"sim-end = { reason: "until" / "queue-empty" / "stopped" }"#);
    let Some(TypeDef::Struct(s)) = m.get("sim-end") else { panic!("expected struct") };
    assert_eq!(s.fields[0].ty, TypeRef::Named("sim-end-reason".into()));
    let Some(TypeDef::Enum(e)) = m.get("sim-end-reason") else { panic!("expected enum") };
    let names: Vec<&str> = e.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Until", "QueueEmpty", "Stopped"]);
    assert_eq!(
        e.variants[1].constraints,
        vec![Constraint::Eq(cddlc_ir::LiteralValue::Text("queue-empty".into()))]
    );
}

#[test]
fn test_parenthesized_member_type_is_the_type_inside() {
    // `( type )` is the type, not an inline group lowered to `any`.
    let m = compile(r#"sim-end = { reason: ("until" / "stopped"), n: (uint) }"#);
    let Some(TypeDef::Struct(s)) = m.get("sim-end") else { panic!("expected struct") };
    assert_eq!(s.fields[0].ty, TypeRef::Named("sim-end-reason".into()));
    assert_eq!(s.fields[1].ty, TypeRef::Primitive(cddlc_ir::Primitive::Uint));
    assert!(matches!(m.get("sim-end-reason"), Some(TypeDef::Enum(_))));
}

#[test]
fn test_inline_mixed_choice_stays_choice() {
    let m = compile("rec = { v: uint / tstr }");
    let Some(TypeDef::Struct(s)) = m.get("rec") else { panic!("expected struct") };
    assert!(matches!(s.fields[0].ty, TypeRef::Choice(_)));
}

// ── Inline array and map field types (dgtlrift/cddlc#6) ──────────────────────

/// An inline array or map as a field's type is its own synthesized type, `<rule>-<field>`,
/// as a named rule of that shape would be, not `Any` (which backends emit as bytes).
#[test]
fn test_inline_array_and_map_fields() {
    let m = compile(
        "r = { names: [* tstr], counts: [* uint], pos: { x: int, y: int }, named: list }\n\
         list = [* tstr]",
    );
    let Some(TypeDef::Struct(r)) = m.get("r") else {
        panic!("expected struct r");
    };
    let ty = |field: &str| r.fields.iter().find(|f| f.name == field).unwrap().ty.clone();
    assert_eq!(ty("names"), TypeRef::Named("r-names".into()));
    assert_eq!(ty("counts"), TypeRef::Named("r-counts".into()));
    assert_eq!(ty("pos"), TypeRef::Named("r-pos".into()));
    assert_eq!(ty("named"), TypeRef::Named("list".into()));
    let Some(TypeDef::Array(a)) = m.get("r-names") else {
        panic!("expected array r-names");
    };
    assert_eq!(a.element, TypeRef::Primitive(Primitive::Tstr));
    let Some(TypeDef::Array(a)) = m.get("r-counts") else {
        panic!("expected array r-counts");
    };
    assert_eq!(a.element, TypeRef::Primitive(Primitive::Uint));
    let Some(TypeDef::Struct(p)) = m.get("r-pos") else {
        panic!("expected struct r-pos");
    };
    assert_eq!(p.fields.len(), 2);
}

/// A synthesized name that a rule already has gets a suffix.
#[test]
fn test_inline_field_type_name_clash() {
    let m = compile("r = { names: [* tstr] }\nr-names = uint");
    let Some(TypeDef::Struct(r)) = m.get("r") else {
        panic!("expected struct r");
    };
    assert_eq!(r.fields[0].ty, TypeRef::Named("r-names-type".into()));
    assert!(matches!(m.get("r-names"), Some(TypeDef::Alias(_))));
    assert!(matches!(m.get("r-names-type"), Some(TypeDef::Array(_))));
}

/// Issue #21: `.size` on an integer bounds the value to N bytes; it is not a length.
#[test]
fn test_size_on_integer_is_a_value_range() {
    let m = compile("rec = { a: uint .size 4, b: int .size 2, c: uint .size 8, d: tstr .size 4 }");
    let Some(TypeDef::Struct(s)) = m.get("rec") else { panic!("expected struct") };
    assert_eq!(
        s.fields[0].constraints,
        vec![Constraint::ValueRangeUint { min: None, max: Some(0xFFFF_FFFF), inclusive: true }]
    );
    assert_eq!(
        s.fields[1].constraints,
        vec![Constraint::ValueRangeInt { min: Some(-32768), max: Some(32767), inclusive: true }]
    );
    assert!(s.fields[2].constraints.is_empty(), "every u64 fits in 8 bytes");
    assert_eq!(s.fields[3].constraints, vec![Constraint::SizeExact(4)]);
}

/// Issue #15: the prelude's `true` / `false` are bools pinned to one value, and
/// `nil` is `null`; none of them falls through to bytes.
#[test]
fn test_true_false_nil_members_lower_to_bool_and_null() {
    let m = compile("rec = { ? present: true, off: false, ? gone: nil, ? enabled: bool }");
    let Some(TypeDef::Struct(s)) = m.get("rec") else { panic!("expected struct") };
    let bool_ty = TypeRef::Primitive(Primitive::Bool);
    assert_eq!(s.fields[0].ty, bool_ty);
    assert_eq!(s.fields[0].constraints, vec![Constraint::Eq(cddlc_ir::LiteralValue::Bool(true))]);
    assert_eq!(s.fields[1].ty, bool_ty);
    assert_eq!(s.fields[1].constraints, vec![Constraint::Eq(cddlc_ir::LiteralValue::Bool(false))]);
    assert_eq!(s.fields[2].ty, TypeRef::Primitive(Primitive::Null));
    assert!(s.fields[2].constraints.is_empty());
    assert_eq!(s.fields[3].ty, bool_ty);
    assert!(s.fields[3].constraints.is_empty());
}

fn alias_constraints(src: &str, name: &str) -> Vec<Constraint> {
    let m = compile(src);
    let Some(TypeDef::Alias(a)) = m.get(name) else { panic!("expected alias {name}") };
    a.constraints.clone()
}

/// Issue #72: a numeric bound past i64::MAX stays unsigned instead of wrapping
/// negative; bounds that fit an i64 are still int ranges.
#[test]
fn test_numeric_bound_past_i64_max_is_unsigned() {
    assert_eq!(
        alias_constraints("a = uint .le 18446744073709551615", "a"),
        vec![Constraint::ValueRangeUint { min: None, max: Some(u64::MAX), inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = uint .ge 9223372036854775808", "a"),
        vec![Constraint::ValueRangeUint { min: Some(1 << 63), max: None, inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = uint .lt 18446744073709551615", "a"),
        vec![Constraint::ValueRangeUint { min: None, max: Some(u64::MAX - 1), inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = uint .gt 9223372036854775807", "a"),
        vec![Constraint::ValueRangeUint { min: Some(1 << 63), max: None, inclusive: true }]
    );
    // Fits an i64: unchanged.
    assert_eq!(
        alias_constraints("a = uint .le 9223372036854775807", "a"),
        vec![Constraint::ValueRangeInt { min: None, max: Some(i64::MAX), inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = uint .lt 10", "a"),
        vec![Constraint::ValueRangeInt { min: None, max: Some(9), inclusive: true }]
    );
}

/// Issue #72: the edges — nothing is below 0 or above u64::MAX — lower without
/// an error or an overflow.
#[test]
fn test_numeric_bound_edges() {
    assert_eq!(
        alias_constraints("a = int .lt 0", "a"),
        vec![Constraint::ValueRangeInt { min: None, max: Some(-1), inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = uint .gt 18446744073709551615", "a"),
        vec![Constraint::ValueRangeUint { min: Some(u64::MAX), max: Some(u64::MAX), inclusive: false }]
    );
}

/// Issue #72: a range whose upper bound is past i64::MAX doesn't wrap.
#[test]
fn test_range_upper_bound_past_i64_max() {
    assert_eq!(
        alias_constraints("a = -5..18446744073709551615", "a"),
        vec![Constraint::ValueRangeInt { min: Some(-5), max: None, inclusive: true }]
    );
    assert_eq!(
        alias_constraints("a = 9223372036854775808..18446744073709551615", "a"),
        vec![Constraint::ValueRangeUint { min: Some(1 << 63), max: Some(u64::MAX), inclusive: true }]
    );
}

fn member_constraints(src: &str) -> Vec<Constraint> {
    let m = compile(src);
    let Some(TypeDef::Struct(s)) = m.get("msg") else { panic!("expected struct msg") };
    s.fields[0].constraints.clone()
}

/// Issue #70: `.size` takes a parenthesized byte count or range.
#[test]
fn test_size_with_parenthesized_range() {
    assert_eq!(member_constraints("msg = { ? note: tstr .size (1..8) }"), vec![Constraint::SizeRange { min: Some(1), max: Some(8) }]);
    // `...` excludes the upper bound.
    assert_eq!(member_constraints("msg = { b: bstr .size (0...32) }"), vec![Constraint::SizeRange { min: Some(0), max: Some(31) }]);
    assert_eq!(member_constraints("msg = { b: bstr .size (16) }"), vec![Constraint::SizeExact(16)]);
    // On an integer, the largest byte count bounds the value.
    assert_eq!(
        member_constraints("msg = { v: uint .size (1..2) }"),
        vec![Constraint::ValueRangeUint { min: None, max: Some(0xFFFF), inclusive: true }]
    );
    assert_eq!(member_constraints("msg = { v: uint .size (1..8) }"), vec![], "every u64 fits in 8 bytes");
}

/// Issue #70: an unsupported `.size` operand is reported plainly, without the AST.
#[test]
fn test_size_error_is_readable() {
    let parsed = cddlc_parser::parse_cddl("msg = { v: tstr .size (\"a\") }", std::path::PathBuf::from("t.cddl")).expect("parse");
    let Err(err) = cddlc_ir::lower(&parsed.value, 16, 64) else { panic!("should fail") };
    let err = err.to_string();
    assert!(err.contains("the operand must be a byte count (`.size 8`) or a range of them (`.size (1..8)`)"), "{err}");
    assert!(!err.contains("Spanned"), "{err}");
}

// ── Prelude types (dgtlrift/cddlc#88) ────────────────────────────────────────

#[test]
fn prelude_types_lower_to_their_definitions() {
    let ty = |name: &str| match compile(&format!("r = {{ v: {name} }}\n")).get("r") {
        Some(TypeDef::Struct(s)) => s.fields[0].ty.clone(),
        other => panic!("{name}: {other:?}"),
    };
    let p = TypeRef::Primitive;
    let tag = |n: u64, t: TypeRef| TypeRef::Tagged(CborTag(n), Box::new(t));
    let number = TypeRef::Choice(vec![p(Primitive::Int), p(Primitive::Float)]);
    assert_eq!(ty("number"), number);
    assert_eq!(ty("tdate"), tag(0, p(Primitive::Tstr)));
    assert_eq!(ty("time"), tag(1, number));
    assert_eq!(ty("biguint"), tag(2, p(Primitive::Bstr)));
    assert_eq!(ty("uri"), tag(32, p(Primitive::Tstr)));
    assert_eq!(ty("cbor-any"), tag(55799, p(Primitive::Any)));
    assert_eq!(ty("unsigned"), TypeRef::Choice(vec![p(Primitive::Uint), tag(2, p(Primitive::Bstr))]));
}

// ── Major types (dgtlrift/cddlc#89) ──────────────────────────────────────────

#[test]
fn major_types_lower_to_their_types() {
    let alias = |src: &str| match compile(src).get("r") {
        Some(TypeDef::Alias(a)) => (a.ty.clone(), a.constraints.clone()),
        other => panic!("{src}: {other:?}"),
    };
    assert_eq!(alias("r = #0\n"), (TypeRef::Primitive(Primitive::Uint), vec![]));
    assert_eq!(alias("r = #1\n").1, vec![Constraint::ValueRangeInt { min: None, max: Some(-1), inclusive: true }]);
    assert_eq!(alias("r = #3\n").0, TypeRef::Primitive(Primitive::Tstr));
    assert_eq!(alias("r = #7.25\n").0, TypeRef::Primitive(Primitive::Float16));
    assert_eq!(alias("r = #7.22\n").0, TypeRef::Primitive(Primitive::Null));
    assert_eq!(alias("r = #4\n").0, TypeRef::Primitive(Primitive::Any));
    // As a member, with its constraint.
    let Some(TypeDef::Struct(s)) = compile("r = { n: #1 }\n").get("r").cloned() else { panic!() };
    assert_eq!(s.fields[0].ty, TypeRef::Primitive(Primitive::Int));
    assert_eq!(s.fields[0].constraints, vec![Constraint::ValueRangeInt { min: None, max: Some(-1), inclusive: true }]);
}

// ── Record arrays (dgtlrift/cddlc#81) ────────────────────────────────────────

fn record(m: &IrModule, name: &str) -> (Vec<String>, cddlc_ir::StructEncoding) {
    let Some(TypeDef::Struct(s)) = m.get(name) else { panic!("{name} is not a struct") };
    (s.fields.iter().map(|f| f.name.to_string()).collect(), s.encoding)
}

#[test]
fn record_arrays_are_array_encoded_structs() {
    use cddlc_ir::StructEncoding::{Array, Map};
    let m = compile("r = [a: uint, b: tstr]\n");
    assert_eq!(record(&m, "r"), (vec!["a".into(), "b".into()], Array));

    // Members without keys are named after a rule they refer to, else by position.
    let m = compile("r = [uint, uint, hdr]\nhdr = { v: uint }\n");
    assert_eq!(record(&m, "r"), (vec!["item0".into(), "item1".into(), "hdr".into()], Array));
    assert_eq!(record(&m, "hdr").1, Map);

    // Generic and nested records too.
    let m = compile("r = pair<uint, tstr>\npair<K, V> = [K, V]\n");
    assert_eq!(record(&m, "r"), (vec!["item0".into(), "item1".into()], Array));
    let m = compile("r = { a: [uint, tstr] }\n");
    assert_eq!(record(&m, "r-a").1, Array);

    // A homogeneous array stays an array.
    let m = compile("r = [* uint]\n");
    assert!(matches!(m.get("r"), Some(TypeDef::Array(_))));
}

#[test]
fn record_optional_members_anywhere_but_no_repeats() {
    assert_eq!(record(&compile("r = [a: uint, ? b: tstr]\n"), "r").0, ["a", "b"]);
    // An optional member before required ones: the array's length decides
    // (dgtlrift/cddlc#134).
    assert_eq!(record(&compile("r = [? a: uint, b: tstr]\n"), "r").0, ["a", "b"]);
    let src = "r = [a: uint, * b: tstr]\n";
    let parsed = parse_cddl(src, PathBuf::from("test.cddl")).unwrap();
    let err = cddlc_ir::lower(&parsed.value, 16, 64).err().expect(src).to_string();
    assert!(err.contains("can't repeat a member yet"), "{src}: {err}");
}

// ── Tables (dgtlrift/cddlc#82) ────────────────────────────────────────────────

#[test]
fn computed_key_map_lowers_to_map_def() {
    let m = compile("r = { * tstr => uint }\n");
    let Some(TypeDef::Map(t)) = m.get("r") else { panic!("{:?}", m.get("r")) };
    assert_eq!(t.key_ty, TypeRef::Primitive(Primitive::Tstr));
    assert_eq!(t.val_ty, TypeRef::Primitive(Primitive::Uint));
    assert!(matches!(t.occurrence, Occurrence::ZeroOrMore { .. }));

    let m = compile("r = { + uint => tstr }\n");
    let Some(TypeDef::Map(t)) = m.get("r") else { panic!() };
    assert_eq!(t.key_ty, TypeRef::Primitive(Primitive::Uint));
    assert!(matches!(t.occurrence, Occurrence::OneOrMore { .. }));
}

#[test]
fn table_member_and_constrained_value() {
    // An inline table member is a table type of its own.
    let m = compile("r = { counts: { * tstr => uint } }\n");
    assert!(matches!(m.get("r-counts"), Some(TypeDef::Map(_))));

    // A value with control operators becomes an alias that carries them.
    let m = compile("r = { * tstr => uint .le 9 }\n");
    let Some(TypeDef::Map(t)) = m.get("r") else { panic!() };
    assert_eq!(t.val_ty, TypeRef::Named("r-value".into()));
    let Some(TypeDef::Alias(a)) = m.get("r-value") else { panic!() };
    assert!(!a.constraints.is_empty());
}

#[test]
fn mixed_map_has_rest() {
    let m = compile("r = { a: uint, ? extra: tstr, * tstr => any }\n");
    let Some(TypeDef::Struct(st)) = m.get("r") else { panic!("{:?}", m.get("r")) };
    assert_eq!(st.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["a", "extra"]);
    let rest = st.rest.as_ref().expect("rest");
    // A member called `extra` moves the other entries' name aside.
    assert_eq!(rest.name, "extra_entries");
    assert_eq!(rest.key_ty, TypeRef::Primitive(Primitive::Tstr));
    assert_eq!(rest.val_ty, TypeRef::Primitive(Primitive::Any));

    // A key alias defined later is text too.
    let m = compile("r = { a: uint, + label => uint }\nlabel = tstr\n");
    let Some(TypeDef::Struct(st)) = m.get("r") else { panic!() };
    assert!(matches!(st.rest.as_ref().unwrap().occurrence, Occurrence::OneOrMore { .. }));
}

#[test]
fn mixed_map_limits() {
    for (src, want) in [
        ("r = { a: uint, * bstr => any }\n", "other than text or integers"),
        ("r = { a: uint, * tstr => any, * int => any }\n", "more than one member with a computed key"),
    ] {
        let parsed = parse_cddl(src, PathBuf::from("test.cddl")).unwrap();
        let err = cddlc_ir::lower(&parsed.value, 16, 64).err().expect(src).to_string();
        assert!(err.contains(want), "{src}: {err}");
    }
}

// ── Groups inlined where they are used (dgtlrift/cddlc#83) ───────────────────

/// The member names and occurrences of the struct `name`.
fn members(m: &IrModule, name: &str) -> Vec<(String, bool)> {
    let Some(TypeDef::Struct(st)) = m.get(name) else { panic!("{name}: {:?}", m.get(name)) };
    st.fields.iter().map(|f| (f.name.to_string(), f.occurrence.is_required())).collect()
}

#[test]
fn groups_are_inlined() {
    let want = vec![("a".to_string(), true), ("b".to_string(), true)];
    for src in [
        "r = { g, b: tstr }\ng = ( a: uint )\n",
        "r = { ~base, b: tstr }\nbase = { a: uint }\n",
        "r = { $$g, b: tstr }\n$$g //= ( a: uint )\n",
        "r = { (a: uint), b: tstr }\n",
        "r = { g, b: tstr }\ng = ( h )\nh = ( a: uint )\n",
    ] {
        assert_eq!(members(&compile(src), "r"), want, "{src}");
    }
    // In a record array too.
    let m = compile("r = [g, b: tstr]\ng = ( a: uint )\n");
    assert_eq!(members(&m, "r"), want);

    // `? g` makes each of its members optional; an empty socket adds none.
    let m = compile("r = { ? g, b: tstr, $$none }\ng = ( a: uint, ? c: tstr )\n");
    assert_eq!(members(&m, "r"), vec![("a".into(), false), ("c".into(), false), ("b".into(), true)]);

    // A group used only by inlining isn't a type of its own.
    let m = compile("r = { g }\ng = ( a: uint )\n");
    assert!(m.get("g").is_none());
}

#[test]
fn parenthesized_type_rule_is_a_type() {
    let m = compile("t = (int / uint)\n");
    assert!(matches!(m.get("t"), Some(TypeDef::Enum(_))), "{:?}", m.get("t"));
}

#[test]
fn group_inlining_limits() {
    for (src, want) in [
        ("r = { * g }\ng = ( a: uint )\n", "repeated group in a map"),
        ("r = { g }\ng = ( a: uint, g )\n", "includes itself"),
        ("r = { ~t }\nt = uint\n", "other than a map or array"),
    ] {
        let parsed = parse_cddl(src, PathBuf::from("test.cddl")).unwrap();
        let err = cddlc_ir::lower(&parsed.value, 16, 64).err().expect(src).to_string();
        assert!(err.contains(want), "{src}: {err}");
    }
}

// ── Group choices (dgtlrift/cddlc#84) ────────────────────────────────────────

/// The member names of each alternative of the choice `name`.
fn alternatives(m: &IrModule, name: &str) -> Vec<Vec<String>> {
    let Some(TypeDef::Enum(e)) = m.get(name) else { panic!("{name}: {:?}", m.get(name)) };
    e.variants.iter().map(|v| {
        let TypeRef::Named(n) = &v.ty else { panic!("{v:?}") };
        match m.get(n) {
            Some(TypeDef::Struct(st)) => st.fields.iter().map(|f| f.name.to_string()).collect(),
            Some(TypeDef::Array(_)) => vec!["[array]".to_string()],
            other => panic!("{n}: {other:?}"),
        }
    }).collect()
}

#[test]
fn group_choices_are_choices_of_maps() {
    let m = compile("r = { a: uint // b: tstr }\n");
    assert_eq!(alternatives(&m, "r"), vec![vec!["a"], vec!["b"]]);

    // A choice inside an inlined group takes the other members along.
    let m = compile("r = { kind, id: uint }\nkind = ( k: \"x\" // k: \"y\", v: int )\n");
    assert_eq!(alternatives(&m, "r"), vec![vec!["k", "id"], vec!["k", "v", "id"]]);
    // The group, used only inlined, leaves no types behind.
    assert!(m.types.keys().all(|n| !n.starts_with("kind")), "{:?}", m.types.keys().collect::<Vec<_>>());

    // `? (a // b)`: neither, or either.
    let m = compile("r = { ? (p: int // q: tstr), z: bool }\n");
    assert_eq!(alternatives(&m, "r"), vec![vec!["z"], vec!["p", "z"], vec!["q", "z"]]);

    // In an array: a choice of records.
    let m = compile("s = [ uint, tstr // tstr ]\n");
    assert_eq!(alternatives(&m, "s").len(), 2);
}

#[test]
fn group_choices_have_a_limit() {
    // Nine two-way choices multiply out to 512 alternatives, past the 256 allowed.
    let g: String = (0..9).map(|i| format!("g{i} = ( a{i}: uint // b{i}: uint )\n")).collect();
    let src = format!("r = {{ g0, g1, g2, g3, g4, g5, g6, g7, g8 }}\n{g}");
    let parsed = parse_cddl(&src, PathBuf::from("test.cddl")).unwrap();
    let err = cddlc_ir::lower(&parsed.value, 16, 64).err().expect("too many").to_string();
    assert!(err.contains("alternatives"), "{err}");
}

// ── Enumerations (dgtlrift/cddlc#85) ─────────────────────────────────────────

/// The literal values of the choice `name`'s variants.
fn enum_literals(m: &IrModule, name: &str) -> Vec<String> {
    let Some(TypeDef::Enum(e)) = m.get(name) else { panic!("{name}: {:?}", m.get(name)) };
    e.variants.iter().map(|v| format!("{:?}", v.constraints)).collect()
}

#[test]
fn enumerations_are_choices_of_values() {
    let m = compile("r = &colors\ncolors = ( red: 1, green: 2 )\n");
    assert_eq!(enum_literals(&m, "r"), vec!["[Eq(Uint(1))]", "[Eq(Uint(2))]"]);
    // The group, used only in the enumeration, isn't a type.
    assert!(m.get("colors").is_none());

    // Inline, as a member's type, and inside an array.
    let m = compile("m = { c: &( on: \"on\", off: \"off\" ), e: [* &colors] }\ncolors = ( red: 1 // green: 2 )\n");
    assert_eq!(enum_literals(&m, "m-c"), vec!["[Eq(Text(\"on\"))]", "[Eq(Text(\"off\"))]"]);
    let Some(TypeDef::Array(a)) = m.get("m-e") else { panic!() };
    let TypeRef::Named(n) = &a.element else { panic!() };
    assert_eq!(enum_literals(&m, n).len(), 2);

    // An enumeration of a type isn't one.
    let parsed = parse_cddl("r = &t\nt = uint\n", PathBuf::from("test.cddl")).unwrap();
    let err = cddlc_ir::lower(&parsed.value, 16, 64).err().expect("type").to_string();
    assert!(err.contains("not a group"), "{err}");
}

// ── Control operators (dgtlrift/cddlc#86) ────────────────────────────────────

#[test]
fn unchecked_control_operators_warn() {
    let r = compile_warns("r = tstr .abnf \"DIGIT\"\n");
    assert!(
        r.warnings.iter().any(|w| w.kind == cddlc_ir::IrWarningKind::UncheckedControlOp && w.message.contains(".abnf")),
        "{:?}", r.warnings
    );
    let r = compile_warns("r = uint .within other\nother = uint / tstr\n");
    assert!(r.warnings.iter().any(|w| w.kind == cddlc_ir::IrWarningKind::UncheckedControlOp), "{:?}", r.warnings);
}

#[test]
fn plus_and_cat_compute_literals() {
    let m = compile("r = 1 .plus 2\ns = \"a\" .cat \"b\"\n");
    let Some(TypeDef::Alias(a)) = m.get("r") else { panic!() };
    assert_eq!(a.constraints, vec![Constraint::Eq(cddlc_ir::LiteralValue::Uint(3))]);
    let Some(TypeDef::Alias(a)) = m.get("s") else { panic!() };
    assert_eq!(a.constraints, vec![Constraint::Eq(cddlc_ir::LiteralValue::Text("ab".into()))]);
}

// ── Many alternatives, named by their literal (dgtlrift/cddlc#120) ──────────

#[test]
fn a_choice_of_many_maps_is_allowed() {
    // 40 alternatives, like WebDriver BiDi's 33 commands.
    let cmds: Vec<String> = (0..40).map(|i| format!("( method: \"m.c{i}\", params: uint )")).collect();
    let src = format!("command = {{ id: uint, cmd }}\ncmd = ( {} )\n", cmds.join(" // "));
    let m = compile(&src);
    assert_eq!(alternatives(&m, "command").len(), 40);
}

/// The variant names of the enum `name`.
fn variant_names(m: &cddlc_ir::IrModule, name: &str) -> Vec<String> {
    match m.types.get(name) {
        Some(cddlc_ir::TypeDef::Enum(e)) => e.variants.iter().map(|v| v.name.to_string()).collect(),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn alternatives_are_named_by_the_literal_that_tells_them_apart() {
    let m = compile("event = { type: \"start\" // type: \"stop\", code: int }\n");
    assert_eq!(variant_names(&m, "event"), ["start", "stop"]);
    assert!(m.types.contains_key("event-start") && m.types.contains_key("event-stop"));

    // The key needn't come first, and dots are kept (`session.end`).
    let m = compile("c = { id: uint, method: \"session.new\" // id: uint, method: \"session.end\" }\n");
    assert_eq!(variant_names(&m, "c"), ["session.new", "session.end"]);

    // Without such a key (or with a repeated literal), they are numbered.
    let m = compile("r = { a: uint // b: tstr }\n");
    assert_eq!(variant_names(&m, "r"), ["alt1", "alt2"]);
    let m = compile("r = { t: \"x\", a: uint // t: \"x\", b: tstr }\n");
    assert_eq!(variant_names(&m, "r"), ["alt1", "alt2"]);
    // A literal that isn't a name.
    let m = compile("r = { t: \"1\", a: uint // t: \"2\", b: tstr }\n");
    assert_eq!(variant_names(&m, "r"), ["alt1", "alt2"]);
}

#[test]
fn a_group_rule_can_hold_a_computed_key() {
    // WebDriver BiDi: `Extensible = (* text => any)`, included in its maps.
    let m = compile("r = { id: uint, Extensible }\nExtensible = (* text => any)\n");
    match m.types.get("r") {
        Some(cddlc_ir::TypeDef::Struct(s)) => assert!(s.rest.is_some(), "{s:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn prelude_value_names_are_literals() {
    // `bool .default false`, as in WebDriver BiDi.
    let m = compile("r = { ? p: bool .default false, ? n: tstr / null .default null }\n");
    assert!(m.types.contains_key("r"));
}

// ── RFC 9741 text encodings, .det and .feature (dgtlrift/cddlc#109) ──────────

fn module_alias_constraints(m: &IrModule, name: &str) -> Vec<cddlc_ir::Constraint> {
    match m.types.get(name) {
        Some(cddlc_ir::TypeDef::Alias(a)) => a.constraints.clone(),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn text_encodings_lower_with_their_operand() {
    use cddlc_ir::{Constraint, TextEncoding, TypeRef, Primitive};
    let r = compile_warns("r = tstr .b64u (bstr .size 32)\nn = tstr .base10 (0..9)\n");
    match module_alias_constraints(&r.module, "r").as_slice() {
        [Constraint::TextEncoded { encoding: TextEncoding::B64u, ty, constraints }] => {
            assert_eq!(**ty, TypeRef::Primitive(Primitive::Bstr));
            assert_eq!(constraints, &[Constraint::SizeExact(32)]);
        }
        other => panic!("{other:?}"),
    }
    match module_alias_constraints(&r.module, "n").as_slice() {
        [Constraint::TextEncoded { encoding: TextEncoding::Base10, ty, constraints }] => {
            assert_eq!(**ty, TypeRef::Primitive(Primitive::Uint));
            assert!(matches!(constraints.as_slice(), [Constraint::ValueRangeUint { min: Some(0), max: Some(9), .. }]), "{constraints:?}");
        }
        other => panic!("{other:?}"),
    }
    // Generated code doesn't check them: each says so.
    let msgs: Vec<&str> = r.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("'.b64u' is checked by cddlc validate, not by generated code")), "{msgs:?}");
}

#[test]
fn det_folds_and_feature_is_its_type() {
    use cddlc_ir::{Constraint, LiteralValue};
    let r = compile_warns("r = \"  a\\n\" .det \"b\"\nf = uint .feature \"x\"\n");
    assert_eq!(module_alias_constraints(&r.module, "r"), [Constraint::Eq(LiteralValue::Text("a\nb".into()))]);
    assert!(module_alias_constraints(&r.module, "f").is_empty());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn printf_and_join_warn() {
    let r = compile_warns("r = tstr .printf [\"%d\", 1]\n");
    assert!(r.warnings.iter().any(|w| w.message.contains("'.printf' isn't checked")), "{:?}", r.warnings);
}

#[test]
fn test_inline_array_and_map_in_type_choice_are_types() {
    // dgtlrift/cddlc#138: RFC 9594's `get_creds`, RFC 9321's `Extension`.
    let m = compile("g = null / [ uint, tstr ]\ne = { + text => text } / null");
    let TypeDef::Enum(g) = m.get("g").unwrap() else { panic!("not an enum") };
    assert!(matches!(&g.variants[1].ty, TypeRef::Named(n) if n == "g-alt1"));
    assert!(matches!(m.get("g-alt1"), Some(TypeDef::Struct(_))));
    let TypeDef::Enum(e) = m.get("e").unwrap() else { panic!("not an enum") };
    assert!(matches!(&e.variants[0].ty, TypeRef::Named(n) if n == "e-alt0"));
    assert!(matches!(m.get("e-alt0"), Some(TypeDef::Map(_))));
}

// ── Found by the published-schema corpus (dgtlrift/cddlc#113) ─────────────────

#[test]
fn test_nint_is_a_negative_int() {
    let m = compile("r = { a: nint }\ns = nint");
    let TypeDef::Struct(st) = m.get("r").unwrap() else { panic!("not a struct") };
    assert!(matches!(st.fields[0].ty, TypeRef::Primitive(Primitive::Int)));
    assert!(st.fields[0].constraints.iter().any(|c| matches!(c, Constraint::ValueRangeInt { max: Some(-1), .. })));
    let TypeDef::Alias(a) = m.get("s").unwrap() else { panic!("not an alias") };
    assert!(a.constraints.iter().any(|c| matches!(c, Constraint::ValueRangeInt { max: Some(-1), .. })));
}

#[test]
fn test_size_bounded_by_a_uint() {
    let m = compile("p = bytes .size (uint .le 16)\nq = bytes .size (uint .ge 2 .lt 8)");
    let TypeDef::Alias(p) = m.get("p").unwrap() else { panic!("not an alias") };
    assert!(matches!(p.constraints[..], [Constraint::SizeRange { min: Some(0), max: Some(16) }]));
    let TypeDef::Alias(q) = m.get("q").unwrap() else { panic!("not an alias") };
    assert!(matches!(q.constraints[..], [Constraint::SizeRange { min: Some(2), max: Some(7) }]));
}

#[test]
fn test_group_recursing_through_inline_maps_ends() {
    // RFC 8927's shape: a group that holds a map of itself.
    let m = compile("root = { schema }\nschema = ( elements: { schema } // type: tstr )");
    assert!(m.get("root").is_some());
}

#[test]
fn test_undefined_generic_is_undefined() {
    let parsed = parse_cddl("r = g<uint>", PathBuf::from("t.cddl")).unwrap();
    let err = lower(&parsed.value, 16, 64).err().expect("should fail").to_string();
    assert!(err.contains("undefined type 'g'"), "{err}");
}

// ── Repeated groups in arrays (dgtlrift/cddlc#90) ─────────────────────────────

#[test]
fn test_array_of_repeated_inline_group() {
    let m = compile("r = [ * ( a: uint, b: tstr ) ]");
    let TypeDef::Array(a) = m.get("r").unwrap() else { panic!("not an array") };
    assert!(a.flat);
    assert!(matches!(a.occurrence, Occurrence::ZeroOrMore { .. }));
    assert!(matches!(&a.element, TypeRef::Named(n) if n == "r-entry"));
    let TypeDef::Struct(s) = m.get("r-entry").unwrap() else { panic!("not a record") };
    assert_eq!(s.encoding, cddlc_ir::StructEncoding::Array);
    let names: Vec<_> = s.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[test]
fn test_array_of_repeated_group_rule() {
    let m = compile("r = [ + pair ]\npair = ( key: tstr, value: int )");
    let TypeDef::Array(a) = m.get("r").unwrap() else { panic!("not an array") };
    assert!(a.flat);
    assert!(matches!(a.occurrence, Occurrence::OneOrMore { .. }));
    let TypeDef::Struct(s) = m.get("r-entry").unwrap() else { panic!("not a record") };
    assert_eq!(s.fields.len(), 2);
}

#[test]
fn test_array_of_repeated_single_member_group_is_plain() {
    let m = compile("r = [ * ( a: uint ) ]");
    let TypeDef::Array(a) = m.get("r").unwrap() else { panic!("not an array") };
    assert!(!a.flat);
    assert!(matches!(a.element, TypeRef::Primitive(Primitive::Uint)));
}

#[test]
fn test_array_of_repeated_group_with_optional_member_unsupported() {
    let parsed = parse_cddl("r = [ * ( a: uint, ? b: tstr ) ]", PathBuf::from("t.cddl")).unwrap();
    let err = lower(&parsed.value, 16, 64).err().expect("should fail").to_string();
    assert!(err.contains("each repetition must have the same members"), "{err}");
}
