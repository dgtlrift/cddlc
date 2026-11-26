/// Go backend for cddlc.
///
/// Emits a Go module:
///   <module>-cbor/
///     go.mod               ← module + github.com/fxamacker/cbor/v2 (CBOR only)
///     <pkg>.go             ← types + MarshalCBOR/UnmarshalCBOR (or the JSON pair)
///     <pkg>_test.go        ← `go test` roundtrip tests
///
/// Every type implements the format's marshaler interfaces, so it nests in other
/// Go types and works with `cbor.Marshal` / `json.Marshal` directly. Maps are
/// written in schema order (not Go's map order), every decoded value has its
/// CBOR/JSON type checked, and CDDL constraints are checked on encode and decode.

use cddlc_codegen::{
    to_pascal_case, Backend, CodegenError, CodegenOptions, CodegenOutput, GeneratedFile,
    IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, FieldKey, IrModule, LiteralValue, Occurrence,
    MapDef, Primitive, StructDef, TypeDef, TypeRef,
};

/// The fxamacker/cbor release generated CBOR modules require: v2.9.0 needs Go
/// 1.20, so a distribution's Go builds them (Ubuntu 24.04 has 1.22); v2.10.0
/// needs 1.24.
pub const FXAMACKER_CBOR_VERSION: &str = "v2.9.0";

pub struct GoBackend;

impl Backend for GoBackend {
    fn language_name(&self) -> &str { "Go" }
    fn file_extension(&self) -> &str { "go" }

    fn generate(&self, module: &IrModule, opts: &CodegenOptions) -> Result<CodegenOutput, CodegenError> {
        cddlc_codegen::check_rest_keys(module, opts, "Go", true)?;
        // Members and variants held through a pointer (dgtlrift/cddlc#221).
        POINTERS.with(|p| *p.borrow_mut() = pointer_items(module));
        let name = module.name.as_str();
        let dir  = format!("{name}-{}", fmt_tag(opts));
        let pkg  = go_package_name(name, opts.namespace.as_deref());
        Ok(CodegenOutput {
            files: vec![
                GeneratedFile { path: format!("{dir}/go.mod"),        content: emit_go_mod(&dir, opts) },
                GeneratedFile { path: format!("{dir}/{pkg}.go"),      content: emit_source(module, &pkg, opts) },
                GeneratedFile { path: format!("{dir}/{pkg}_test.go"), content: emit_tests(module, &pkg, opts) },
            ],
        })
    }
}


/// A map member's key in the generated code: its text; an integer key as a
/// `uint64` / `int64` (as fxamacker/cbor decodes it), or in JSON as its
/// decimal text (dgtlrift/cddlc#176).
fn go_key(key: &FieldKey, opts: &CodegenOptions) -> String {
    match key {
        FieldKey::Text(t) => go_str(t),
        _ if opts.is_json() => go_str(&cddlc_codegen::field_key_json(key)),
        FieldKey::Uint(n) => format!("uint64({n})"),
        FieldKey::Int(n) if *n >= 0 => format!("uint64({n})"),
        FieldKey::Int(n) => format!("int64({n})"),
    }
}

/// Exported names the generated package's runtime defines (`type Null
/// struct{}`, the CDDL nil). A rule whose type would take one gets a trailing
/// `_` instead, which no rule's name can produce (dgtlrift/cddlc#149).
const RESERVED_TYPES: &[&str] = &["Null"];

/// The Go type name of a rule.
pub fn go_type_name(name: &str) -> String {
    let n = to_pascal_case(name);
    if RESERVED_TYPES.contains(&n.as_str()) { format!("{n}_") } else { n }
}

fn fmt_tag(opts: &CodegenOptions) -> &'static str {
    if opts.is_json() { "json" } else { "cbor" }
}

/// The Go package name for a module: lowercase letters and digits only
/// (`iot_sensor` → `iotsensor`), or `--namespace` when given.
pub fn go_package_name(module: &str, namespace: Option<&str>) -> String {
    let src = namespace.unwrap_or(module);
    let mut s: String = src.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, 'p');
    }
    s
}

/// The Go name of a struct member: PascalCase, renamed when it would clash
/// with a method every generated type has.
pub fn go_field_name(name: &str) -> String {
    let n = to_pascal_case(name);
    match n.as_str() {
        "Encode" | "Validate" | "MarshalCBOR" | "UnmarshalCBOR" | "MarshalJSON" | "UnmarshalJSON" => format!("{n}_"),
        _ => n,
    }
}

/// The Go name of an enum's kind constant for a variant (`StatusKindOk`).
pub fn go_kind_const(enum_name: &str, variant: &str) -> String {
    format!("{}Kind{}", to_pascal_case(enum_name), to_pascal_case(variant))
}

/// A Go interpreted string literal.
pub fn go_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A Go byte slice literal (`[]byte{0xca, 0xfe}`).
fn go_bytes_lit(b: &[u8]) -> String {
    format!("[]byte{{{}}}", b.iter().map(|x| format!("0x{x:02x}")).collect::<Vec<_>>().join(", "))
}

/// A Go string literal holding these bytes (`"\x01\x02"`).
fn go_bytes_str(b: &[u8]) -> String {
    let mut out = String::from("\"");
    for x in b {
        out.push_str(&format!("\\x{x:02x}"));
    }
    out.push('"');
    out
}

/// Go is tab-indented: turn each 4-space indent step into a tab. The file ends
/// in exactly one newline, as gofmt leaves it.
fn tabify(s: String) -> String {
    let s = format!("{}\n", s.trim_end());
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let spaces = line.len() - line.trim_start_matches(' ').len();
        out.push_str(&"\t".repeat(spaces / 4));
        out.push_str(&line[spaces - spaces % 4..]);
    }
    out
}

fn header(w: &mut IndentWriter, opts: &CodegenOptions) {
    // The form Go tools recognise as generated code.
    w.line("// Code generated by cddlc. DO NOT EDIT.");
    w.blank();
    for l in cddlc_codegen::header_comment(opts, cddlc_codegen::CommentStyle::Slash) {
        w.line(&l);
    }
}

// ── go.mod ────────────────────────────────────────────────────────────────────

fn emit_go_mod(dir: &str, opts: &CodegenOptions) -> String {
    let mut s = String::new();
    s.push_str(&cddlc_codegen::header_text(opts, cddlc_codegen::CommentStyle::Slash));
    s.push_str("// Run `go mod tidy` once to fetch the dependencies and write go.sum.\n");
    s.push_str(&format!("module {dir}\n\ngo 1.21\n"));
    if !opts.is_json() {
        s.push_str(&format!("\nrequire github.com/fxamacker/cbor/v2 {FXAMACKER_CBOR_VERSION}\n"));
    }
    s
}

// ── Runtime helpers emitted into every package ────────────────────────────────

/// Helpers shared by both formats: value kinds, the ordered map encoder, and
/// type-checked primitive decoders.
const COMMON_HELPERS: &str = r#"// Null is a CDDL nil: CBOR or JSON null.
type Null struct{}

// rawKind is the type of an encoded value, read from its first byte.
type rawKind int

const (
	kindOther rawKind = iota
	kindUint
	kindNint
	kindBytes
	kindText
	kindArray
	kindMap
	kindTag
	kindFloat
	kindBool
	kindNull
)

var kindNames = [...]string{"an unknown value", "an unsigned integer", "a negative integer", "a byte string", "text", "an array", "a map", "a tag", "a float", "a bool", "null"}

func mismatch(at, want string, r []byte) error {
	return fmt.Errorf("%s: expected %s, got %s", at, want, kindNames[kindOf(r)])
}

// mapEncoder collects a map's members in schema order.
type mapEncoder struct {
	keys []mapKey
	vals [][]byte
	err  error
}

func (e *mapEncoder) put(key mapKey) func([]byte, error) {
	return func(b []byte, err error) {
		if e.err != nil {
			return
		}
		if err != nil {
			e.err = fmt.Errorf("%v: %w", key, err)
			return
		}
		e.keys = append(e.keys, key)
		e.vals = append(e.vals, b)
	}
}

func (e *mapEncoder) finish() ([]byte, error) {
	if e.err != nil {
		return nil, e.err
	}
	return encodeMap(e.keys, e.vals)
}

// finishArray writes the members' values as an array, in order: a record.
func (e *mapEncoder) finishArray() ([]byte, error) {
	if e.err != nil {
		return nil, e.err
	}
	return encodeArray(e.vals)
}

func encodeEach[T any](items []T, f func(T) ([]byte, error)) ([]byte, error) {
	out := make([][]byte, 0, len(items))
	for i, it := range items {
		b, err := f(it)
		if err != nil {
			return nil, fmt.Errorf("[%d]: %w", i, err)
		}
		out = append(out, b)
	}
	return encodeArray(out)
}

// encodeFlat writes the members of each item's array inline, in one array:
// an array of a repeated group (`[* (a: uint, b: tstr)]`).
func encodeFlat[T any](items []T, f func(T) ([]byte, error)) ([]byte, error) {
	out := make([][]byte, 0, len(items))
	for i, it := range items {
		b, err := f(it)
		if err != nil {
			return nil, fmt.Errorf("[%d]: %w", i, err)
		}
		members, err := decodeArray(b, fmt.Sprintf("[%d]", i))
		if err != nil {
			return nil, err
		}
		for _, m := range members {
			out = append(out, []byte(m))
		}
	}
	return encodeArray(out)
}

// decodeFlat reads an array of a repeated group of n members: one array of
// the members of each group, in turn.
func decodeFlat(r []byte, n int, at string) ([][]byte, error) {
	items, err := decodeArray(r, at)
	if err != nil {
		return nil, err
	}
	if len(items)%n != 0 {
		return nil, fmt.Errorf("%s: a partial group", at)
	}
	out := make([][]byte, 0, len(items)/n)
	for i := 0; i < len(items); i += n {
		group := make([][]byte, 0, n)
		for _, m := range items[i : i+n] {
			group = append(group, []byte(m))
		}
		b, err := encodeArray(group)
		if err != nil {
			return nil, err
		}
		out = append(out, b)
	}
	return out, nil
}

func decodeMap(r []byte, at string) (map[mapKey]raw, error) {
	if kindOf(r) != kindMap {
		return nil, mismatch(at, "a map", r)
	}
	var m map[mapKey]raw
	if err := unmarshal(r, &m); err != nil {
		return nil, fmt.Errorf("%s: %w", at, err)
	}
	return m, nil
}

func decodeArray(r []byte, at string) ([]raw, error) {
	if kindOf(r) != kindArray {
		return nil, mismatch(at, "an array", r)
	}
	var items []raw
	if err := unmarshal(r, &items); err != nil {
		return nil, fmt.Errorf("%s: %w", at, err)
	}
	return items, nil
}

func decInto(r []byte, at string, v any) error {
	if err := unmarshal(r, v); err != nil {
		return fmt.Errorf("%s: %w", at, err)
	}
	return nil
}

func decUint(r []byte, at string) (uint64, error) {
	if kindOf(r) != kindUint {
		return 0, mismatch(at, "an unsigned integer", r)
	}
	var v uint64
	return v, decInto(r, at, &v)
}

func decInt(r []byte, at string) (int64, error) {
	if k := kindOf(r); k != kindUint && k != kindNint {
		return 0, mismatch(at, "an integer", r)
	}
	var v int64
	return v, decInto(r, at, &v)
}

func decFloat64(r []byte, at string) (float64, error) {
	switch kindOf(r) {
	case kindFloat:
		var v float64
		return v, decInto(r, at, &v)
	case kindUint:
		u, err := decUint(r, at)
		return float64(u), err
	case kindNint:
		i, err := decInt(r, at)
		return float64(i), err
	}
	return 0, mismatch(at, "a number", r)
}

// decChoice reads a choice of primitive types: the first alternative that
// accepts r, as that alternative's Go type.
func decChoice(r []byte, at string, alts ...func([]byte, string) (any, error)) (any, error) {
	for _, alt := range alts {
		if v, err := alt(r, at); err == nil {
			return v, nil
		}
	}
	return nil, fmt.Errorf("%s: no alternative of the choice matches %s", at, kindNames[kindOf(r)])
}

// asAny adapts a typed decoder to decChoice.
func asAny[T any](dec func([]byte, string) (T, error)) func([]byte, string) (any, error) {
	return func(r []byte, at string) (any, error) { return dec(r, at) }
}

func decFloat32(r []byte, at string) (float32, error) {
	f, err := decFloat64(r, at)
	return float32(f), err
}

func decBool(r []byte, at string) (bool, error) {
	if kindOf(r) != kindBool {
		return false, mismatch(at, "a bool", r)
	}
	var v bool
	return v, decInto(r, at, &v)
}

func decText(r []byte, at string) (string, error) {
	if kindOf(r) != kindText {
		return "", mismatch(at, "text", r)
	}
	var v string
	return v, decInto(r, at, &v)
}

func decNull(r []byte, at string) (Null, error) {
	if kindOf(r) != kindNull {
		return Null{}, mismatch(at, "null", r)
	}
	return Null{}, nil
}

func isNull(r []byte) bool { return kindOf(r) == kindNull }"#;

/// CBOR (fxamacker/cbor): maps and arrays are written by hand so members keep
/// schema order; tags are kept raw.
const CBOR_HELPERS: &str = r#"type raw = cbor.RawMessage

// mapKey is a map member's key: text, or an integer key as uint64 / int64
// (dgtlrift/cddlc#176).
type mapKey = any

// keyText is a key of a map's other entries, which is text.
func keyText(k mapKey, at string) (string, error) {
	s, ok := k.(string)
	if !ok {
		return "", fmt.Errorf("%s: unknown member %v", at, k)
	}
	return s, nil
}

var encMode = func() cbor.EncMode {
	m, err := cbor.EncOptions{NilContainers: cbor.NilContainerAsEmpty}.EncMode()
	if err != nil {
		panic(err)
	}
	return m
}()

var decMode = func() cbor.DecMode {
	m, err := cbor.DecOptions{DupMapKey: cbor.DupMapKeyEnforcedAPF}.DecMode()
	if err != nil {
		panic(err)
	}
	return m
}()

func marshal(v any) ([]byte, error) { return encMode.Marshal(v) }

func unmarshal(r []byte, v any) error { return decMode.Unmarshal(r, v) }

// head is a CBOR item head: major type and argument.
func head(major byte, n uint64) []byte {
	m := major << 5
	switch {
	case n < 24:
		return []byte{m | byte(n)}
	case n <= 0xff:
		return []byte{m | 24, byte(n)}
	case n <= 0xffff:
		return []byte{m | 25, byte(n >> 8), byte(n)}
	case n <= 0xffffffff:
		return []byte{m | 26, byte(n >> 24), byte(n >> 16), byte(n >> 8), byte(n)}
	}
	b := []byte{m | 27}
	for s := 56; s >= 0; s -= 8 {
		b = append(b, byte(n>>uint(s)))
	}
	return b
}

func encodeMap(keys []mapKey, vals [][]byte) ([]byte, error) {
	out := head(5, uint64(len(keys)))
	for i, k := range keys {
		kb, err := marshal(k)
		if err != nil {
			return nil, err
		}
		out = append(out, kb...)
		out = append(out, vals[i]...)
	}
	return out, nil
}

func encodeArray(items [][]byte) ([]byte, error) {
	out := head(4, uint64(len(items)))
	for _, it := range items {
		out = append(out, it...)
	}
	return out, nil
}

// tagWith wraps an encoded value in CBOR tag n.
func tagWith(n uint64) func([]byte, error) ([]byte, error) {
	return func(content []byte, err error) ([]byte, error) {
		if err != nil {
			return nil, err
		}
		return marshal(cbor.RawTag{Number: n, Content: content})
	}
}

// untag returns the content of CBOR tag n, or an error if r is not that tag.
func untag(r []byte, n uint64, at string) ([]byte, error) {
	var t cbor.RawTag
	if kindOf(r) != kindTag || unmarshal(r, &t) != nil || t.Number != n {
		return nil, fmt.Errorf("%s: expected CBOR tag %d", at, n)
	}
	return t.Content, nil
}

func encBytes(b []byte) ([]byte, error) {
	if b == nil {
		b = []byte{}
	}
	return marshal(b)
}

func encNull() ([]byte, error) { return []byte{0xf6}, nil }

func decBytes(r []byte, at string) ([]byte, error) {
	if kindOf(r) != kindBytes {
		return nil, mismatch(at, "a byte string", r)
	}
	var b []byte
	if err := decInto(r, at, &b); err != nil {
		return nil, err
	}
	if b == nil {
		b = []byte{}
	}
	return b, nil
}

func kindOf(r []byte) rawKind {
	if len(r) == 0 {
		return kindOther
	}
	b := r[0]
	switch b >> 5 {
	case 0:
		return kindUint
	case 1:
		return kindNint
	case 2:
		return kindBytes
	case 3:
		return kindText
	case 4:
		return kindArray
	case 5:
		return kindMap
	case 6:
		return kindTag
	}
	switch b {
	case 0xf4, 0xf5:
		return kindBool
	case 0xf6, 0xf7:
		return kindNull
	case 0xf9, 0xfa, 0xfb:
		return kindFloat
	}
	return kindOther
}"#;

/// JSON (encoding/json): byte strings are base64url text without padding, and
/// JSON has no tags.
const JSON_HELPERS: &str = r#"type raw = json.RawMessage

// mapKey is a map member's key; a JSON object's keys are text, an integer
// key as its decimal text (dgtlrift/cddlc#176).
type mapKey = string

// keyText is a key of a map's other entries.
func keyText(k mapKey, at string) (string, error) { return k, nil }

func marshal(v any) ([]byte, error) { return json.Marshal(v) }

func unmarshal(r []byte, v any) error { return json.Unmarshal(r, v) }

func encodeMap(keys []mapKey, vals [][]byte) ([]byte, error) {
	var buf bytes.Buffer
	buf.WriteByte('{')
	for i, k := range keys {
		if i > 0 {
			buf.WriteByte(',')
		}
		kb, err := json.Marshal(k)
		if err != nil {
			return nil, err
		}
		buf.Write(kb)
		buf.WriteByte(':')
		buf.Write(vals[i])
	}
	buf.WriteByte('}')
	return buf.Bytes(), nil
}

func encodeArray(items [][]byte) ([]byte, error) {
	var buf bytes.Buffer
	buf.WriteByte('[')
	for i, it := range items {
		if i > 0 {
			buf.WriteByte(',')
		}
		buf.Write(it)
	}
	buf.WriteByte(']')
	return buf.Bytes(), nil
}

// tagWith passes the value through: JSON has no tags.
func tagWith(n uint64) func([]byte, error) ([]byte, error) {
	return func(content []byte, err error) ([]byte, error) { return content, err }
}

// untag passes the value through: JSON has no tags.
func untag(r []byte, n uint64, at string) ([]byte, error) { return r, nil }

func encBytes(b []byte) ([]byte, error) {
	return json.Marshal(base64.RawURLEncoding.EncodeToString(b))
}

func encNull() ([]byte, error) { return []byte("null"), nil }

func decBytes(r []byte, at string) ([]byte, error) {
	s, err := decText(r, at)
	if err != nil {
		return nil, mismatch(at, "base64url text", r)
	}
	b, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(s, "="))
	if err != nil {
		return nil, fmt.Errorf("%s: %w", at, err)
	}
	return b, nil
}

func kindOf(r []byte) rawKind {
	s := bytes.TrimSpace(r)
	if len(s) == 0 {
		return kindOther
	}
	switch c := s[0]; {
	case c == '"':
		return kindText
	case c == '{':
		return kindMap
	case c == '[':
		return kindArray
	case c == 't' || c == 'f':
		return kindBool
	case c == 'n':
		return kindNull
	case c == '-' || (c >= '0' && c <= '9'):
		if bytes.ContainsAny(s, ".eE") {
			return kindFloat
		}
		if c == '-' {
			return kindNint
		}
		return kindUint
	}
	return kindOther
}"#;

// ── Source file ───────────────────────────────────────────────────────────────

fn emit_source(module: &IrModule, pkg: &str, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    header(&mut w, opts);
    let fmt_label = if opts.is_json() { "JSON (RFC 8259)" } else { "CBOR (RFC 8949)" };
    w.line(&format!("// Format: {fmt_label}"));
    if opts.dcbor { w.line("// Encoding: deterministic (dCBOR)"); }
    w.blank();
    w.line(&format!("// Package {pkg} encodes and decodes the types of the {} CDDL schema.", module.name));
    w.line(&format!("package {pkg}"));
    w.blank();
    // Tables (`{ * tstr => uint }`) bring their own helpers.
    let tables = module.types.values().any(|d| matches!(d, TypeDef::Map(_)));
    // Maps with other entries write them in key order.
    let rests = module.types.values().any(|d| matches!(d, TypeDef::Struct(s) if s.rest.is_some()));
    let typed_rests = module.types.values().any(|d| matches!(d, TypeDef::Struct(s) if s.rest.as_ref().is_some_and(|r| !text_rest(r))));
    w.line("import (");
    if opts.is_json() {
        let mut imps = vec!["\"bytes\"", "\"encoding/base64\"", "\"encoding/json\"", "\"fmt\""];
        if tables { imps.push("\"slices\""); }
        if rests { imps.push("\"sort\""); }
        imps.push("\"strings\"");
        for imp in imps {
            w.line(&format!("\t{imp}"));
        }
    } else {
        if tables || typed_rests { w.line("\t\"bytes\""); }
        w.line("\t\"fmt\"");
        if tables { w.line("\t\"slices\""); }
        if rests { w.line("\t\"sort\""); }
        w.blank();
        w.line("\t\"github.com/fxamacker/cbor/v2\"");
    }
    w.line(")");
    w.blank();

    for (_, def) in &module.types {
        emit_type(&mut w, def, module, opts);
        w.blank();
    }

    w.line("// ── Format helpers ──────────────────────────────────────────────────────────");
    w.blank();
    let helpers = if opts.is_json() { JSON_HELPERS } else { CBOR_HELPERS };
    for line in helpers.lines().chain([""]).chain(COMMON_HELPERS.lines()) {
        if line.is_empty() { w.blank() } else { w.line(line) }
    }
    if rests {
        for line in [""].into_iter().chain(SORTED_KEYS_HELPER.lines()) {
            if line.is_empty() { w.blank() } else { w.line(line) }
        }
    }
    if typed_rests {
        for line in [""].into_iter().chain(ENCODED_KEY_ORDER_HELPER.lines()) {
            if line.is_empty() { w.blank() } else { w.line(line) }
        }
    }
    if tables {
        let entries = if opts.is_json() { JSON_TABLE_HELPERS } else { CBOR_TABLE_HELPERS };
        for line in [""].into_iter().chain(TABLE_HELPERS.lines()).chain([""]).chain(entries.lines()) {
            if line.is_empty() { w.blank() } else { w.line(line) }
        }
    }
    tabify(w.finish())
}

fn emit_type(w: &mut IndentWriter, def: &TypeDef, module: &IrModule, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct(w, s, module, opts),
        TypeDef::Enum(e)   => emit_enum(w, e, opts),
        TypeDef::Array(a)  => emit_array(w, a, flat_width(a, module), opts),
        TypeDef::Alias(a)  => emit_alias(w, a, opts),
        TypeDef::Map(m)    => emit_map(w, m, opts),
    }
}

fn marshal_name(opts: &CodegenOptions) -> &'static str {
    if opts.is_json() { "MarshalJSON" } else { "MarshalCBOR" }
}

fn unmarshal_name(opts: &CodegenOptions) -> &'static str {
    if opts.is_json() { "UnmarshalJSON" } else { "UnmarshalCBOR" }
}

fn format_name(opts: &CodegenOptions) -> &'static str {
    if opts.is_json() { "JSON" } else { "CBOR" }
}

/// `Encode` and `Decode<T>`, shared by every type.
fn emit_encode_decode(w: &mut IndentWriter, name: &str, opts: &CodegenOptions) {
    let f = format_name(opts);
    w.line(&format!("// Encode returns v as {f}."));
    w.line(&format!("func (v {name}) Encode() ([]byte, error) {{ return marshal(v) }}"));
    w.blank();
    w.line(&format!("// Decode{name} reads a {name} from {f}."));
    w.line(&format!("func Decode{name}(data []byte) ({name}, error) {{"));
    w.indent();
    w.line(&format!("var v {name}"));
    w.line("err := unmarshal(data, &v)");
    w.line("return v, err");
    w.dedent();
    w.line("}");
}

/// A Go return statement for an encoded value, wrapped in the type's tag.
fn return_encoded(w: &mut IndentWriter, tag: Option<cddlc_ir::CborTag>, expr: &str) {
    match tag {
        Some(t) => w.line(&format!("return tagWith({})({expr})", t.0)),
        None => w.line(&format!("return {expr}")),
    }
}

/// In an unmarshal method: `r` is the input, its own tag removed.
fn untag_input(w: &mut IndentWriter, tag: Option<cddlc_ir::CborTag>, name: &str) {
    match tag {
        Some(t) => {
            w.line(&format!("r, err := untag(data, {}, {})", t.0, go_str(name)));
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
        }
        None => w.line("r := data"),
    }
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Struct fields as gofmt lays them out: names padded to a common width within
/// each run of fields that no comment interrupts.
fn emit_fields(w: &mut IndentWriter, fields: &[(Option<&str>, String, String)]) {
    let mut i = 0;
    while i < fields.len() {
        // A run starts at a field (with or without a doc) and continues over
        // fields without one.
        let mut j = i + 1;
        while j < fields.len() && fields[j].0.is_none() {
            j += 1;
        }
        let width = fields[i..j].iter().map(|f| f.1.len()).max().unwrap_or(0);
        for (k, (doc, name, ty)) in fields[i..j].iter().enumerate() {
            if k == 0 {
                cddlc_codegen::emit_doc_lines(w, *doc, cddlc_codegen::DocStyle::Line("//"));
            }
            w.line(&format!("{name:<width$} {ty}"));
        }
        i = j;
    }
}

fn go_primitive(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool => "bool",
        Primitive::Null | Primitive::Undefined => "Null",
        Primitive::Uint => "uint64",
        Primitive::Int => "int64",
        Primitive::Float16 | Primitive::Float32 => "float32",
        Primitive::Float64 | Primitive::Float => "float64",
        Primitive::Tstr => "string",
        Primitive::Bstr | Primitive::Any => "[]byte",
    }
}

pub fn go_type(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Primitive(p) => go_primitive(p).to_owned(),
        TypeRef::Named(n) => go_type_name(n),
        TypeRef::Tagged(_, inner) => go_type(inner),
        TypeRef::Choice(_) => "any".into(),
    }
}

fn is_repeated(occ: &Occurrence) -> bool {
    matches!(occ, Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. } | Occurrence::Bounded { .. })
}

// ── Recursion (dgtlrift/cddlc#221) ───────────────────────────────────────────

thread_local! {
    /// The required members and variants held through a pointer, as (owner,
    /// item) names: set for each `generate`. (An optional member is a pointer
    /// already.)
    static POINTERS: std::cell::RefCell<std::collections::HashSet<(String, String)>> = Default::default();
}

/// Each required member and variant that holds its owner again, by value: a
/// pointer, as a Go type can't contain itself.
fn pointer_items(module: &IrModule) -> std::collections::HashSet<(String, String)> {
    let mut out = std::collections::HashSet::new();
    for (name, def) in &module.types {
        match def {
            TypeDef::Struct(s) => for f in &s.fields {
                if matches!(f.occurrence, Occurrence::Required) && holds_by_value(name, &f.ty, module) {
                    out.insert((name.to_string(), f.name.to_string()));
                }
            },
            TypeDef::Enum(e) => for v in &e.variants {
                if has_payload(v) && holds_by_value(name, &v.ty, module) {
                    out.insert((name.to_string(), v.name.to_string()));
                }
            },
            _ => {}
        }
    }
    out
}

/// Whether `ty` holds `from` again by value: through required members,
/// variants and aliases. A slice, a map and an optional member's pointer
/// already hold it elsewhere, so they end a cycle in Go.
fn holds_by_value(from: &str, ty: &TypeRef, module: &IrModule) -> bool {
    fn named(ty: &TypeRef) -> Option<&str> {
        match ty {
            TypeRef::Named(n) => Some(n.as_str()),
            TypeRef::Tagged(_, inner) => named(inner),
            _ => None,
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<&str> = named(ty).into_iter().collect();
    while let Some(n) = stack.pop() {
        if n == from {
            return true;
        }
        if !seen.insert(n) {
            continue;
        }
        match module.get(n) {
            Some(TypeDef::Struct(s)) => stack.extend(s.fields.iter()
                .filter(|f| matches!(f.occurrence, Occurrence::Required))
                .filter_map(|f| named(&f.ty))),
            Some(TypeDef::Enum(e)) => stack.extend(e.variants.iter().filter(|v| has_payload(v)).filter_map(|v| named(&v.ty))),
            Some(TypeDef::Alias(a)) => stack.extend(named(&a.ty)),
            _ => {}
        }
    }
    false
}

/// Whether `owner`'s required member or variant `item` is a pointer.
fn is_ptr(owner: &str, item: &str) -> bool {
    POINTERS.with(|p| p.borrow().contains(&(owner.to_owned(), item.to_owned())))
}

/// A required member as encoding reads it: through its pointer when it has
/// one (`Validate` has checked it isn't nil).
fn req_value(owner: &str, f: &cddlc_ir::FieldDef, field: &str) -> String {
    if is_ptr(owner, &f.name) { format!("*{field}") } else { field.to_owned() }
}

/// Decodes a required member from `raw` into `field`: through a value whose
/// address it takes when the member is a pointer.
fn emit_decode_member(w: &mut IndentWriter, owner: &str, f: &cddlc_ir::FieldDef, raw: &str, field: &str, at: &str) {
    if !is_ptr(owner, &f.name) {
        emit_decode(w, &f.ty, raw, field, at);
        return;
    }
    w.line("{");
    w.indent();
    w.line(&format!("var x {}", go_type(&f.ty)));
    emit_decode(w, &f.ty, raw, "x", at);
    w.line(&format!("{field} = &x"));
    w.dedent();
    w.line("}");
}

fn go_field_type(ty: &TypeRef, occ: &Occurrence) -> String {
    let base = go_type(ty);
    match occ {
        Occurrence::Optional => format!("*{base}"),
        o if is_repeated(o) => format!("[]{base}"),
        _ => base,
    }
}

/// A Go expression of type `([]byte, error)` encoding `expr`.
fn enc_expr(ty: &TypeRef, expr: &str) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => format!("encBytes({expr})"),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => "encNull()".into(),
        TypeRef::Primitive(_) | TypeRef::Named(_) | TypeRef::Choice(_) => format!("marshal({expr})"),
        TypeRef::Tagged(tag, inner) => format!("tagWith({})({})", tag.0, enc_expr(inner, expr)),
    }
}

/// An encoder for each element of the slice `expr`.
fn enc_each(ty: &TypeRef, expr: &str) -> String {
    format!(
        "encodeEach({expr}, func(x {}) ([]byte, error) {{ return {} }})",
        go_type(ty),
        enc_expr(ty, "x")
    )
}

/// The runtime decoder for a primitive alternative of a choice.
fn prim_decoder(ty: &TypeRef) -> Option<&'static str> {
    Some(match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => "decUint",
            Primitive::Int => "decInt",
            Primitive::Bool => "decBool",
            Primitive::Float16 | Primitive::Float32 => "decFloat32",
            Primitive::Float64 | Primitive::Float => "decFloat64",
            Primitive::Tstr => "decText",
            Primitive::Bstr => "decBytes",
            Primitive::Null | Primitive::Undefined => "decNull",
            Primitive::Any => return None,
        },
        _ => return None,
    })
}

/// Statements setting `dst` (of type `go_type(ty)`) from the encoded `src`, each
/// value's type checked; on failure the enclosing function returns the error.
fn emit_decode(w: &mut IndentWriter, ty: &TypeRef, src: &str, dst: &str, at: &str) {
    let at_lit = go_str(at);
    match ty {
        TypeRef::Primitive(p) => {
            let f = match p {
                Primitive::Uint => "decUint",
                Primitive::Int => "decInt",
                Primitive::Bool => "decBool",
                Primitive::Float16 | Primitive::Float32 => "decFloat32",
                Primitive::Float64 | Primitive::Float => "decFloat64",
                Primitive::Tstr => "decText",
                Primitive::Bstr | Primitive::Any => "decBytes",
                Primitive::Null | Primitive::Undefined => "decNull",
            };
            w.line("{");
            w.indent();
            // `val`, not `x`: the destination is often a local named `x`.
            w.line(&format!("val, err := {f}({src}, {at_lit})"));
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
            w.line(&format!("{dst} = val"));
            w.dedent();
            w.line("}");
        }
        // A choice of primitives: each alternative's decoder, in order, so a
        // value comes back as its alternative's type (an `int` as int64).
        TypeRef::Choice(alts) if alts.iter().all(|a| prim_decoder(a).is_some()) => {
            let decs: Vec<String> = alts.iter()
                .map(|a| format!("asAny({})", prim_decoder(a).unwrap()))
                .collect();
            w.line("{");
            w.indent();
            w.line(&format!("val, err := decChoice({src}, {at_lit}, {})", decs.join(", ")));
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
            w.line(&format!("{dst} = val"));
            w.dedent();
            w.line("}");
        }
        TypeRef::Named(_) | TypeRef::Choice(_) => {
            w.line(&format!("if err := decInto({src}, {at_lit}, &{dst}); err != nil {{"));
            w.line("    return err");
            w.line("}");
        }
        TypeRef::Tagged(tag, inner) => {
            w.line("{");
            w.indent();
            w.line(&format!("r, err := untag({src}, {}, {at_lit})", tag.0));
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
            emit_decode(w, inner, "r", dst, at);
            w.dedent();
            w.line("}");
        }
    }
}

// ── Constraints ───────────────────────────────────────────────────────────────

/// The primitive under tags, if `ty` is one.
fn base_primitive(ty: &TypeRef) -> Option<&Primitive> {
    match ty {
        TypeRef::Primitive(p) => Some(p),
        TypeRef::Tagged(_, inner) => base_primitive(inner),
        _ => None,
    }
}

fn go_literal(v: &LiteralValue) -> Option<String> {
    Some(match v {
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Uint(n) => n.to_string(),
        LiteralValue::Int(n) => n.to_string(),
        LiteralValue::Float(f) => format!("{f:?}"),
        LiteralValue::Text(s) => go_str(s),
        LiteralValue::Bytes(_) | LiteralValue::Null => return None,
    })
}

/// A Go boolean expression that holds when `x` (a value of primitive `p`)
/// meets the constraint, or `None` when it does not apply.
fn go_check(c: &Constraint, p: &Primitive, x: &str) -> Option<String> {
    let int_like = matches!(p, Primitive::Uint | Primitive::Int);
    let is_uint = matches!(p, Primitive::Uint);
    let len_ok = matches!(p, Primitive::Tstr | Primitive::Bstr | Primitive::Any);
    match c {
        Constraint::SizeExact(n) if len_ok => Some(format!("len({x}) == {n}")),
        Constraint::SizeRange { min, max } if len_ok => {
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("len({x}) >= {lo}")); }
            if let Some(hi) = max { parts.push(format!("len({x}) <= {hi}")); }
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        Constraint::ValueRangeInt { min, max, inclusive } if int_like || is_float(p) => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            // A uint is never below a negative bound, and Go rejects the comparison;
            // no uint is at or below a negative upper bound.
            if let Some(lo) = min.filter(|lo| !(is_uint && *lo <= 0)) { parts.push(format!("{x} >= {lo}")); }
            match max {
                Some(hi) if is_uint && *hi < 0 => return Some("false".into()),
                Some(hi) => parts.push(format!("{x} {le} {hi}")),
                None => {}
            }
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        Constraint::ValueRangeUint { min, max, inclusive } if int_like || is_float(p) => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            // No int64 reaches a lower bound past its range.
            if min.is_some_and(|lo| matches!(p, Primitive::Int) && lo > i64::MAX as u64) { return Some("false".into()); }
            if let Some(lo) = min.filter(|lo| *lo > 0) { parts.push(format!("{x} >= {lo}")); }
            // An int64 can't exceed a bound past its range, and Go rejects the comparison.
            if let Some(hi) = max.filter(|hi| is_uint || is_float(p) || *hi <= i64::MAX as u64) {
                parts.push(format!("{x} {le} {hi}"));
            }
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        Constraint::ValueRangeF64 { min, max, inclusive } if is_float(p) => {
            let le = if *inclusive { "<=" } else { "<" };
            let mut parts = Vec::new();
            if let Some(lo) = min { parts.push(format!("{x} >= {lo:?}")); }
            if let Some(hi) = max { parts.push(format!("{x} {le} {hi:?}")); }
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        Constraint::Eq(LiteralValue::Bytes(b)) if len_ok => Some(format!("string({x}) == {}", go_bytes_str(b))),
        Constraint::Ne(LiteralValue::Bytes(b)) if len_ok => Some(format!("string({x}) != {}", go_bytes_str(b))),
        Constraint::Eq(v) => literal_fits(v, p).then(|| go_literal(v)).flatten().map(|l| format!("{x} == {l}")),
        Constraint::Ne(v) => literal_fits(v, p).then(|| go_literal(v)).flatten().map(|l| format!("{x} != {l}")),
        // `.bits` (dgtlrift/cddlc#107): no bit outside the mask is set.
        Constraint::Bits(bits) if is_uint => Some(format!("{x}&^{:#x} == 0", cddlc_codegen::bits_mask_u64(bits))),
        Constraint::Bits(bits) if matches!(p, Primitive::Bstr) => {
            let m = cddlc_codegen::bits_mask_bytes(bits);
            Some(format!(
                "func(v []byte) bool {{ m := {}; for i, b := range v {{ var k byte; if i < len(m) {{ k = m[i] }}; if b&^k != 0 {{ return false }} }}; return true }}({x})",
                go_bytes_lit(&m)
            ))
        }
        _ => None,
    }
}

fn is_float(p: &Primitive) -> bool {
    matches!(p, Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float)
}

/// Whether a literal can be compared with a Go value of primitive `p`.
fn literal_fits(v: &LiteralValue, p: &Primitive) -> bool {
    match v {
        LiteralValue::Bool(_) => matches!(p, Primitive::Bool),
        LiteralValue::Text(_) => matches!(p, Primitive::Tstr),
        LiteralValue::Uint(n) => matches!(p, Primitive::Uint) || is_float(p) || (matches!(p, Primitive::Int) && *n <= i64::MAX as u64),
        LiteralValue::Int(n) => matches!(p, Primitive::Int) || is_float(p) || (matches!(p, Primitive::Uint) && *n >= 0),
        LiteralValue::Float(_) => is_float(p),
        LiteralValue::Bytes(_) | LiteralValue::Null => false,
    }
}

/// "value must be >= 0 and <= 100": the bounds a range check enforces.
fn bounds_description<T: std::fmt::Display>(what: &str, min: Option<T>, max: Option<T>, inclusive: bool) -> String {
    let le = if inclusive { "<=" } else { "<" };
    match (min, max) {
        (Some(lo), Some(hi)) => format!("{what} must be >= {lo} and {le} {hi}"),
        (Some(lo), None)     => format!("{what} must be >= {lo}"),
        (None, Some(hi))     => format!("{what} must be {le} {hi}"),
        (None, None)         => format!("{what} out of range"),
    }
}

fn describe(c: &Constraint) -> String {
    match c {
        Constraint::SizeExact(n) => format!("length must be {n}"),
        Constraint::SizeRange { min, max } => bounds_description("length", *min, *max, true),
        Constraint::ValueRangeInt { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::ValueRangeUint { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::ValueRangeF64 { min, max, inclusive } => bounds_description("value", *min, *max, *inclusive),
        Constraint::Eq(v) => format!("must equal {}", literal_text(v)),
        Constraint::Ne(v) => format!("must not equal {}", literal_text(v)),
        _ => "constraint violated".into(),
    }
}

fn literal_text(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(b) => format!("h'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        LiteralValue::Null => "nil".into(),
        other => go_literal(other).unwrap_or_default(),
    }
}

/// `if !(check) { return error }` for each constraint that applies to `x`.
fn emit_checks(w: &mut IndentWriter, ty: &TypeRef, constraints: &[Constraint], x: &str, at: &str) -> bool {
    let Some(p) = base_primitive(ty) else { return false };
    let mut any = false;
    for c in constraints {
        if let Some(check) = go_check(c, p, x) {
            any = true;
            w.line(&format!("if !({check}) {{"));
            w.line(&format!("    return fmt.Errorf(\"%s: %s\", {}, {})", go_str(at), go_str(&describe(c))));
            w.line("}");
        }
    }
    any
}

fn has_checks(ty: &TypeRef, constraints: &[Constraint]) -> bool {
    base_primitive(ty).is_some_and(|p| constraints.iter().any(|c| go_check(c, p, "x").is_some()))
}

// ── Struct ────────────────────────────────────────────────────────────────────

fn struct_fields<'a>(s: &'a StructDef, opts: &CodegenOptions) -> Vec<&'a cddlc_ir::FieldDef> {
    let mut fs: Vec<_> = s.fields.iter().collect();
    if opts.dcbor {
        fs.sort_by(|a, b| cddlc_codegen::dcbor_key_order(a, b));
    }
    fs
}

fn emit_struct(w: &mut IndentWriter, s: &StructDef, _module: &IrModule, opts: &CodegenOptions) {
    let name = go_type_name(&s.name);
    cddlc_codegen::emit_doc_lines(w, s.doc.as_deref(), cddlc_codegen::DocStyle::Line("//"));
    if s.deprecated { w.line("//"); w.line("// Deprecated: marked deprecated in the schema."); }
    w.line(&format!("type {name} struct {{"));
    w.indent();
    let mut fields: Vec<_> = s.fields.iter()
        .map(|f| {
            // A required member that holds this type again is a pointer
            // (dgtlrift/cddlc#221).
            let ty = if is_ptr(&s.name, &f.name) { format!("*{}", go_type(&f.ty)) } else { go_field_type(&f.ty, &f.occurrence) };
            (f.doc.as_deref(), go_field_name(&f.name), ty)
        })
        .collect();
    // The map's other entries (dgtlrift/cddlc#82).
    if let Some(r) = &s.rest {
        fields.push((Some("The map's other entries."), go_field_name(&r.name), format!("map[{}]{}", rest_key_go(r), go_type(&r.val_ty))));
    }
    emit_fields(w, &fields);
    w.dedent();
    w.line("}");
    w.blank();

    // Validate
    w.line("// Validate checks the CDDL constraints of every member.");
    w.line(&format!("func (v {name}) Validate() error {{"));
    w.indent();
    // A required pointer member must point at a value (dgtlrift/cddlc#221).
    for f in s.fields.iter().filter(|f| is_ptr(&s.name, &f.name)) {
        w.line(&format!("if v.{} == nil {{", go_field_name(&f.name)));
        w.line(&format!("    return fmt.Errorf(\"%s: missing\", {})", go_str(&format!("{name}.{}", f.name))));
        w.line("}");
    }
    for f in &s.fields {
        if !has_checks(&f.ty, &f.constraints) || is_repeated(&f.occurrence) { continue; }
        let field = format!("v.{}", go_field_name(&f.name));
        let at = format!("{name}.{}", f.name);
        if matches!(f.occurrence, Occurrence::Optional) {
            w.line(&format!("if {field} != nil {{"));
            w.indent();
            emit_checks(w, &f.ty, &f.constraints, &format!("*{field}"), &at);
            w.dedent();
            w.line("}");
        } else {
            emit_checks(w, &f.ty, &f.constraints, &field, &at);
        }
    }
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();

    if s.encoding == cddlc_ir::StructEncoding::Array {
        emit_record_marshal(w, s, &name, opts);
        emit_encode_decode(w, &name, opts);
        return;
    }

    // Marshal
    w.line(&format!("// {} writes v as a {} map, members in schema order.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    w.line("if err := v.Validate(); err != nil {");
    w.line("    return nil, err");
    w.line("}");
    w.line("var e mapEncoder");
    for f in struct_fields(s, opts) {
        let field = format!("v.{}", go_field_name(&f.name));
        let key = go_key(&f.key, opts);
        match &f.occurrence {
            Occurrence::Optional => {
                w.line(&format!("if {field} != nil {{"));
                w.line(&format!("    e.put({key})({})", enc_expr(&f.ty, &format!("*{field}"))));
                w.line("}");
            }
            o if is_repeated(o) => w.line(&format!("e.put({key})({})", enc_each(&f.ty, &field))),
            _ => w.line(&format!("e.put({key})({})", enc_expr(&f.ty, &req_value(&s.name, f, &field)))),
        }
    }
    // The other entries after the named members, in key order.
    if let Some(r) = &s.rest {
        let field = format!("v.{}", go_field_name(&r.name));
        // The other entries have text keys: only text named keys can clash.
        let named: Vec<String> = s.fields.iter()
            .filter_map(|f| match &f.key { FieldKey::Text(t) => Some(go_str(t)), _ => None })
            .collect();
        // Typed keys go in the order of their encodings (dgtlrift/cddlc#136).
        let order = if text_rest(r) { "sortedKeys" } else { "encodedKeyOrder" };
        w.line(&format!("for _, k := range {order}({field}) {{"));
        w.indent();
        if !named.is_empty() && text_rest(r) {
            w.line("switch k {");
            w.line(&format!("case {}:", named.join(", ")));
            w.line(&format!("    return nil, fmt.Errorf(\"%s: other entry %q is a named member's key\", {}, k)", go_str(&name)));
            w.line("}");
        }
        w.line(&format!("e.put(k)({})", enc_expr(&r.val_ty, &format!("{field}[k]"))));
        w.dedent();
        w.line("}");
    }
    let tag = if opts.is_json() { None } else { s.tagged };
    return_encoded(w, tag, "e.finish()");
    w.dedent();
    w.line("}");
    w.blank();

    // Unmarshal
    w.line(&format!("// {} reads v from a {} map, checking each member's type and constraints.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, &name);
    w.line(&format!("m, err := decodeMap(r, {})", go_str(&name)));
    w.line("if err != nil {");
    w.line("    return err");
    w.line("}");
    // A map has only the members its schema gives it (RFC 8610 §3.5;
    // dgtlrift/cddlc#102), unless it has other entries.
    if s.rest.is_none() {
        let named: Vec<String> = s.fields.iter().map(|f| go_key(&f.key, opts)).collect();
        w.line("for k := range m {");
        w.indent();
        if !named.is_empty() {
            w.line("switch k {");
            w.line(&format!("case {}:", named.join(", ")));
            w.line("    continue");
            w.line("}");
        }
        w.line(&format!("return fmt.Errorf(\"%s: unknown member %v\", {}, k)", go_str(&name)));
        w.dedent();
        w.line("}");
    }
    w.line(&format!("var out {name}"));
    for f in &s.fields {
        let field = format!("out.{}", go_field_name(&f.name));
        let key = go_key(&f.key, opts);
        let key_text = go_str(&cddlc_codegen::field_key_json(&f.key));
        let at = format!("{name}.{}", cddlc_codegen::field_key_json(&f.key));
        match &f.occurrence {
            Occurrence::Optional => {
                let is_null = matches!(base_primitive(&f.ty), Some(Primitive::Null | Primitive::Undefined));
                // A present `nil` member is null; for any other type null means absent.
                if is_null {
                    w.line(&format!("if r, ok := m[{key}]; ok {{"));
                } else {
                    w.line(&format!("if r, ok := m[{key}]; ok && !isNull(r) {{"));
                }
                w.indent();
                w.line(&format!("var x {}", go_type(&f.ty)));
                emit_decode(w, &f.ty, "r", "x", &at);
                w.line(&format!("{field} = &x"));
                w.dedent();
                w.line("}");
            }
            o if is_repeated(o) => {
                w.line(&format!("if r, ok := m[{key}]; ok {{"));
                w.indent();
                w.line(&format!("items, err := decodeArray(r, {})", go_str(&at)));
                w.line("if err != nil {");
                w.line("    return err");
                w.line("}");
                w.line(&format!("{field} = make([]{}, 0, len(items))", go_type(&f.ty)));
                w.line("for _, r := range items {");
                w.indent();
                w.line(&format!("var x {}", go_type(&f.ty)));
                emit_decode(w, &f.ty, "r", "x", &at);
                w.line(&format!("{field} = append({field}, x)"));
                w.dedent();
                w.line("}");
                w.dedent();
                w.line("}");
            }
            _ => {
                w.line("{");
                w.indent();
                w.line(&format!("r, ok := m[{key}]"));
                w.line("if !ok {");
                w.line(&format!("    return fmt.Errorf(\"%s: missing member %q\", {}, {key_text})", go_str(&name)));
                w.line("}");
                emit_decode_member(w, &s.name, f, "r", &field, &at);
                w.dedent();
                w.line("}");
            }
        }
    }
    if let Some(r) = &s.rest {
        let field = format!("out.{}", go_field_name(&r.name));
        let named: Vec<String> = s.fields.iter().map(|f| go_key(&f.key, opts)).collect();
        w.line("for k, r := range m {");
        w.indent();
        if !named.is_empty() {
            w.line("switch k {");
            w.line(&format!("case {}:", named.join(", ")));
            w.line("    continue");
            w.line("}");
        }
        w.line(&format!("var x {}", go_type(&r.val_ty)));
        emit_decode(w, &r.val_ty, "r", "x", &format!("{name} value"));
        w.line(&format!("if {field} == nil {{"));
        w.line(&format!("    {field} = map[{}]{}{{}}", rest_key_go(r), go_type(&r.val_ty)));
        w.line("}");
        if text_rest(r) {
            w.line(&format!("ks, err := keyText(k, {})", go_str(&name)));
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
            w.line(&format!("{field}[ks] = x"));
        } else {
            // A typed key: written again as CBOR, and read as its type
            // (dgtlrift/cddlc#136).
            w.line("kr, err := marshal(k)");
            w.line("if err != nil {");
            w.line("    return err");
            w.line("}");
            w.line(&format!("var kx {}", rest_key_go(r)));
            emit_decode(w, &r.key_ty, "kr", "kx", &format!("{name} key"));
            w.line(&format!("{field}[kx] = x"));
        }
        w.dedent();
        w.line("}");
    }
    w.line("if err := out.Validate(); err != nil {");
    w.line("    return err");
    w.line("}");
    w.line("*v = out");
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
    emit_encode_decode(w, &name, opts);
}

/// A record's marshal / unmarshal methods (RFC 8610 §3.4): an array of the
/// members' values, in order. Optional members come last: those present are
/// written up to the first absent one, and one set after an absent one is an error.
fn emit_record_marshal(w: &mut IndentWriter, s: &StructDef, name: &str, opts: &CodegenOptions) {
    if let Some(ri) = cddlc_codegen::record_repeated(s) {
        emit_record_marshal_repeated(w, s, ri, name, opts);
        return;
    }
    w.line(&format!("// {} writes v as a {} array, members in order.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    w.line("if err := v.Validate(); err != nil {");
    w.line("    return nil, err");
    w.line("}");
    w.line("var e mapEncoder");
    let mut gap = false;
    for f in &s.fields {
        let field = format!("v.{}", go_field_name(&f.name));
        let key = go_str(&f.name);
        if f.occurrence.is_required() {
            w.line(&format!("e.put({key})({})", enc_expr(&f.ty, &req_value(&s.name, f, &field))));
        } else {
            if !gap {
                w.line("gap := false");
                gap = true;
            }
            w.line(&format!("if {field} != nil {{"));
            w.indent();
            w.line("if gap {");
            w.line(&format!(
                "    return nil, fmt.Errorf(\"%s: set, but an optional member before it isn't\", {})",
                go_str(&format!("{name}.{}", f.name))
            ));
            w.line("}");
            w.line(&format!("e.put({key})({})", enc_expr(&f.ty, &format!("*{field}"))));
            w.dedent();
            w.line("} else {");
            w.line("    gap = true");
            w.line("}");
        }
    }
    if gap { w.line("_ = gap"); }
    let tag = if opts.is_json() { None } else { s.tagged };
    return_encoded(w, tag, "e.finishArray()");
    w.dedent();
    w.line("}");
    w.blank();

    let required = s.fields.iter().filter(|f| f.occurrence.is_required()).count();
    let want = if required == s.fields.len() { required.to_string() } else { format!("{required} to {}", s.fields.len()) };
    w.line(&format!("// {} reads v from a {} array, checking each member's type and constraints.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, name);
    w.line(&format!("items, err := decodeArray(r, {})", go_str(name)));
    w.line("if err != nil {");
    w.line("    return err");
    w.line("}");
    w.line(&format!("if len(items) < {required} || len(items) > {} {{", s.fields.len()));
    w.line(&format!("    return fmt.Errorf(\"%s: expected {want} items, got %d\", {}, len(items))", go_str(name)));
    w.line("}");
    w.line(&format!("var out {name}"));
    // One optional member before required ones: present when the array holds
    // every member; those after it move down one when it's absent
    // (dgtlrift/cddlc#134).
    let before = cddlc_codegen::record_optionals_before(s);
    if before.is_some() {
        // How far back a member sits when absent optional members come before it.
        w.line("shift := func(x int) int {");
        w.line("    if len(items) >= x {");
        w.line("        return 0");
        w.line("    }");
        w.line("    return x - len(items)");
        w.line("}");
    }
    for (i, f) in s.fields.iter().enumerate() {
        let field = format!("out.{}", go_field_name(&f.name));
        let at = format!("{name}.{}", f.name);
        let item = match &before {
            Some(b) if b[i] > 0 => format!("items[{i}-shift({})]", required + b[i]),
            _ => format!("items[{i}]"),
        };
        if f.occurrence.is_required() {
            emit_decode_member(w, &s.name, f, &item, &field, &at);
        } else {
            if let Some(b) = &before {
                w.line(&format!("if len(items) > {} {{", required + b[i]));
            } else {
                w.line(&format!("if len(items) > {i} {{"));
            }
            w.indent();
            w.line(&format!("var x {}", go_type(&f.ty)));
            emit_decode(w, &f.ty, &item, "x", &at);
            w.line(&format!("{field} = &x"));
            w.dedent();
            w.line("}");
        }
    }
    w.line("if err := out.Validate(); err != nil {");
    w.line("    return err");
    w.line("}");
    w.line("*v = out");
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
}

/// Returns an error unless a repeated member's item count, `n`, is within its
/// bounds (dgtlrift/cddlc#134); `ret` is what the function returns with it.
fn emit_go_repeat_check(w: &mut IndentWriter, name: &str, f: &cddlc_ir::FieldDef, n: &str, ret: &str) {
    let (min, max) = cddlc_codegen::repeat_bounds(&f.occurrence);
    let at = go_str(&format!("{name}.{}", f.name));
    if min > 0 {
        w.line(&format!("if {n} < {min} {{"));
        w.line(&format!("    return {ret}fmt.Errorf(\"%s: needs at least {min} items, got %d\", {at}, {n})"));
        w.line("}");
    }
    if let Some(max) = max {
        w.line(&format!("if {n} > {max} {{"));
        w.line(&format!("    return {ret}fmt.Errorf(\"%s: has more than {max} items, got %d\", {at}, {n})"));
        w.line("}");
    }
}

/// A record with a repeated member (`[primary, * extension, payload]`): the
/// member's items go inline, and the array's length says how many there are
/// (dgtlrift/cddlc#134).
fn emit_record_marshal_repeated(w: &mut IndentWriter, s: &StructDef, ri: usize, name: &str, opts: &CodegenOptions) {
    let rf = &s.fields[ri];
    let fixed = s.fields.len() - 1;
    w.line(&format!("// {} writes v as a {} array, members in order.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    w.line("if err := v.Validate(); err != nil {");
    w.line("    return nil, err");
    w.line("}");
    emit_go_repeat_check(w, name, rf, &format!("len(v.{})", go_field_name(&rf.name)), "nil, ");
    w.line("var e mapEncoder");
    for f in &s.fields {
        let field = format!("v.{}", go_field_name(&f.name));
        let key = go_str(&f.name);
        if f.occurrence.is_repeated() {
            w.line(&format!("for _, x := range {field} {{"));
            w.line(&format!("    e.put({key})({})", enc_expr(&f.ty, "x")));
            w.line("}");
        } else {
            w.line(&format!("e.put({key})({})", enc_expr(&f.ty, &req_value(&s.name, f, &field))));
        }
    }
    let tag = if opts.is_json() { None } else { s.tagged };
    return_encoded(w, tag, "e.finishArray()");
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("// {} reads v from a {} array, checking each member's type and constraints.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, name);
    w.line(&format!("items, err := decodeArray(r, {})", go_str(name)));
    w.line("if err != nil {");
    w.line("    return err");
    w.line("}");
    w.line(&format!("if len(items) < {fixed} {{"));
    w.line(&format!("    return fmt.Errorf(\"%s: expected at least {fixed} items, got %d\", {}, len(items))", go_str(name)));
    w.line("}");
    w.line(&format!("reps := len(items) - {fixed}"));
    emit_go_repeat_check(w, name, rf, "reps", "");
    w.line(&format!("var out {name}"));
    for (i, f) in s.fields.iter().enumerate() {
        let field = format!("out.{}", go_field_name(&f.name));
        let at = format!("{name}.{}", f.name);
        if f.occurrence.is_repeated() {
            w.line(&format!("{field} = make([]{}, 0, reps)", go_type(&f.ty)));
            w.line("for k := 0; k < reps; k++ {");
            w.indent();
            w.line(&format!("var x {}", go_type(&f.ty)));
            emit_decode(w, &f.ty, &format!("items[{i}+k]"), "x", &at);
            w.line(&format!("{field} = append({field}, x)"));
            w.dedent();
            w.line("}");
        } else {
            let item = if i < ri { format!("items[{i}]") } else { format!("items[{}+reps]", i - 1) };
            emit_decode_member(w, &s.name, f, &item, &field, &at);
        }
    }
    w.line("if err := out.Validate(); err != nil {");
    w.line("    return err");
    w.line("}");
    w.line("*v = out");
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
}

// ── Enum ──────────────────────────────────────────────────────────────────────

/// The integer literal of an integer variant (the `2` in `1 / 2 / 3`).
pub fn int_literal(v: &EnumVariant) -> Option<i128> {
    match &v.ty {
        TypeRef::Primitive(Primitive::Uint | Primitive::Int) => v.constraints.iter().find_map(|c| match c {
            Constraint::Eq(LiteralValue::Uint(n)) => Some(*n as i128),
            Constraint::Eq(LiteralValue::Int(n)) => Some(*n as i128),
            _ => None,
        }),
        _ => None,
    }
}

/// Whether a variant carries a value (not a text, nil or integer literal).
pub fn has_payload(v: &EnumVariant) -> bool {
    if int_literal(v).is_some() {
        return false;
    }
    match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) => !v.constraints.iter().any(|c| matches!(c, Constraint::Eq(LiteralValue::Text(_)))),
        TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => false,
        TypeRef::Primitive(_) | TypeRef::Named(_) => true,
        TypeRef::Tagged(..) | TypeRef::Choice(_) => false,
    }
}

/// The name of the struct field holding a variant's payload.
pub fn payload_field(v: &EnumVariant) -> String {
    let n = to_pascal_case(&v.name);
    if n == "Kind" { "KindValue".into() } else { n }
}

/// Whether a variant is `null` / `nil` (or `undefined`): CBOR / JSON null.
fn is_nil(v: &EnumVariant) -> bool {
    matches!(&v.ty, TypeRef::Primitive(Primitive::Null | Primitive::Undefined)) && !has_payload(v)
}

fn emit_enum(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions) {
    let name = go_type_name(&e.name);
    let kind_ty = format!("{name}Kind");
    let at = go_str(&name);

    cddlc_codegen::emit_doc_lines(w, e.doc.as_deref(), cddlc_codegen::DocStyle::Line("//"));
    if e.deprecated { w.line("//"); w.line("// Deprecated: marked deprecated in the schema."); }
    w.line(&format!("// {name} is one of its variants: Kind names which, and a valued variant's"));
    w.line("// value is in the field named after it.");
    w.line(&format!("type {name} struct {{"));
    w.indent();
    let mut fields = vec![(None, "Kind".to_owned(), kind_ty.clone())];
    // A variant that holds this type again is a pointer (dgtlrift/cddlc#221).
    fields.extend(e.variants.iter().filter(|v| has_payload(v)).map(|v| {
        let ty = if is_ptr(&e.name, &v.name) { format!("*{}", go_type(&v.ty)) } else { go_type(&v.ty) };
        (None, payload_field(v), ty)
    }));
    emit_fields(w, &fields);
    w.dedent();
    w.line("}");
    w.blank();
    w.line(&format!("// {kind_ty} names a variant of {name}."));
    w.line(&format!("type {kind_ty} int"));
    w.blank();
    w.line("const (");
    w.indent();
    for (i, v) in e.variants.iter().enumerate() {
        let c = go_kind_const(&e.name, &v.name);
        if i == 0 { w.line(&format!("{c} {kind_ty} = iota")); } else { w.line(&c); }
    }
    w.dedent();
    w.line(")");
    w.blank();

    for v in &e.variants {
        let vname = to_pascal_case(&v.name);
        let c = go_kind_const(&e.name, &v.name);
        if has_payload(v) {
            w.line(&format!("// New{name}{vname} is the {vname} variant holding x."));
            let x = if is_ptr(&e.name, &v.name) { "&x" } else { "x" };
            w.line(&format!("func New{name}{vname}(x {}) {name} {{ return {name}{{Kind: {c}, {}: {x}}} }}", go_type(&v.ty), payload_field(v)));
        } else {
            w.line(&format!("// New{name}{vname} is the {vname} variant."));
            w.line(&format!("func New{name}{vname}() {name} {{ return {name}{{Kind: {c}}} }}"));
        }
        w.blank();
    }

    let tag = if opts.is_json() { None } else { e.tagged };

    // Marshal
    w.line(&format!("// {} writes the variant v.Kind names.", marshal_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    w.line("switch v.Kind {");
    for v in &e.variants {
        w.line(&format!("case {}:", go_kind_const(&e.name, &v.name)));
        w.indent();
        let expr = if let Some(n) = int_literal(v) {
            if n < 0 { format!("marshal(int64({n}))") } else { format!("marshal(uint64({n}))") }
        } else if has_payload(v) && is_ptr(&e.name, &v.name) {
            // A pointer variant is written through it; nil is an error.
            w.line(&format!("if v.{} == nil {{", payload_field(v)));
            w.line(&format!("    return nil, fmt.Errorf(\"%s: %s is nil\", {at}, {})", go_str(&v.name)));
            w.line("}");
            enc_expr(&v.ty, &format!("*v.{}", payload_field(v)))
        } else if has_payload(v) {
            enc_expr(&v.ty, &format!("v.{}", payload_field(v)))
        } else if is_nil(v) {
            // `null` is CBOR / JSON null, not text (dgtlrift/cddlc#147).
            "marshal(nil)".to_owned()
        } else {
            // A text literal is written as its text.
            format!("marshal({})", go_str(&cddlc_codegen::enum_variant_text(v)))
        };
        return_encoded(w, tag, &expr);
        w.dedent();
    }
    w.line("}");
    w.line(&format!("return nil, fmt.Errorf(\"%s: unknown kind %d\", {at}, v.Kind)"));
    w.dedent();
    w.line("}");
    w.blank();

    // Unmarshal: literals first, then the variant whose type the value has.
    w.line(&format!("// {} picks the variant from the value's type, matching literals first.", unmarshal_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, &name);
    let payload = |f: &dyn Fn(&Primitive) -> bool| {
        e.variants.iter().find(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(p) if f(p)))
    };
    let set = |v: &EnumVariant, value: Option<&str>| match value {
        // A pointer variant holds the value's address (dgtlrift/cddlc#221).
        Some(x) if is_ptr(&e.name, &v.name) => format!("*v = {name}{{Kind: {}, {}: &{x}}}", go_kind_const(&e.name, &v.name), payload_field(v)),
        Some(x) => format!("*v = {name}{{Kind: {}, {}: {x}}}", go_kind_const(&e.name, &v.name), payload_field(v)),
        None => format!("*v = {name}{{Kind: {}}}", go_kind_const(&e.name, &v.name)),
    };
    let text_lits: Vec<&EnumVariant> = e.variants.iter().filter(|v| !has_payload(v) && int_literal(v).is_none() && !is_nil(v)).collect();
    let nil = e.variants.iter().find(|v| is_nil(v));
    let tstr = payload(&|p| matches!(p, Primitive::Tstr));
    let uint = payload(&|p| matches!(p, Primitive::Uint));
    let int = payload(&|p| matches!(p, Primitive::Int));
    let float = payload(&|p| is_float(p));
    let boolean = payload(&|p| matches!(p, Primitive::Bool));
    let any_bytes = payload(&|p| matches!(p, Primitive::Bstr | Primitive::Any));
    let bytes = any_bytes.filter(|_| !opts.is_json());
    // JSON writes bytes as base64url text: read them in the text branch, unless a
    // tstr variant takes any text (the two can't be told apart).
    let json_bytes = any_bytes.filter(|_| opts.is_json() && tstr.is_none());
    let pos_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n >= 0).collect();
    let neg_lits: Vec<(&EnumVariant, i128)> = e.variants.iter().filter_map(|v| int_literal(v).map(|n| (v, n))).filter(|(_, n)| *n < 0).collect();

    w.line("switch kindOf(r) {");
    if let Some(v) = nil {
        w.line("case kindNull:");
        w.line(&format!("    {}", set(v, None)));
        w.line("    return nil");
    }
    if !text_lits.is_empty() || tstr.is_some() || json_bytes.is_some() {
        w.line("case kindText:");
        w.indent();
        w.line(&format!("s, err := decText(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        if !text_lits.is_empty() {
            w.line("switch s {");
            for v in &text_lits {
                w.line(&format!("case {}:", go_str(&cddlc_codegen::enum_variant_text(v))));
                w.line(&format!("    {}", set(v, None)));
                w.line("    return nil");
            }
            w.line("}");
        }
        if let Some(v) = tstr {
            w.line(&set(v, Some("s")));
            w.line("return nil");
        }
        if let Some(v) = json_bytes {
            w.line(&format!("if b, err := decBytes(r, {at}); err == nil {{"));
            w.line(&format!("    {}", set(v, Some("b"))));
            w.line("    return nil");
            w.line("}");
        }
        if text_lits.is_empty() && tstr.is_none() {
            w.line("_ = s");
        }
        w.dedent();
    }
    if !pos_lits.is_empty() || uint.is_some() || int.is_some() || float.is_some() {
        w.line("case kindUint:");
        w.indent();
        w.line(&format!("u, err := decUint(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        if !pos_lits.is_empty() {
            w.line("switch u {");
            for (v, n) in &pos_lits {
                w.line(&format!("case {n}:"));
                w.line(&format!("    {}", set(v, None)));
                w.line("    return nil");
            }
            w.line("}");
        }
        if let Some(v) = uint {
            w.line(&set(v, Some("u")));
            w.line("return nil");
        } else if let Some(v) = int {
            w.line("if u <= 9223372036854775807 {");
            w.line(&format!("    {}", set(v, Some("int64(u)"))));
            w.line("    return nil");
            w.line("}");
        } else if let Some(v) = float {
            w.line(&set(v, Some(&format!("{}(u)", go_type(&v.ty)))));
            w.line("return nil");
        } else {
            w.line("_ = u");
        }
        w.dedent();
    }
    if !neg_lits.is_empty() || int.is_some() || float.is_some() {
        w.line("case kindNint:");
        w.indent();
        w.line(&format!("i, err := decInt(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        if !neg_lits.is_empty() {
            w.line("switch i {");
            for (v, n) in &neg_lits {
                w.line(&format!("case {n}:"));
                w.line(&format!("    {}", set(v, None)));
                w.line("    return nil");
            }
            w.line("}");
        }
        if let Some(v) = int {
            w.line(&set(v, Some("i")));
            w.line("return nil");
        } else if let Some(v) = float {
            w.line(&set(v, Some(&format!("{}(i)", go_type(&v.ty)))));
            w.line("return nil");
        } else {
            w.line("_ = i");
        }
        w.dedent();
    }
    if let Some(v) = float {
        w.line("case kindFloat:");
        w.indent();
        w.line(&format!("f, err := decFloat64(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        w.line(&set(v, Some(&format!("{}(f)", go_type(&v.ty)))));
        w.line("return nil");
        w.dedent();
    }
    if let Some(v) = boolean {
        w.line("case kindBool:");
        w.indent();
        w.line(&format!("b, err := decBool(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        w.line(&set(v, Some("b")));
        w.line("return nil");
        w.dedent();
    }
    if let Some(v) = bytes {
        w.line("case kindBytes:");
        w.indent();
        w.line(&format!("b, err := decBytes(r, {at})"));
        w.line("if err != nil {");
        w.line("    return err");
        w.line("}");
        w.line(&set(v, Some("b")));
        w.line("return nil");
        w.dedent();
    }
    w.line("}");
    for v in e.variants.iter().filter(|v| has_payload(v) && matches!(&v.ty, TypeRef::Named(_))) {
        w.line("{");
        w.indent();
        w.line(&format!("var x {}", go_type(&v.ty)));
        w.line("if unmarshal(r, &x) == nil {");
        w.line(&format!("    {}", set(v, Some("x"))));
        w.line("    return nil");
        w.line("}");
        w.dedent();
        w.line("}");
    }
    w.line(&format!("return mismatch({at}, \"one of its variants\", r)"));
    w.dedent();
    w.line("}");
    w.blank();
    emit_encode_decode(w, &name, opts);
}

// ── Array ─────────────────────────────────────────────────────────────────────

/// For an array of a repeated group, the number of members in the group,
/// which sit inline in the array (dgtlrift/cddlc#90).
fn flat_width(a: &ArrayDef, module: &IrModule) -> Option<usize> {
    match (&a.element, a.flat) {
        (TypeRef::Named(n), true) => match module.get(n) {
            Some(TypeDef::Struct(s)) => Some(s.fields.len()),
            _ => None,
        },
        _ => None,
    }
}

fn emit_array(w: &mut IndentWriter, a: &ArrayDef, flat: Option<usize>, opts: &CodegenOptions) {
    let name = go_type_name(&a.name);
    let elem = go_type(&a.element);
    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Line("//"));
    if a.deprecated { w.line("//"); w.line("// Deprecated: marked deprecated in the schema."); }
    w.line(&format!("type {name} []{elem}"));
    w.blank();

    let tag = if opts.is_json() { None } else { a.tagged };
    w.line(&format!("// {} writes v as a {} array.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    let each = enc_each(&a.element, &format!("[]{elem}(v)"));
    // A repeated group: each record's members, inline.
    let each = if flat.is_some() { each.replacen("encodeEach(", "encodeFlat(", 1) } else { each };
    return_encoded(w, tag, &each);
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("// {} reads v from a {} array, checking each item's type.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, &name);
    match flat {
        Some(n) => w.line(&format!("items, err := decodeFlat(r, {n}, {})", go_str(&name))),
        None => w.line(&format!("items, err := decodeArray(r, {})", go_str(&name))),
    }
    w.line("if err != nil {");
    w.line("    return err");
    w.line("}");
    w.line(&format!("out := make({name}, 0, len(items))"));
    w.line("for _, r := range items {");
    w.indent();
    w.line(&format!("var x {elem}"));
    emit_decode(w, &a.element, "r", "x", &format!("{name} item"));
    w.line("out = append(out, x)");
    w.dedent();
    w.line("}");
    w.line("*v = out");
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
    emit_encode_decode(w, &name, opts);
}

// ── Map ───────────────────────────────────────────────────────────────────────

/// A table (`{ * tstr => uint }`): a Go map (dgtlrift/cddlc#82).
fn emit_map(w: &mut IndentWriter, m: &MapDef, opts: &CodegenOptions) {
    let name = go_type_name(&m.name);
    let key = go_type(&m.key_ty);
    let val = go_type(&m.val_ty);
    cddlc_codegen::emit_doc_lines(w, m.doc.as_deref(), cddlc_codegen::DocStyle::Line("//"));
    if m.deprecated { w.line("//"); w.line("// Deprecated: marked deprecated in the schema."); }
    w.line(&format!("type {name} map[{key}]{val}"));
    w.blank();

    let tag = if opts.is_json() { None } else { m.tagged };
    w.line(&format!("// {} writes v as a {} map, in the order of its encoded keys.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    let each = format!(
        "encodeTable(map[{key}]{val}(v), func(x {val}) ([]byte, error) {{ return {} }})",
        enc_expr(&m.val_ty, "x")
    );
    return_encoded(w, tag, &each);
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("// {} reads v from a {} map, checking each entry's types.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, &name);
    w.line(&format!("entries, err := decodeTable[{key}](r, {})", go_str(&name)));
    w.line("if err != nil {");
    w.line("    return err");
    w.line("}");
    w.line(&format!("out := make({name}, len(entries))"));
    w.line("for k, r := range entries {");
    w.indent();
    w.line(&format!("var x {val}"));
    emit_decode(w, &m.val_ty, "r", "x", &format!("{name} value"));
    w.line("out[k] = x");
    w.dedent();
    w.line("}");
    w.line("*v = out");
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
    emit_encode_decode(w, &name, opts);
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    let name = go_type_name(&a.name);
    let inner = go_type(&a.ty);
    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Line("//"));
    if a.deprecated { w.line("//"); w.line("// Deprecated: marked deprecated in the schema."); }
    w.line(&format!("type {name} {inner}"));
    w.blank();

    w.line("// Validate checks the CDDL constraints on v.");
    w.line(&format!("func (v {name}) Validate() error {{"));
    w.indent();
    if has_checks(&a.ty, &a.constraints) {
        w.line(&format!("x := {inner}(v)"));
        emit_checks(w, &a.ty, &a.constraints, "x", &name);
    }
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();

    let tag = if opts.is_json() { None } else { a.tagged };
    w.line(&format!("// {} writes v as {}.", marshal_name(opts), format_name(opts)));
    w.line(&format!("func (v {name}) {}() ([]byte, error) {{", marshal_name(opts)));
    w.indent();
    w.line("if err := v.Validate(); err != nil {");
    w.line("    return nil, err");
    w.line("}");
    return_encoded(w, tag, &enc_expr(&a.ty, &format!("{inner}(v)")));
    w.dedent();
    w.line("}");
    w.blank();

    w.line(&format!("// {} reads v from {}, checking its type and constraints.", unmarshal_name(opts), format_name(opts)));
    w.line(&format!("func (v *{name}) {}(data []byte) error {{", unmarshal_name(opts)));
    w.indent();
    untag_input(w, tag, &name);
    w.line(&format!("var x {inner}"));
    emit_decode(w, &a.ty, "r", "x", &name);
    w.line(&format!("if err := {name}(x).Validate(); err != nil {{"));
    w.line("    return err");
    w.line("}");
    w.line(&format!("*v = {name}(x)"));
    w.line("return nil");
    w.dedent();
    w.line("}");
    w.blank();
    emit_encode_decode(w, &name, opts);
}

// ── Test file ─────────────────────────────────────────────────────────────────

/// Table helpers (dgtlrift/cddlc#82): entries are written in the order of
/// their encoded keys, so the output is deterministic.
const TABLE_HELPERS: &str = r#"// encodeTable writes a table's entries in the order of their encoded keys.
func encodeTable[K comparable, V any](m map[K]V, val func(V) ([]byte, error)) ([]byte, error) {
	type entry struct{ k, v []byte }
	es := make([]entry, 0, len(m))
	for k, x := range m {
		kb, err := marshal(k)
		if err != nil {
			return nil, err
		}
		vb, err := val(x)
		if err != nil {
			return nil, fmt.Errorf("[%v]: %w", k, err)
		}
		es = append(es, entry{kb, vb})
	}
	slices.SortFunc(es, func(a, b entry) int { return bytes.Compare(a.k, b.k) })
	keys := make([][]byte, len(es))
	vals := make([][]byte, len(es))
	for i, e := range es {
		keys[i], vals[i] = e.k, e.v
	}
	return encodeEntries(keys, vals)
}

// decodeTable reads a map's entries, each key as a K.
func decodeTable[K comparable](r []byte, at string) (map[K]raw, error) {
	if kindOf(r) != kindMap {
		return nil, mismatch(at, "a map", r)
	}
	var m map[K]raw
	if err := unmarshal(r, &m); err != nil {
		return nil, fmt.Errorf("%s: %w", at, err)
	}
	return m, nil
}"#;

const CBOR_TABLE_HELPERS: &str = r#"// encodeEntries writes encoded keys and values as a CBOR map.
func encodeEntries(keys, vals [][]byte) ([]byte, error) {
	out := head(5, uint64(len(keys)))
	for i, k := range keys {
		out = append(out, k...)
		out = append(out, vals[i]...)
	}
	return out, nil
}"#;

const JSON_TABLE_HELPERS: &str = r#"// encodeEntries writes encoded keys and values as a JSON object.
func encodeEntries(keys, vals [][]byte) ([]byte, error) {
	var buf bytes.Buffer
	buf.WriteByte('{')
	for i, k := range keys {
		if i > 0 {
			buf.WriteByte(',')
		}
		buf.Write(k)
		buf.WriteByte(':')
		buf.Write(vals[i])
	}
	buf.WriteByte('}')
	return buf.Bytes(), nil
}"#;

/// A map's other entries are written in key order.
const ENCODED_KEY_ORDER_HELPER: &str = r#"// encodedKeyOrder returns m's keys in the order of their CBOR encodings.
func encodedKeyOrder[K comparable, V any](m map[K]V) []K {
	keys := make([]K, 0, len(m))
	enc := map[K][]byte{}
	for k := range m {
		b, _ := marshal(k)
		enc[k] = b
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool { return bytes.Compare(enc[keys[i]], enc[keys[j]]) < 0 })
	return keys
}"#;

/// Whether a map's other entries have plain `tstr` keys (otherwise they hold
/// their key type; dgtlrift/cddlc#136).
fn text_rest(r: &cddlc_ir::MapRest) -> bool {
    matches!(r.key_ty, TypeRef::Primitive(Primitive::Tstr))
}

/// The Go type of a map's other entries' keys.
fn rest_key_go(r: &cddlc_ir::MapRest) -> String {
    if text_rest(r) { "string".into() } else { go_type(&r.key_ty) }
}

const SORTED_KEYS_HELPER: &str = r#"// sortedKeys returns m's keys in order.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}"#;

const TEST_HELPERS: &str = r#"type encodable interface {
	Encode() ([]byte, error)
}

// roundtrip encodes orig, decodes the result and checks it reads back equal.
func roundtrip[T encodable](t *testing.T, orig T, decode func([]byte) (T, error)) {
	t.Helper()
	data, err := orig.Encode()
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := decode(data)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if !reflect.DeepEqual(got, orig) {
		t.Fatalf("roundtrip mismatch:\n got  %#v\n want %#v", got, orig)
	}
}

// roundtripBytes encodes orig, decodes the result and checks it encodes the
// same: for a value that may decode as another alternative of its choice.
func roundtripBytes[T encodable](t *testing.T, orig T, decode func([]byte) (T, error)) {
	t.Helper()
	data, err := orig.Encode()
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	got, err := decode(data)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	again, err := got.Encode()
	if err != nil {
		t.Fatalf("encode again: %v", err)
	}
	if !reflect.DeepEqual(again, data) {
		t.Fatalf("roundtrip mismatch:\n got  %x\n want %x", again, data)
	}
}

func ptr[T any](v T) *T { return &v }"#;

fn emit_tests(module: &IrModule, pkg: &str, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(4);
    header(&mut w, opts);
    w.blank();
    w.line(&format!("package {pkg}"));
    w.blank();
    w.line("import (");
    w.line("\t\"reflect\"");
    w.line("\t\"testing\"");
    w.line(")");
    w.blank();
    for line in TEST_HELPERS.lines() {
        if line.is_empty() { w.blank() } else { w.line(line) }
    }
    w.blank();

    for (_, def) in &module.types {
        match def {
            TypeDef::Struct(s) => emit_struct_tests(&mut w, s, module, opts),
            TypeDef::Enum(e) => emit_enum_tests(&mut w, e, module, opts),
            TypeDef::Alias(a) => emit_alias_tests(&mut w, a, module),
            TypeDef::Array(a) => emit_array_tests(&mut w, a, module),
            TypeDef::Map(m) => emit_map_tests(&mut w, m, module),
        }
    }
    tabify(w.finish())
}

fn test_fn(w: &mut IndentWriter, name: &str, body: &[String]) {
    w.line(&format!("func {name}(t *testing.T) {{"));
    w.indent();
    for l in body {
        for part in l.lines() {
            w.line(part);
        }
    }
    w.dedent();
    w.line("}");
    w.blank();
}

fn emit_struct_tests(w: &mut IndentWriter, s: &StructDef, module: &IrModule, opts: &CodegenOptions) {
    let name = go_type_name(&s.name);
    for (suffix, explicit) in [("Default", false), ("Explicit", true)] {
        let value = go_struct_value(s, module, explicit, 0);
        test_fn(w, &format!("Test{name}Roundtrip{suffix}"), &[format!("roundtrip(t, {value}, Decode{name})")]);
    }
    if s.tagged.is_some() && !opts.is_json() {
        let value = go_struct_value(s, module, false, 0);
        test_fn(w, &format!("Test{name}EncodeHasTag"), &[
            format!("data, err := {value}.Encode()"),
            "if err != nil {\n    t.Fatal(err)\n}".into(),
            "if data[0]>>5 != 6 {\n    t.Fatalf(\"expected CBOR major type 6 (tag), got %d\", data[0]>>5)\n}".into(),
        ]);
    }
}

/// A test value for one enum variant.
fn go_variant_value(e: &EnumDef, v: &EnumVariant, module: &IrModule, depth: usize) -> String {
    let name = go_type_name(&e.name);
    let vname = to_pascal_case(&v.name);
    if !has_payload(v) {
        return format!("New{name}{vname}()");
    }
    format!("New{name}{vname}({})", go_value(&v.ty, &Occurrence::Required, &[], module, true, depth + 1))
}

fn emit_enum_tests(w: &mut IndentWriter, e: &EnumDef, module: &IrModule, opts: &CodegenOptions) {
    let name = go_type_name(&e.name);
    let takes_text = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr)));
    for v in &e.variants {
        // In JSON a byte string is text, which a tstr variant decodes first.
        if opts.is_json() && takes_text && has_payload(v) && matches!(&v.ty, TypeRef::Primitive(Primitive::Bstr | Primitive::Any)) {
            continue;
        }
        let value = go_variant_value(e, v, module, 0);
        // A map variant with the same members as an earlier one decodes as
        // that one, to the same encoding (dgtlrift/cddlc#192).
        let check = if e.variants.iter().take_while(|x| x.name != v.name).any(|x| cddlc_codegen::same_map_shape(&x.ty, &v.ty, module)) {
            "roundtripBytes"
        } else {
            "roundtrip"
        };
        test_fn(w, &format!("Test{name}Roundtrip{}", to_pascal_case(&v.name)), &[format!("{check}(t, {value}, Decode{name})")]);
    }
    // An integer no literal names is rejected, unless some variant takes any number.
    let lits: Vec<i128> = e.variants.iter().filter_map(int_literal).collect();
    let takes_number = e.variants.iter().any(|v| has_payload(v) && matches!(&v.ty,
        TypeRef::Primitive(p) if matches!(p, Primitive::Uint | Primitive::Int) || is_float(p)));
    if !lits.is_empty() && !takes_number && e.tagged.is_none() {
        if let Some(other) = (0i128..24).find(|k| !lits.contains(k)) {
            // A CBOR unsigned integer below 24 is the one byte itself.
            let data = if opts.is_json() { format!("[]byte(\"{other}\")") } else { format!("[]byte{{{other}}}") };
            test_fn(w, &format!("Test{name}RejectsUnnamedInteger"), &[
                format!("if _, err := Decode{name}({data}); err == nil {{\n    t.Fatal(\"expected an error for an integer no variant names\")\n}}"),
            ]);
        }
    }
}

fn emit_alias_tests(w: &mut IndentWriter, a: &AliasDef, module: &IrModule) {
    let name = go_type_name(&a.name);
    let valid = go_value(&a.ty, &Occurrence::Required, &a.constraints, module, false, 1);
    test_fn(w, &format!("Test{name}RoundtripValid"), &[format!("roundtrip(t, {name}({valid}), Decode{name})")]);
    if let Some(bad) = go_invalid_value(&a.constraints, &a.ty) {
        test_fn(w, &format!("Test{name}InvalidRejected"), &[
            format!("if _, err := {name}({bad}).Encode(); err == nil {{\n    t.Fatal(\"expected a constraint error\")\n}}"),
        ]);
    }
}

fn emit_array_tests(w: &mut IndentWriter, a: &ArrayDef, module: &IrModule) {
    let name = go_type_name(&a.name);
    test_fn(w, &format!("Test{name}RoundtripEmpty"), &[format!("roundtrip(t, {name}{{}}, Decode{name})")]);
    let item = go_value(&a.element, &Occurrence::Required, &[], module, true, 1);
    test_fn(w, &format!("Test{name}RoundtripItems"), &[format!("roundtrip(t, {name}{{{item}, {item}}}, Decode{name})")]);
}

fn emit_map_tests(w: &mut IndentWriter, m: &MapDef, module: &IrModule) {
    let name = go_type_name(&m.name);
    test_fn(w, &format!("Test{name}RoundtripEmpty"), &[format!("roundtrip(t, {name}{{}}, Decode{name})")]);
    test_fn(w, &format!("Test{name}RoundtripEntries"), &[format!("roundtrip(t, {}, Decode{name})", go_map_value(m, module, true, 1))]);
}

// ── Test values ───────────────────────────────────────────────────────────────

/// Nesting depth past which a named type's test value is not built further.
const MAX_FIXTURE_DEPTH: usize = 4;

/// A table with one entry.
fn go_map_value(m: &MapDef, module: &IrModule, explicit: bool, depth: usize) -> String {
    let key = go_value(&m.key_ty, &Occurrence::Required, &[], module, explicit, depth + 1);
    let val = go_value(&m.val_ty, &Occurrence::Required, &[], module, explicit, depth + 1);
    format!("{}{{{key}: {val}}}", go_type_name(&m.name))
}

fn go_struct_value(s: &StructDef, module: &IrModule, explicit: bool, depth: usize) -> String {
    let mut fields: Vec<String> = s.fields.iter().map(|f| {
        let v = go_value(&f.ty, &f.occurrence, &f.constraints, module, explicit, depth + 1);
        // A required pointer member points at a value (dgtlrift/cddlc#221).
        let v = if is_ptr(&s.name, &f.name) { format!("ptr({v})") } else { v };
        format!("{}: {v}", go_field_name(&f.name))
    }).collect();
    // One other entry when explicit or when the map needs one; else none (nil).
    if let Some(r) = &s.rest {
        if explicit || matches!(r.occurrence, Occurrence::OneOrMore { .. }) {
            let v = go_value(&r.val_ty, &Occurrence::Required, &[], module, explicit, depth + 1);
            // A key no member has: text, or 1000 for an integer key, through an
            // enum's integer alternative (dgtlrift/cddlc#136).
            let key = match &r.key_ty {
                TypeRef::Primitive(Primitive::Tstr) => "\"other-key\"".to_owned(),
                TypeRef::Primitive(Primitive::Int) => "int64(1000)".to_owned(),
                TypeRef::Primitive(Primitive::Uint) => "uint64(1000)".to_owned(),
                TypeRef::Named(n) => match module.get(n) {
                    Some(TypeDef::Enum(e)) => match e.variants.iter().find(|v| matches!(v.ty, TypeRef::Primitive(Primitive::Int | Primitive::Uint)) && v.constraints.is_empty()) {
                        Some(v) => format!("New{}{}(1000)", go_type_name(n), go_type_name(&v.name)),
                        None => go_value(&r.key_ty, &Occurrence::Required, &[], module, explicit, depth + 1),
                    },
                    _ => go_value(&r.key_ty, &Occurrence::Required, &[], module, explicit, depth + 1),
                },
                _ => go_value(&r.key_ty, &Occurrence::Required, &[], module, explicit, depth + 1),
            };
            fields.push(format!("{}: map[{}]{}{{{key}: {v}}}", go_field_name(&r.name), rest_key_go(r), go_type(&r.val_ty)));
        }
    }
    format!("{}{{{}}}", go_type_name(&s.name), fields.join(", "))
}

/// A Go literal of primitive type `p`, typed so it can't default to `int`.
fn typed_literal(p: &Primitive, lit: &LiteralValue) -> Option<String> {
    let l = go_literal(lit)?;
    Some(match p {
        Primitive::Tstr | Primitive::Bool => l,
        _ => format!("{}({l})", go_primitive(p)),
    })
}

/// A valid test value: the literal a type is pinned to, a value inside its
/// constraints, or a default/explicit value.
fn go_value(ty: &TypeRef, occ: &Occurrence, constraints: &[Constraint], module: &IrModule, explicit: bool, depth: usize) -> String {
    match occ {
        Occurrence::Optional if !explicit => return "nil".into(),
        // A choice's field is `*any`, so its pointer must be one too.
        Occurrence::Optional if matches!(ty, TypeRef::Choice(_)) => {
            return format!("ptr[any]({})", go_value(ty, &Occurrence::Required, constraints, module, explicit, depth));
        }
        Occurrence::Optional => return format!("ptr({})", go_value(ty, &Occurrence::Required, constraints, module, explicit, depth)),
        // As many items as the member needs (`+`: one, `2*3`: two).
        Occurrence::ZeroOrMore { .. } | Occurrence::OneOrMore { .. } | Occurrence::Bounded { .. } => {
            let item = go_value(ty, &Occurrence::Required, constraints, module, explicit, depth);
            let items: Vec<String> = (0..cddlc_codegen::repeat_bounds(occ).0).map(|_| item.clone()).collect();
            return format!("[]{}{{{}}}", go_type(ty), items.join(", "));
        }
        Occurrence::Required => {}
    }
    if let Some(p) = base_primitive(ty) {
        if let Some(l) = constraints.iter().find_map(|c| match c {
            Constraint::Eq(LiteralValue::Bytes(b)) if matches!(p, Primitive::Bstr | Primitive::Any) => Some(go_bytes_lit(b)),
            Constraint::Eq(v) if literal_fits(v, p) => typed_literal(p, v),
            _ => None,
        }) {
            return l;
        }
        for c in constraints {
            match (c, p) {
                // `.bits`: every allowed bit set (dgtlrift/cddlc#107).
                (Constraint::Bits(b), Primitive::Uint) => return format!("uint64({:#x})", cddlc_codegen::bits_mask_u64(b)),
                (Constraint::Bits(b), Primitive::Bstr) => return go_bytes_lit(&cddlc_codegen::bits_mask_bytes(b)),
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Tstr) => return go_str(&"a".repeat(*n)),
                (Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. }, Primitive::Bstr | Primitive::Any) => return format!("make([]byte, {n})"),
                (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) => return format!("int64({lo})"),
                (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) => return format!("uint64({lo})"),
                // Only an upper bound: the largest value under it.
                (Constraint::ValueRangeInt { min: None, max: Some(hi), inclusive }, Primitive::Int) => {
                    return format!("int64({})", if *inclusive { *hi } else { hi - 1 });
                }
                _ => {}
            }
        }
    }
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::Uint => (if explicit { "uint64(42)" } else { "uint64(0)" }).into(),
            Primitive::Int => (if explicit { "int64(-1)" } else { "int64(0)" }).into(),
            Primitive::Bool => explicit.to_string(),
            // 1.5 is exact as a float32, so it reads back unchanged from any decoder.
            Primitive::Float16 | Primitive::Float32 => (if explicit { "float32(1.5)" } else { "float32(0)" }).into(),
            Primitive::Float64 | Primitive::Float => (if explicit { "float64(1.5)" } else { "float64(0)" }).into(),
            Primitive::Tstr => (if explicit { "\"hello\"" } else { "\"\"" }).into(),
            Primitive::Bstr | Primitive::Any => (if explicit { "[]byte{0x01, 0x02}" } else { "[]byte{}" }).into(),
            Primitive::Null | Primitive::Undefined => "Null{}".into(),
        },
        TypeRef::Tagged(_, inner) => go_value(inner, &Occurrence::Required, constraints, module, explicit, depth),
        // A choice's first alternative (nil would be null, not a valid value).
        TypeRef::Choice(alts) => match alts.first() {
            Some(first) => go_value(first, &Occurrence::Required, &[], module, explicit, depth),
            None => "nil".into(),
        },
        TypeRef::Named(n) => {
            let tn = go_type_name(n);
            if depth > MAX_FIXTURE_DEPTH {
                return format!("{tn}{{}}");
            }
            match module.get(n) {
                Some(TypeDef::Struct(s)) => go_struct_value(s, module, explicit, depth),
                Some(TypeDef::Alias(a)) => format!("{tn}({})", go_value(&a.ty, &Occurrence::Required, &a.constraints, module, explicit, depth + 1)),
                Some(TypeDef::Enum(e)) => e.variants.first().map(|v| go_variant_value(e, v, module, depth)).unwrap_or_else(|| format!("{tn}{{}}")),
                Some(TypeDef::Array(_)) => format!("{tn}{{}}"),
                Some(TypeDef::Map(m)) => go_map_value(m, module, explicit, depth),
                None => format!("{tn}{{}}"),
            }
        }
    }
}

/// A value just outside an alias's constraints, if there is a simple one.
fn go_invalid_value(constraints: &[Constraint], ty: &TypeRef) -> Option<String> {
    let p = base_primitive(ty)?;
    for c in constraints {
        match (c, p) {
            (Constraint::SizeExact(n), Primitive::Tstr) => return Some(go_str(&"a".repeat(n + 1))),
            (Constraint::SizeExact(n), Primitive::Bstr | Primitive::Any) => return Some(format!("make([]byte, {})", n + 1)),
            (Constraint::ValueRangeInt { min: Some(lo), .. }, Primitive::Int) if *lo > i64::MIN => return Some(format!("int64({})", lo - 1)),
            (Constraint::ValueRangeUint { min: Some(lo), .. }, Primitive::Uint) if *lo > 0 => return Some(format!("uint64({})", lo - 1)),
            (Constraint::ValueRangeUint { max: Some(hi), .. }, Primitive::Uint) if *hi < u64::MAX => return Some(format!("uint64({})", hi + 1)),
            // `.bits`: the lowest bit it doesn't allow.
            (Constraint::Bits(b), Primitive::Uint | Primitive::Bstr) => {
                let Some(bad) = (0u64..64).find(|n| !b.contains(n)) else { continue };
                if *p == Primitive::Uint {
                    return Some(format!("uint64({:#x})", 1u64 << bad));
                }
                let mut bytes = vec![0u8; bad as usize / 8 + 1];
                bytes[bad as usize / 8] = 1 << (bad % 8);
                return Some(go_bytes_lit(&bytes));
            }
            _ => {}
        }
    }
    None
}
