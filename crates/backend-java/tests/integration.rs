//! Integration tests for backend-java: the shape of the generated project. The
//! generated code is built and its tests run by backend-buildtest
//! (`test_build_java`, `_json`, `_ee`, `_ee_json`).

use std::path::PathBuf;

use backend_java::{java_member_name, java_package_name, java_type_name, JavaBackend};
use cddlc_codegen::{Backend, CodegenOptions, Format, JavaFlavor};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

struct Project {
    files: Vec<(String, String)>,
}

impl Project {
    fn file(&self, suffix: &str) -> &str {
        &self.files.iter().find(|(p, _)| p.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix} in {:?}", self.paths())).1
    }
    fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|(p, _)| p.as_str()).collect()
    }
}

fn gen(src: &str, format: Format, flavor: JavaFlavor) -> Project {
    let parsed  = parse_cddl(src, PathBuf::from("my-schema.cddl")).expect("parse");
    let lowered = lower(&parsed.value, 16, 64).expect("lower");
    let opts    = CodegenOptions { format, java_flavor: flavor, ..Default::default() };
    let out     = JavaBackend.generate(&lowered.module, &opts).expect("generate");
    Project { files: out.files.into_iter().map(|f| (f.path, f.content)).collect() }
}

fn se(src: &str) -> Project { gen(src, Format::Cbor, JavaFlavor::Se) }

#[test]
fn test_project_layout() {
    let p = se("sensor = { id: uint }\nstatus = \"ok\" / \"warn\"");
    assert_eq!(p.paths(), [
        "my-schema-cbor/pom.xml",
        "my-schema-cbor/src/main/java/my_schema/Cddl.java",
        "my-schema-cbor/src/main/java/my_schema/Sensor.java",
        "my-schema-cbor/src/main/java/my_schema/Status.java",
        "my-schema-cbor/src/test/java/my_schema/RoundtripTest.java",
    ]);
    assert!(p.file("Sensor.java").contains("package my_schema;\n"));
}

#[test]
fn test_names() {
    assert_eq!(java_package_name("iot_sensor", None), "iot_sensor");
    assert_eq!(java_package_name("simple-values", None), "simple_values");
    assert_eq!(java_package_name("x", Some("com.Example.class")), "com.example.class_");
    assert_eq!(java_type_name("sensor-reading"), "SensorReading");
    assert_eq!(java_type_name("string"), "String_");
    assert_eq!(java_member_name("sensor-id"), "sensorId");
    assert_eq!(java_member_name("class"), "class_");
    assert_eq!(java_member_name("encode"), "encode_");
}

#[test]
fn test_pom_dependencies_by_format_and_flavor() {
    let cbor = se("x = uint");
    let pom = cbor.file("pom.xml");
    assert!(pom.contains("<maven.compiler.release>17</maven.compiler.release>"), "{pom}");
    assert!(pom.contains("<artifactId>jackson-dataformat-cbor</artifactId>\n      <version>2.22.4</version>"), "{pom}");
    assert!(pom.contains("<artifactId>junit-jupiter</artifactId>"), "{pom}");
    assert!(!pom.contains("jakarta"), "{pom}");

    let json = gen("x = uint", Format::Json, JavaFlavor::Se);
    assert!(json.file("pom.xml").contains("<artifactId>jackson-core</artifactId>"));

    let ee = gen("x = uint", Format::Json, JavaFlavor::Ee);
    let pom = ee.file("pom.xml");
    assert!(pom.contains("<artifactId>jakarta.json.bind-api</artifactId>\n      <version>3.0.4</version>\n      <scope>provided</scope>"), "{pom}");
    assert!(pom.contains("<artifactId>jakarta.validation-api</artifactId>"), "{pom}");
    assert!(pom.contains("<artifactId>hibernate-validator</artifactId>\n      <version>9.1.6.Final</version>\n      <scope>test</scope>"), "{pom}");
    assert!(pom.contains("<artifactId>yasson</artifactId>"), "{pom}");
    assert!(!pom.contains("jackson"), "{pom}");
}

#[test]
fn test_struct_is_a_record_with_checked_members() {
    let p = se("reading = { sensor-id: uint, value: float32, ? unit: tstr .size 4, raw: bstr }");
    let r = p.file("Reading.java");
    assert!(r.contains("public record Reading(\n        long sensorId,\n        float value,\n        String unit,\n        byte[] raw) {"), "{r}");
    assert!(r.contains("if (this.raw == null) throw Cddl.invalid(\"Reading.raw\", \"missing\");"), "{r}");
    assert!(r.contains("if (this.unit != null) {\n            if (!(Cddl.utf8Length(this.unit) == 4)) throw Cddl.invalid(\"Reading.unit\", \"length must be 4\");"), "{r}");
    // Members in schema order; an absent optional member is left out.
    assert!(r.contains("m.put(\"sensor-id\", Cddl.uint(this.sensorId));\n        m.put(\"value\", Cddl.f32(this.value));\n        if (this.unit != null) m.put(\"unit\", this.unit);\n        m.put(\"raw\", Cddl.bytes(this.raw));"), "{r}");
    assert!(r.contains("long f_sensorId = Cddl.asUint(Cddl.required(m, \"sensor-id\", \"Reading\"), \"Reading.sensor-id\");"), "{r}");
    assert!(r.contains("String f_unit = Cddl.present(m, \"unit\") ? Cddl.asText(m.get(\"unit\"), \"Reading.unit\") : null;"), "{r}");
    // byte[] members compare by content.
    assert!(r.contains("return o instanceof Reading that && Cddl.eq(this.sensorId, that.sensorId)"), "{r}");
    assert!(r.contains("public byte[] encode() {") && r.contains("public static Reading decode(byte[] data) {"), "{r}");
}

#[test]
fn test_literal_enum_is_a_java_enum() {
    let p = se("status = \"ok\" / \"warn\"\nprio = 1 / -2");
    let s = p.file("Status.java");
    assert!(s.contains("public enum Status {\n    OK(\"Ok\", \"ok\"),\n    WARN(\"Warn\", \"warn\");"), "{s}");
    assert!(p.file("Prio.java").contains("VARIANT1(\"Variant1\", 1L),\n    VARIANT2(\"Variant2\", -2L);"), "{}", p.file("Prio.java"));
    assert!(p.file("RoundtripTest.java").contains("assertThrows(IllegalArgumentException.class, () -> Prio.decode(new byte[] {0}));"));
}

#[test]
fn test_valued_enum_is_a_sealed_interface() {
    let p = se("level = uint / \"off\" / sensor\nsensor = { id: uint }");
    let l = p.file("Level.java");
    assert!(l.contains("public sealed interface Level {"), "{l}");
    assert!(l.contains("record Uint(long value) implements Level {"), "{l}");
    assert!(l.contains("record Off() implements Level {"), "{l}");
    // A named payload is package-qualified: a variant record may share its name.
    assert!(l.contains("record Sensor(my_schema.Sensor value) implements Level {"), "{l}");
    assert!(l.contains("if (s.equals(\"off\")) return new Off();"), "{l}");
    assert!(l.contains("if (Cddl.isUint(v)) {\n            long u = Cddl.asUint(v, \"Level\");\n            return new Uint(u);"), "{l}");
    assert!(l.contains("return new Sensor(my_schema.Sensor.fromRaw(v));"), "{l}");
}

#[test]
fn test_tags_written_and_checked() {
    let p = se("stamp = #6.1(uint)\nmsg = { at: #6.1(uint) }");
    assert!(p.file("Stamp.java").contains("return Cddl.tag(1L, Cddl.uint(this.value));"));
    assert!(p.file("Stamp.java").contains("new Stamp(Cddl.asUint(Cddl.untag(raw, 1L, \"Stamp\"), \"Stamp\"))"));
    assert!(p.file("Msg.java").contains("m.put(\"at\", Cddl.tag(1L, Cddl.uint(this.at)));"));
    // JSON has no tags: the runtime passes the value through.
    let j = gen("stamp = #6.1(uint)", Format::Json, JavaFlavor::Se);
    assert!(j.file("Cddl.java").contains("public static Object tag(long tag, Object v) { return v; }"));
}

#[test]
fn test_uint_is_unsigned() {
    let p = se("small = uint .le 10");
    assert!(p.file("Cddl.java").contains("return v >= 0 ? (Object) v : new BigInteger(Long.toUnsignedString(v));"));
    assert!(p.file("Cddl.java").contains("if (v instanceof BigInteger b && b.signum() >= 0 && b.compareTo(TWO_64) < 0) return b.longValue();"));
    assert!(p.file("Small.java").contains("if (!(Long.compareUnsigned(this.value, 10L) <= 0))"), "{}", p.file("Small.java"));
}

#[test]
fn test_ee_annotations() {
    let p = gen("dev = { name: tstr .size 16, temp: int .ge -40 .le 85, on: true, ? note: tstr, sub: sub }\nsub = { id: uint }", Format::Cbor, JavaFlavor::Ee);
    let d = p.file("Dev.java");
    assert!(d.contains("@jakarta.validation.constraints.NotNull @jakarta.validation.constraints.Size(min = 16, max = 16) String name,"), "{d}");
    assert!(d.contains("@jakarta.validation.constraints.Min(-40L) @jakarta.validation.constraints.Max(85L) long temp,"), "{d}");
    assert!(d.contains("@jakarta.validation.constraints.AssertTrue boolean on,"), "{d}");
    assert!(d.contains("        String note,"), "{d}");
    assert!(d.contains("@jakarta.validation.constraints.NotNull @jakarta.validation.Valid Sub sub)"), "{d}");
    // The hand-written checks stay: encode/decode don't depend on a validator.
    assert!(d.contains("public void validate() {"), "{d}");
    let t = p.file("RoundtripTest.java");
    assert!(t.contains("VALIDATOR.validate(original)"), "{t}");
}

#[test]
fn test_ee_json_uses_jsonp_and_jsonb() {
    let p = gen("sensor = { id: uint }\nstatus = \"ok\" / \"warn\"", Format::Json, JavaFlavor::Ee);
    let rt = p.file("Cddl.java");
    assert!(rt.contains("import jakarta.json.Json;") && rt.contains("public static JsonValue toJson(Object v) {"), "{rt}");
    let s = p.file("Sensor.java");
    assert!(s.contains("@jakarta.json.bind.annotation.JsonbTypeSerializer(Sensor.JsonbCodec.class)"), "{s}");
    assert!(s.contains("public static final class JsonbCodec implements jakarta.json.bind.serializer.JsonbSerializer<Sensor>"), "{s}");
    // Yasson ignores a deserializer on an enum, so enums use an adapter.
    let st = p.file("Status.java");
    assert!(st.contains("@jakarta.json.bind.annotation.JsonbTypeAdapter(Status.JsonbAdapter.class)"), "{st}");
    assert!(p.file("RoundtripTest.java").contains("assertEquals(new String(encoded, StandardCharsets.UTF_8), json);"));
}
