//! The `Cddl` runtime class emitted into every generated Java package: the
//! format-neutral value model, type-checked conversions, and the codec for the
//! selected format and flavor.

/// The value model and checks, shared by every format. `@@FORMAT@@` is the
/// format's name.
pub const COMMON: &str = r#"/**
 * Runtime support for the generated types: the format-neutral value model,
 * type-checked conversions and the @@FORMAT@@ codec.
 *
 * <p>Every generated type converts to and from a raw value: {@code null},
 * {@link Boolean}, {@link Long} or {@link BigInteger}, {@link Double} (or
 * {@link F32} when written), {@link String}, {@code byte[]}, {@link Tagged},
 * a {@code Map<String, Object>} in member order, or a {@code List<Object>}.
 */
public final class Cddl {
    private Cddl() {}

    /** A CDDL {@code nil}: CBOR or JSON null. */
    public enum Null { NULL }

    /** A tagged value in the value model. */
    public record Tagged(long tag, Object value) {}

    /** A float written at float32 width. */
    public record F32(float value) {}

    private static final BigInteger TWO_64 = BigInteger.ONE.shiftLeft(64);

    // ── Java values to raw ─────────────────────────────────────────────────────

    /** An unsigned 64-bit integer held in a long (as {@link Long#toUnsignedString}). */
    public static Object uint(long v) {
        return v >= 0 ? (Object) v : new BigInteger(Long.toUnsignedString(v));
    }

    public static Object integer(long v) { return v; }

    public static Object f32(float v) { return new F32(v); }

    public static Object f64(double v) { return v; }

    public static Object bool(boolean v) { return v; }

    // ── Raw to Java values, each type checked ──────────────────────────────────

    public static IllegalArgumentException invalid(String at, String message) {
        return new IllegalArgumentException(at + ": " + message);
    }

    public static IllegalArgumentException mismatch(String at, String want, Object v) {
        return invalid(at, "expected " + want + ", got " + kindName(v));
    }

    static String kindName(Object v) {
        if (v == null) return "null";
        if (v instanceof Boolean) return "a bool";
        if (isNint(v)) return "a negative integer";
        if (v instanceof Long || v instanceof BigInteger) return "an unsigned integer";
        if (v instanceof Double) return "a float";
        if (v instanceof String) return "text";
        if (v instanceof byte[]) return "a byte string";
        if (v instanceof Map) return "a map";
        if (v instanceof List) return "an array";
        if (v instanceof Tagged) return "a tag";
        return "an unknown value";
    }

    public static boolean isUint(Object v) {
        return (v instanceof Long l && l >= 0) || (v instanceof BigInteger b && b.signum() >= 0);
    }

    public static boolean isNint(Object v) {
        return (v instanceof Long l && l < 0) || (v instanceof BigInteger b && b.signum() < 0);
    }

    public static boolean isFloat(Object v) { return v instanceof Double; }

    public static boolean isText(Object v) { return v instanceof String; }

    public static boolean isBool(Object v) { return v instanceof Boolean; }

    public static long asUint(Object v, String at) {
        if (v instanceof Long l && l >= 0) return l;
        if (v instanceof BigInteger b && b.signum() >= 0 && b.compareTo(TWO_64) < 0) return b.longValue();
        throw mismatch(at, "an unsigned integer", v);
    }

    public static long asInt(Object v, String at) {
        if (v instanceof Long l) return l;
        throw mismatch(at, "an integer", v);
    }

    public static double asFloat64(Object v, String at) {
        if (v instanceof Double d) return d;
        if (v instanceof Long l) return l;
        if (v instanceof BigInteger b) return b.doubleValue();
        throw mismatch(at, "a number", v);
    }

    public static float asFloat32(Object v, String at) { return (float) asFloat64(v, at); }

    public static boolean asBool(Object v, String at) {
        if (v instanceof Boolean b) return b;
        throw mismatch(at, "a bool", v);
    }

    public static String asText(Object v, String at) {
        if (v instanceof String s) return s;
        throw mismatch(at, "text", v);
    }

    public static Null asNull(Object v, String at) {
        if (v != null) throw mismatch(at, "null", v);
        return Null.NULL;
    }

    @SuppressWarnings("unchecked")
    public static Map<String, Object> asMap(Object v, String at) {
        if (v instanceof Map) return (Map<String, Object>) v;
        throw mismatch(at, "a map", v);
    }

    @SuppressWarnings("unchecked")
    public static List<Object> asList(Object v, String at) {
        if (v instanceof List) return (List<Object>) v;
        throw mismatch(at, "an array", v);
    }

    /** A required member's raw value. */
    public static Object required(Map<String, Object> m, String key, String at) {
        if (!m.containsKey(key)) throw invalid(at, "missing member \"" + key + "\"");
        return m.get(key);
    }

    /** Whether an optional member is present (null counts as absent). */
    public static boolean present(Map<String, Object> m, String key) { return m.get(key) != null; }

    public static IllegalArgumentException noVariant(String at, Object v) {
        return invalid(at, "no variant matches " + kindName(v));
    }

    /** CDDL {@code .size} on text counts UTF-8 bytes. */
    public static int utf8Length(String s) { return s.getBytes(StandardCharsets.UTF_8).length; }

    // ── Equality for generated types holding byte strings ──────────────────────

    public static boolean eq(Object a, Object b) {
        if (a instanceof List<?> x && b instanceof List<?> y) {
            if (x.size() != y.size()) return false;
            for (int i = 0; i < x.size(); i++) {
                if (!eq(x.get(i), y.get(i))) return false;
            }
            return true;
        }
        return Arrays.deepEquals(new Object[] {a}, new Object[] {b});
    }

    public static int hash(Object a) {
        if (a instanceof List<?> x) {
            int h = 1;
            for (Object o : x) h = 31 * h + hash(o);
            return h;
        }
        return Arrays.deepHashCode(new Object[] {a});
    }
"#;

/// CBOR via Jackson's CBOR streaming API: tags kept raw, maps and arrays
/// written with a definite length.
pub const CBOR_JACKSON: &str = r#"
    // ── CBOR (Jackson) ─────────────────────────────────────────────────────────

    private static final CBORFactory FACTORY = CBORFactory.builder()
        .enable(StreamReadFeature.STRICT_DUPLICATE_DETECTION)
        .build();

    public static Object bytes(byte[] v) { return v; }

    public static byte[] asBytes(Object v, String at) {
        if (v instanceof byte[] b) return b;
        throw mismatch(at, "a byte string", v);
    }

    public static Object tag(long tag, Object v) { return new Tagged(tag, v); }

    public static Object untag(Object v, long tag, String at) {
        if (v instanceof Tagged t && t.tag() == tag) return t.value();
        throw invalid(at, "expected CBOR tag " + tag);
    }

    public static byte[] encode(Object raw) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        try (CBORGenerator g = FACTORY.createGenerator(out)) {
            write(g, raw);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        return out.toByteArray();
    }

    private static void write(CBORGenerator g, Object v) throws IOException {
        if (v == null) {
            g.writeNull();
        } else if (v instanceof Boolean b) {
            g.writeBoolean(b);
        } else if (v instanceof Long l) {
            g.writeNumber(l.longValue());
        } else if (v instanceof BigInteger b) {
            g.writeNumber(b);
        } else if (v instanceof F32 f) {
            g.writeNumber(f.value());
        } else if (v instanceof Double d) {
            g.writeNumber(d.doubleValue());
        } else if (v instanceof String s) {
            g.writeString(s);
        } else if (v instanceof byte[] b) {
            g.writeBinary(b);
        } else if (v instanceof Tagged t) {
            g.writeTag((int) t.tag());
            write(g, t.value());
        } else if (v instanceof Map<?, ?> m) {
            g.writeStartObject(m, m.size());
            for (Map.Entry<?, ?> e : m.entrySet()) {
                g.writeFieldName((String) e.getKey());
                write(g, e.getValue());
            }
            g.writeEndObject();
        } else if (v instanceof List<?> l) {
            g.writeStartArray(l, l.size());
            for (Object o : l) write(g, o);
            g.writeEndArray();
        } else {
            throw new IllegalArgumentException("cannot encode " + v.getClass().getName());
        }
    }

    public static Object decode(byte[] data) {
        try (CBORParser p = FACTORY.createParser(data)) {
            if (p.nextToken() == null) throw new IllegalArgumentException("empty input");
            Object v = read(p);
            if (p.nextToken() != null) throw new IllegalArgumentException("trailing data after the value");
            return v;
        } catch (IOException e) {
            throw new IllegalArgumentException("invalid CBOR: " + e.getMessage(), e);
        }
    }

    private static Object read(CBORParser p) throws IOException {
        int tag = p.getCurrentTag();
        Object v = readValue(p);
        return tag >= 0 ? new Tagged(tag, v) : v;
    }

    private static Object readValue(CBORParser p) throws IOException {
        JsonToken t = p.currentToken();
        switch (t) {
            case START_OBJECT: {
                Map<String, Object> m = new LinkedHashMap<>();
                while (p.nextToken() == JsonToken.FIELD_NAME) {
                    String k = p.currentName();
                    p.nextToken();
                    m.put(k, read(p));
                }
                return m;
            }
            case START_ARRAY: {
                List<Object> l = new ArrayList<>();
                while (p.nextToken() != JsonToken.END_ARRAY) l.add(read(p));
                return l;
            }
            case VALUE_STRING:
                return p.getText();
            case VALUE_NUMBER_INT:
                return p.getNumberType() == JsonParser.NumberType.BIG_INTEGER ? p.getBigIntegerValue() : (Object) p.getLongValue();
            case VALUE_NUMBER_FLOAT:
                return p.getDoubleValue();
            case VALUE_TRUE:
                return Boolean.TRUE;
            case VALUE_FALSE:
                return Boolean.FALSE;
            case VALUE_NULL:
                return null;
            case VALUE_EMBEDDED_OBJECT: {
                Object o = p.getEmbeddedObject();
                if (o instanceof byte[]) return o;
                throw new IllegalArgumentException("unsupported CBOR value " + o);
            }
            default:
                throw new IllegalArgumentException("unexpected CBOR token " + t);
        }
    }
}
"#;

pub const CBOR_JACKSON_IMPORTS: &[&str] = &[
    "com.fasterxml.jackson.core.JsonParser",
    "com.fasterxml.jackson.core.JsonToken",
    "com.fasterxml.jackson.core.StreamReadFeature",
    "com.fasterxml.jackson.dataformat.cbor.CBORFactory",
    "com.fasterxml.jackson.dataformat.cbor.CBORGenerator",
    "com.fasterxml.jackson.dataformat.cbor.CBORParser",
    "java.io.ByteArrayOutputStream",
    "java.io.IOException",
    "java.io.UncheckedIOException",
];

/// JSON: byte strings are base64url text without padding, and JSON has no tags.
const JSON_BYTES_AND_TAGS: &str = r#"
    public static Object bytes(byte[] v) { return Base64.getUrlEncoder().withoutPadding().encodeToString(v); }

    public static byte[] asBytes(Object v, String at) {
        if (v instanceof String s) {
            try {
                return Base64.getUrlDecoder().decode(s.replace("=", ""));
            } catch (IllegalArgumentException e) {
                throw invalid(at, "invalid base64url: " + e.getMessage());
            }
        }
        throw mismatch(at, "base64url text", v);
    }

    /** JSON has no tags: the value is written as is. */
    public static Object tag(long tag, Object v) { return v; }

    /** JSON has no tags: the value is read as is. */
    public static Object untag(Object v, long tag, String at) { return v; }
"#;

/// JSON via Jackson's streaming API (Java SE).
pub fn json_jackson() -> String {
    format!("\n    // ── JSON (Jackson) ─────────────────────────────────────────────────────────\n\n    private static final JsonFactory FACTORY = JsonFactory.builder()\n        .enable(StreamReadFeature.STRICT_DUPLICATE_DETECTION)\n        .build();\n{JSON_BYTES_AND_TAGS}{JSON_JACKSON_CODEC}")
}

const JSON_JACKSON_CODEC: &str = r#"
    public static byte[] encode(Object raw) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        try (JsonGenerator g = FACTORY.createGenerator(out)) {
            write(g, raw);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        return out.toByteArray();
    }

    private static void write(JsonGenerator g, Object v) throws IOException {
        if (v == null) {
            g.writeNull();
        } else if (v instanceof Boolean b) {
            g.writeBoolean(b);
        } else if (v instanceof Long l) {
            g.writeNumber(l.longValue());
        } else if (v instanceof BigInteger b) {
            g.writeNumber(b);
        } else if (v instanceof F32 f) {
            g.writeNumber(f.value());
        } else if (v instanceof Double d) {
            g.writeNumber(d.doubleValue());
        } else if (v instanceof String s) {
            g.writeString(s);
        } else if (v instanceof Map<?, ?> m) {
            g.writeStartObject(m, m.size());
            for (Map.Entry<?, ?> e : m.entrySet()) {
                g.writeFieldName((String) e.getKey());
                write(g, e.getValue());
            }
            g.writeEndObject();
        } else if (v instanceof List<?> l) {
            g.writeStartArray(l, l.size());
            for (Object o : l) write(g, o);
            g.writeEndArray();
        } else {
            throw new IllegalArgumentException("cannot encode " + v.getClass().getName());
        }
    }

    public static Object decode(byte[] data) {
        try (JsonParser p = FACTORY.createParser(data)) {
            if (p.nextToken() == null) throw new IllegalArgumentException("empty input");
            Object v = read(p);
            if (p.nextToken() != null) throw new IllegalArgumentException("trailing data after the value");
            return v;
        } catch (IOException e) {
            throw new IllegalArgumentException("invalid JSON: " + e.getMessage(), e);
        }
    }

    private static Object read(JsonParser p) throws IOException {
        JsonToken t = p.currentToken();
        switch (t) {
            case START_OBJECT: {
                Map<String, Object> m = new LinkedHashMap<>();
                while (p.nextToken() == JsonToken.FIELD_NAME) {
                    String k = p.currentName();
                    p.nextToken();
                    m.put(k, read(p));
                }
                return m;
            }
            case START_ARRAY: {
                List<Object> l = new ArrayList<>();
                while (p.nextToken() != JsonToken.END_ARRAY) l.add(read(p));
                return l;
            }
            case VALUE_STRING:
                return p.getText();
            case VALUE_NUMBER_INT:
                return p.getNumberType() == JsonParser.NumberType.BIG_INTEGER ? p.getBigIntegerValue() : (Object) p.getLongValue();
            case VALUE_NUMBER_FLOAT:
                return p.getDoubleValue();
            case VALUE_TRUE:
                return Boolean.TRUE;
            case VALUE_FALSE:
                return Boolean.FALSE;
            case VALUE_NULL:
                return null;
            default:
                throw new IllegalArgumentException("unexpected JSON token " + t);
        }
    }
}
"#;

pub const JSON_JACKSON_IMPORTS: &[&str] = &[
    "com.fasterxml.jackson.core.JsonFactory",
    "com.fasterxml.jackson.core.JsonGenerator",
    "com.fasterxml.jackson.core.JsonParser",
    "com.fasterxml.jackson.core.JsonToken",
    "com.fasterxml.jackson.core.StreamReadFeature",
    "java.io.ByteArrayOutputStream",
    "java.io.IOException",
    "java.io.UncheckedIOException",
    "java.util.Base64",
];

/// JSON via Jakarta JSON-P (Jakarta EE). `toJson` / `fromJson` also back the
/// generated JSON-B serializers.
pub fn json_jsonp() -> String {
    format!("\n    // ── JSON (Jakarta JSON-P) ──────────────────────────────────────────────────\n{JSON_BYTES_AND_TAGS}{JSON_JSONP_CODEC}")
}

const JSON_JSONP_CODEC: &str = r#"
    public static byte[] encode(Object raw) {
        StringWriter out = new StringWriter();
        try (JsonWriter w = Json.createWriter(out)) {
            w.write(toJson(raw));
        }
        return out.toString().getBytes(StandardCharsets.UTF_8);
    }

    public static Object decode(byte[] data) {
        // A Reader, not a stream: stream encoding detection needs four bytes ("0" has one).
        try (JsonReader r = Json.createReader(new StringReader(new String(data, StandardCharsets.UTF_8)))) {
            return fromJson(r.readValue());
        } catch (JsonException e) {
            throw new IllegalArgumentException("invalid JSON: " + e.getMessage(), e);
        }
    }

    /** A raw value as a JSON-P value. */
    public static JsonValue toJson(Object v) {
        if (v == null) return JsonValue.NULL;
        if (v instanceof Boolean b) return b ? JsonValue.TRUE : JsonValue.FALSE;
        if (v instanceof Long l) return Json.createValue(l.longValue());
        if (v instanceof BigInteger b) return Json.createValue(b);
        // The shortest decimal that reads back as the same float32.
        if (v instanceof F32 f) return Json.createValue(new BigDecimal(Float.toString(f.value())));
        if (v instanceof Double d) return Json.createValue(d.doubleValue());
        if (v instanceof String s) return Json.createValue(s);
        if (v instanceof Map<?, ?> m) {
            JsonObjectBuilder b = Json.createObjectBuilder();
            for (Map.Entry<?, ?> e : m.entrySet()) b.add((String) e.getKey(), toJson(e.getValue()));
            return b.build();
        }
        if (v instanceof List<?> l) {
            JsonArrayBuilder b = Json.createArrayBuilder();
            for (Object o : l) b.add(toJson(o));
            return b.build();
        }
        throw new IllegalArgumentException("cannot encode " + v.getClass().getName());
    }

    /** A JSON-P value as a raw value. */
    public static Object fromJson(JsonValue v) {
        switch (v.getValueType()) {
            case OBJECT: {
                Map<String, Object> m = new LinkedHashMap<>();
                for (Map.Entry<String, JsonValue> e : v.asJsonObject().entrySet()) m.put(e.getKey(), fromJson(e.getValue()));
                return m;
            }
            case ARRAY: {
                List<Object> l = new ArrayList<>();
                for (JsonValue o : v.asJsonArray()) l.add(fromJson(o));
                return l;
            }
            case STRING:
                return ((JsonString) v).getString();
            case NUMBER: {
                JsonNumber n = (JsonNumber) v;
                if (n.isIntegral()) {
                    BigInteger b = n.bigIntegerValueExact();
                    return b.bitLength() < 64 ? (Object) b.longValue() : b;
                }
                return n.doubleValue();
            }
            case TRUE:
                return Boolean.TRUE;
            case FALSE:
                return Boolean.FALSE;
            default:
                return null;
        }
    }
}
"#;

pub const JSON_JSONP_IMPORTS: &[&str] = &[
    "jakarta.json.Json",
    "jakarta.json.JsonArrayBuilder",
    "jakarta.json.JsonException",
    "jakarta.json.JsonNumber",
    "jakarta.json.JsonObjectBuilder",
    "jakarta.json.JsonReader",
    "jakarta.json.JsonString",
    "jakarta.json.JsonValue",
    "jakarta.json.JsonWriter",
    "java.io.StringReader",
    "java.io.StringWriter",
    "java.math.BigDecimal",
    "java.util.Base64",
];

pub const COMMON_IMPORTS: &[&str] = &[
    "java.math.BigInteger",
    "java.nio.charset.StandardCharsets",
    "java.util.ArrayList",
    "java.util.Arrays",
    "java.util.LinkedHashMap",
    "java.util.List",
    "java.util.Map",
];
