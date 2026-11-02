/// Node.js / TypeScript backend for cddlc.
///
/// Emits a complete npm package:
///   <module>-{cbor,json}/
///     package.json          ← ESM, optional cborg runtime, jest+ts-jest
///     tsconfig.json         ← TypeScript 5 strict
///     src/<module>.ts       ← interfaces + encode/decode functions
///     tests/roundtrip.test.ts ← Jest roundtrip tests
use cddlc_codegen::{
    to_pascal_case, to_snake_case, Backend, CodegenError, CodegenOptions, CodegenOutput,
    GeneratedFile, IndentWriter,
};
use cddlc_ir::{
    AliasDef, ArrayDef, Constraint, EnumDef, EnumVariant, IrModule, LiteralValue, Occurrence,
    Primitive, StructDef, TypeDef, TypeRef,
};

/// Self-contained streaming CBOR encoder emitted into every generated file.
/// cborg v4 exposes only a high-level `encode()` function; there is no
/// streaming `Encoder` class.  This thin class satisfies the generated code's
/// `startMap / text / uint / float32 / writeBytes / finish` interface while
/// keeping cborg only for the decode path.
const CBOR_ENCODER_CLASS: &str = r#"class CborEncoder {
  private _buf: Uint8Array[] = [];

  private _head(major: number, n: number | bigint): void {
    const v = typeof n === 'bigint' ? n : BigInt(n);
    if (v < 24n) {
      this._buf.push(new Uint8Array([(major << 5) | Number(v)]));
    } else if (v < 0x100n) {
      this._buf.push(new Uint8Array([(major << 5) | 24, Number(v)]));
    } else if (v < 0x10000n) {
      this._buf.push(new Uint8Array([(major << 5) | 25, Number(v >> 8n), Number(v & 0xffn)]));
    } else if (v < 0x100000000n) {
      const b = new Uint8Array(5); b[0] = (major << 5) | 26;
      new DataView(b.buffer).setUint32(1, Number(v)); this._buf.push(b);
    } else {
      const b = new Uint8Array(9); b[0] = (major << 5) | 27;
      new DataView(b.buffer).setBigUint64(1, v); this._buf.push(b);
    }
  }

  startMap(n: number): void { this._head(5, n); }
  endMap(): void {}
  startArray(n: number): void { this._head(4, n); }
  endArray(): void {}

  text(s: string): void {
    const b = new TextEncoder().encode(s);
    this._head(3, b.length);
    this._buf.push(b);
  }

  uint(n: bigint | number): void { this._head(0, n); }

  int(n: bigint | number): void {
    const v = typeof n === 'bigint' ? n : BigInt(n);
    if (v >= 0n) { this._head(0, v); } else { this._head(1, -v - 1n); }
  }

  float32(n: number): void {
    const b = new Uint8Array(5); b[0] = 0xfa;
    new DataView(b.buffer).setFloat32(1, n); this._buf.push(b);
  }

  float64(n: number): void {
    const b = new Uint8Array(9); b[0] = 0xfb;
    new DataView(b.buffer).setFloat64(1, n); this._buf.push(b);
  }

  bool(v: boolean): void { this._buf.push(new Uint8Array([v ? 0xf5 : 0xf4])); }
  null(): void { this._buf.push(new Uint8Array([0xf6])); }

  bytes(b: Uint8Array): void { this._head(2, b.length); this._buf.push(b); }
  tag(n: number | bigint): void { this._head(6, n); }
  writeBytes(b: Uint8Array): void { this._buf.push(b); }

  finish(): Uint8Array {
    const len = this._buf.reduce((s, c) => s + c.length, 0);
    const out = new Uint8Array(len); let off = 0;
    for (const c of this._buf) { out.set(c, off); off += c.length; }
    return out;
  }
}"#;

pub struct NodeJsBackend;

impl Backend for NodeJsBackend {
    fn language_name(&self) -> &str {
        "Node.js"
    }
    fn file_extension(&self) -> &str {
        "ts"
    }

    fn generate(
        &self,
        module: &IrModule,
        opts: &CodegenOptions,
    ) -> Result<CodegenOutput, CodegenError> {
        let name = module.name.as_str();
        let fmt_tag = if opts.is_json() { "json" } else { "cbor" };
        let crate_dir = format!("{name}-{fmt_tag}");

        Ok(CodegenOutput {
            files: vec![
                GeneratedFile {
                    path: format!("{crate_dir}/package.json"),
                    content: emit_package_json(name, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/tsconfig.json"),
                    content: emit_tsconfig(),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/src/{name}.ts"),
                    content: emit_source(module, opts),
                },
                GeneratedFile {
                    path: format!("{crate_dir}/tests/roundtrip.test.ts"),
                    content: emit_tests(module, opts),
                },
            ],
        })
    }
}

// ── package.json ──────────────────────────────────────────────────────────────

fn emit_package_json(name: &str, opts: &CodegenOptions) -> String {
    let fmt_tag = if opts.is_json() { "json" } else { "cbor" };
    let deps = if opts.is_json() {
        // JSON and Buffer are Node.js builtins — no extra runtime needed
        r#"  "dependencies": {}"#.to_owned()
    } else {
        format!(
            r#"  "dependencies": {{
    "cborg": "^4.0.0"
  }}"#
        )
    };
    format!(
        r#"{{
  "name": "{name}-{fmt_tag}",
  "version": "0.1.0",
  "description": "@generated by cddlc",
  "type": "module",
  "main": "./dist/src/{name}.js",
  "types": "./dist/src/{name}.d.ts",
  "scripts": {{
    "build": "tsc",
    "test":  "jest"
  }},
{deps},
  "devDependencies": {{
    "@types/jest":    "^29.0.0",
    "jest":           "^29.0.0",
    "ts-jest":        "^29.0.0",
    "typescript":     "^5.0.0"
  }},
  "jest": {{
    "preset":        "ts-jest/presets/default-esm",
    "testEnvironment": "node",
    "extensionsToTreatAsEsm": [".ts"],
    "moduleNameMapper": {{
      "^(\\.{{1,2}}/.+)\\.js$": "$1"
    }},
    "transform": {{
      "^.+\\.tsx?$": ["ts-jest", {{ "useESM": true }}]
    }}
  }}
}}
"#
    )
}

// ── tsconfig.json ─────────────────────────────────────────────────────────────

fn emit_tsconfig() -> String {
    r#"{
  "compilerOptions": {
    "target":           "ES2022",
    "module":           "ES2022",
    "moduleResolution": "bundler",
    "lib":              ["ES2022"],
    "outDir":           "./dist",
    "rootDir":          ".",
    "strict":           true,
    "noUncheckedIndexedAccess": true,
    "exactOptionalPropertyTypes": false,
    "declaration":      true,
    "declarationMap":   true,
    "sourceMap":        true,
    "esModuleInterop":  false,
    "skipLibCheck":     true
  },
  "include": ["src/**/*", "tests/**/*"],
  "exclude": ["node_modules", "dist"]
}
"#
    .to_owned()
}

// ── Source file ───────────────────────────────────────────────────────────────

fn emit_source(module: &IrModule, opts: &CodegenOptions) -> String {
    let mut w = IndentWriter::new(2);

    for l in cddlc_codegen::header_lines(opts) {
        w.line(&format!("// {l}"));
    }
    let fmt_label = if opts.is_json() {
        "JSON (RFC 8259)"
    } else {
        "CBOR (RFC 8949)"
    };
    w.line(&format!("// Format: {fmt_label}"));
    if opts.dcbor {
        w.line("// Encoding: deterministic (dCBOR)");
    }
    w.blank();
    if opts.is_json() {
        // JSON is a Node.js/browser built-in — no imports needed
    } else {
        w.line("import { encode as cborgEncode, decode as cborgDecode } from 'cborg';");
        w.blank();
        // Emit a self-contained streaming CBOR encoder — cborg v4 has no Encoder class.
        for line in CBOR_ENCODER_CLASS.lines() {
            if line.is_empty() {
                w.blank();
            } else {
                w.line(line);
            }
        }
    }
    w.blank();

    // Validation error class
    w.line("/** Thrown when a decoded value fails a CDDL constraint. */");
    w.line("export class ValidationError extends Error {");
    w.indent();
    w.line("constructor(");
    w.indent();
    w.line("public readonly field: string,");
    w.line("message: string,");
    w.dedent();
    w.line(") {");
    w.indent();
    w.line("super(`Validation failed for '${field}': ${message}`);");
    w.line("this.name = 'ValidationError';");
    w.dedent();
    w.line("}");
    w.dedent();
    w.line("}");

    // JSON base64url helpers
    if opts.is_json() {
        w.blank();
        // Portable base64url (RFC 4648 section 5, no padding): no Node.js `Buffer`, so the
        // module runs in browsers too.
        for line in BASE64URL_HELPERS.lines() {
            if line.is_empty() {
                w.blank();
            } else {
                w.line(line);
            }
        }
        w.blank();
        // Decoders check primitive types instead of converting them (issue #2).
        for line in JSON_CHECK_HELPERS.lines() {
            if line.is_empty() {
                w.blank();
            } else {
                w.line(line);
            }
        }
    }

    w.blank();

    // Emit each type
    for (_, def) in &module.types {
        emit_type(&mut w, def, opts);
        w.blank();
    }

    w.finish()
}

fn emit_type(w: &mut IndentWriter, def: &TypeDef, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct(w, s, opts),
        TypeDef::Enum(e) => emit_enum(w, e, opts),
        TypeDef::Array(a) => emit_array(w, a, opts),
        TypeDef::Alias(a) => emit_alias(w, a, opts),
        TypeDef::Map(_) => {}
    }
}

// ── Interface + encode/decode ─────────────────────────────────────────────────

fn emit_struct(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    let iname = to_pascal_case(&s.name);
    let fname = encode_fn_name(&s.name);
    let dname = decode_fn_name(&s.name);

    cddlc_codegen::emit_doc_lines(w, s.doc.as_deref(), cddlc_codegen::DocStyle::Block { open: "/**" });
    if s.deprecated {
        w.line("/** @deprecated */");
    }

    // Interface
    w.line(&format!("export interface {iname} {{"));
    w.indent();
    for f in &s.fields {
        cddlc_codegen::emit_doc_lines(w, f.doc.as_deref(), cddlc_codegen::DocStyle::Block { open: "/**" });
        let opt = if matches!(f.occurrence, Occurrence::Optional) {
            "?"
        } else {
            ""
        };
        let ty = ts_field_type(&f.ty, &f.occurrence, opts);
        let prop = to_snake_case(&f.name);
        w.line(&format!("readonly {prop}{opt}: {ty};"));
    }
    w.dedent();
    w.line("}");
    w.blank();

    if opts.is_json() {
        emit_struct_encode_json(w, s, &iname, &fname, opts);
        w.blank();
        emit_struct_decode_json(w, s, &iname, &dname, opts);
    } else {
        emit_struct_encode_cbor(w, s, &iname, &fname, opts);
        w.blank();
        emit_struct_decode_cbor(w, s, &iname, &dname, opts);
    }
}

fn emit_struct_encode_json(
    w: &mut IndentWriter,
    s: &StructDef,
    iname: &str,
    fname: &str,
    opts: &CodegenOptions,
) {
    let fields: Vec<_> = if opts.dcbor {
        let mut fs: Vec<_> = s.fields.iter().collect();
        fs.sort_by(|a, b| a.name.cmp(&b.name));
        fs
    } else {
        s.fields.iter().collect()
    };

    w.line(&format!(
        "export function {fname}(val: {iname}): Uint8Array {{"
    ));
    w.indent();
    w.line("const _map: Record<string, unknown> = {};");
    for f in &fields {
        let prop = to_snake_case(&f.name);
        let key = f.name.as_str();
        let expr = ts_json_encode_expr(&f.ty, &format!("val.{prop}"), opts);
        match &f.occurrence {
            Occurrence::Optional => {
                w.line(&format!(
                    "if (val.{prop} !== undefined) _map[{key:?}] = {expr};"
                ));
            }
            _ => {
                w.line(&format!("_map[{key:?}] = {expr};"));
            }
        }
    }
    w.line("return new TextEncoder().encode(JSON.stringify(_map));");
    w.dedent();
    w.line("}");
}

fn emit_struct_encode_cbor(
    w: &mut IndentWriter,
    s: &StructDef,
    iname: &str,
    fname: &str,
    opts: &CodegenOptions,
) {
    let fields: Vec<_> = if opts.dcbor {
        let mut fs: Vec<_> = s.fields.iter().collect();
        fs.sort_by(|a, b| a.name.cmp(&b.name));
        fs
    } else {
        s.fields.iter().collect()
    };

    w.line(&format!(
        "export function {fname}(val: {iname}): Uint8Array {{"
    ));
    w.indent();
    w.line("const enc = new CborEncoder();");

    if let Some(tag) = s.tagged {
        w.line(&format!("enc.tag({});", tag.0));
    }

    w.line(&format!("enc.startMap({});", fields.len()));
    for f in &fields {
        let prop = to_snake_case(&f.name);
        w.line(&format!("enc.text({:?});", f.name.as_str()));
        emit_ts_encode_value(w, &f.ty, &format!("val.{prop}"), &f.occurrence, opts);
    }
    w.line("enc.endMap();");
    w.line("return enc.finish();");
    w.dedent();
    w.line("}");
}

/// The local a decoder holds field `prop` in: prefixed, so a field can't share a name
/// with the decoder's own locals (`data`, `raw`, `m`, `map`, `decoded`), whatever the
/// schema calls it.
fn field_local(prop: &str) -> String {
    format!("f_{prop}")
}

fn emit_struct_decode_json(
    w: &mut IndentWriter,
    s: &StructDef,
    iname: &str,
    dname: &str,
    opts: &CodegenOptions,
) {
    w.line(&format!(
        "export function {dname}(data: Uint8Array): {iname} {{"
    ));
    w.indent();
    w.line("const raw: unknown = JSON.parse(new TextDecoder().decode(data));");
    w.line("if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {");
    w.indent();
    w.line("throw new Error('Expected JSON object');");
    w.dedent();
    w.line("}");
    w.line("const m = raw as Record<string, unknown>;");
    w.blank();

    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let local = field_local(&prop);
        let key = f.name.as_str();
        let ty = ts_field_type(&f.ty, &f.occurrence, opts);
        match &f.occurrence {
            Occurrence::Optional => {
                w.line(&format!(
                    "const {local}: {ty} | undefined = m[{key:?}] != null"
                ));
                w.indent();
                let conv = ts_json_decode_expr(&f.ty, &format!("m[{key:?}]"), key, opts);
                w.line(&format!("? {conv}"));
                w.line(": undefined;");
                w.dedent();
            }
            _ => {
                w.line(&format!(
                    "if (m[{key:?}] == null) throw new Error('Missing field {key}');"
                ));
                let conv = ts_json_decode_expr(&f.ty, &format!("m[{key:?}]"), key, opts);
                w.line(&format!("const {local}: {ty} = {conv};"));
                emit_ts_validation(w, &f.constraints, &local, key, opts.is_json());
            }
        }
    }
    w.blank();

    w.line("return {");
    w.indent();
    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let local = field_local(&prop);
        w.line(&format!("{prop}: {local},"));
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
}

fn emit_struct_decode_cbor(
    w: &mut IndentWriter,
    s: &StructDef,
    iname: &str,
    dname: &str,
    opts: &CodegenOptions,
) {
    w.line(&format!(
        "export function {dname}(data: Uint8Array): {iname} {{"
    ));
    w.indent();

    if let Some(tag) = s.tagged {
        w.line("const decoded = cborgDecode(data);");
        w.line("if (!Array.isArray(decoded) || decoded.length < 2) throw new Error('Expected CBOR tag');");
        w.line(&format!(
            "if (decoded[0] !== {}n) throw new Error('Unexpected CBOR tag');",
            tag.0
        ));
        w.line("const map: unknown = decoded[1];");
    } else {
        w.line("const map: unknown = cborgDecode(data);");
    }

    w.line("if (typeof map !== 'object' || map === null || Array.isArray(map)) {");
    w.indent();
    w.line("throw new Error('Expected CBOR map');");
    w.dedent();
    w.line("}");
    w.line("const m = map as Record<string, unknown>;");
    w.blank();

    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let local = field_local(&prop);
        let key = f.name.as_str();
        let ty = ts_field_type(&f.ty, &f.occurrence, opts);
        match &f.occurrence {
            Occurrence::Optional => {
                // Encoder writes CBOR null for absent fields; treat both null and
                // undefined as "absent" so the ternary correctly returns undefined.
                w.line(&format!(
                    "const {local}: {ty} | undefined = m[{:?}] != null",
                    key
                ));
                w.indent();
                let conv = ts_convert_expr(&f.ty, &format!("m[{:?}]", key), opts);
                w.line(&format!("? {conv}"));
                w.line(": undefined;");
                w.dedent();
            }
            _ => {
                w.line(&format!(
                    "if (m[{:?}] == null) throw new Error('Missing field {key}');",
                    key
                ));
                let conv = ts_convert_expr(&f.ty, &format!("m[{:?}]", key), opts);
                w.line(&format!("const {local}: {ty} = {conv};"));
                emit_ts_validation(w, &f.constraints, &local, key, opts.is_json());
            }
        }
    }
    w.blank();

    w.line("return {");
    w.indent();
    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let local = field_local(&prop);
        w.line(&format!("{prop}: {local},"));
    }
    w.dedent();
    w.line("};");
    w.dedent();
    w.line("}");
}

/// base64url encode and decode, without padding, in plain TypeScript.
const BASE64URL_HELPERS: &str = r#"const B64URL = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

function toBase64Url(bytes: Uint8Array): string {
  let out = '';
  for (let i = 0; i < bytes.length; i += 3) {
    const b0 = bytes[i] ?? 0;
    const b1 = bytes[i + 1] ?? 0;
    const b2 = bytes[i + 2] ?? 0;
    const n = (b0 << 16) | (b1 << 8) | b2;
    const chars = Math.min(4, Math.ceil(((bytes.length - i) * 8) / 6));
    for (let j = 0; j < chars; j++) {
      out += B64URL.charAt((n >> (18 - 6 * j)) & 63);
    }
  }
  return out;
}

function fromBase64Url(s: string): Uint8Array {
  const clean = s.replace(/=+$/, '');
  const out = new Uint8Array(Math.floor((clean.length * 6) / 8));
  let bits = 0;
  let acc = 0;
  let k = 0;
  for (const ch of clean) {
    const v = B64URL.indexOf(ch);
    if (v < 0) throw new Error(`Invalid base64url character: ${ch}`);
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[k++] = (acc >> bits) & 0xff;
    }
  }
  return out;
}"#;

/// Type checks for JSON decoders: each returns the value with its type, or throws an
/// error naming where it was (`at`). JSON has no NaN or infinity, so any number is a valid
/// float; `int` and `uint` must also be integers, `uint` at least 0.
const JSON_CHECK_HELPERS: &str = r#"function jsonKind(v: unknown): string {
  return v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v;
}

function expectString(v: unknown, at: string): string {
  if (typeof v !== 'string') throw new Error(`${at}: expected a string, got ${jsonKind(v)}`);
  return v;
}

function expectNumber(v: unknown, at: string): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new Error(`${at}: expected a number, got ${jsonKind(v)}`);
  return v;
}

function expectInt(v: unknown, at: string): number {
  const n = expectNumber(v, at);
  if (!Number.isInteger(n)) throw new Error(`${at}: expected an integer, got ${n}`);
  return n;
}

function expectUint(v: unknown, at: string): number {
  const n = expectInt(v, at);
  if (n < 0) throw new Error(`${at}: expected an unsigned integer, got ${n}`);
  return n;
}

function expectBool(v: unknown, at: string): boolean {
  if (typeof v !== 'boolean') throw new Error(`${at}: expected a boolean, got ${jsonKind(v)}`);
  return v;
}

function expectNull(v: unknown, at: string): null {
  if (v !== null) throw new Error(`${at}: expected null, got ${jsonKind(v)}`);
  return null;
}"#;

/// The literal of a `tstr` variant (`"free-space"` in `"free-space" / "log-distance"`).
fn text_literal(v: &EnumVariant) -> Option<&str> {
    let lit = v.constraints.iter().find_map(|c| match c {
        Constraint::Eq(LiteralValue::Text(t)) => Some(t.as_str()),
        _ => None,
    });
    match &v.ty {
        TypeRef::Primitive(Primitive::Tstr) => lit,
        _ => None,
    }
}

/// The CDDL string of a `tstr` or `null` enum variant: its literal, or (for variants
/// without one) the historical lowercased-name fallback.
fn variant_str(v: &EnumVariant) -> String {
    text_literal(v)
        .map(str::to_owned)
        .unwrap_or_else(|| v.name.to_lowercase().replace('_', "-"))
}

// ── Enum (discriminated union) ────────────────────────────────────────────────

fn emit_enum(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&e.name);
    let fname = encode_fn_name(&e.name);
    let dname = decode_fn_name(&e.name);

    cddlc_codegen::emit_doc_lines(w, e.doc.as_deref(), cddlc_codegen::DocStyle::Block { open: "/**" });
    if e.deprecated {
        w.line("/** @deprecated */");
    }

    // Discriminated union type
    w.line(&format!("export type {type_name} ="));
    w.indent();
    for (i, v) in e.variants.iter().enumerate() {
        let vname = to_pascal_case(&v.name);
        let sep = if i < e.variants.len() - 1 { " |" } else { ";" };
        match &v.ty {
            TypeRef::Primitive(Primitive::Tstr | Primitive::Null) => {
                w.line(&format!("{{ readonly kind: {:?} }}{sep}", vname));
            }
            TypeRef::Primitive(p) => {
                let ts = ts_primitive(p, opts);
                w.line(&format!(
                    "{{ readonly kind: {:?}; readonly value: {ts} }}{sep}",
                    vname
                ));
            }
            TypeRef::Named(n) => {
                let ct = to_pascal_case(n);
                w.line(&format!(
                    "{{ readonly kind: {:?}; readonly value: {ct} }}{sep}",
                    vname
                ));
            }
            _ => {
                w.line(&format!("{{ readonly kind: {:?} }}{sep}", vname));
            }
        }
    }
    w.dedent();
    w.blank();

    // Encode function
    w.line(&format!(
        "export function {fname}(val: {type_name}): Uint8Array {{"
    ));
    w.indent();
    if opts.is_json() {
        w.line("switch (val.kind) {");
        w.indent();
        for v in &e.variants {
            let vname = to_pascal_case(&v.name);
            let cddl_str = variant_str(v);
            w.line(&format!("case {:?}:", vname));
            w.indent();
            match &v.ty {
                TypeRef::Primitive(Primitive::Tstr | Primitive::Null) => {
                    w.line(&format!(
                        "return new TextEncoder().encode(JSON.stringify({cddl_str:?}));"
                    ));
                }
                TypeRef::Named(n) => {
                    w.line("if (!('value' in val)) throw new Error('Missing value');");
                    let ef = encode_fn_name(n);
                    w.line(&format!("return {ef}((val as any).value);"));
                }
                _ => {
                    w.line("if (!('value' in val)) throw new Error('Missing value');");
                    w.line("return new TextEncoder().encode(JSON.stringify((val as any).value));");
                }
            }
            w.dedent();
        }
        w.dedent();
        w.line("}");
        w.line("throw new Error(`Unknown kind: ${(val as any).kind}`);");
    } else {
        w.line("const enc = new CborEncoder();");
        if let Some(tag) = e.tagged {
            w.line(&format!("enc.tag({});", tag.0));
        }
        w.line("switch (val.kind) {");
        w.indent();
        for v in &e.variants {
            let vname = to_pascal_case(&v.name);
            w.line(&format!("case {:?}:", vname));
            w.indent();
            match &v.ty {
                TypeRef::Primitive(Primitive::Tstr | Primitive::Null) => {
                    let s = variant_str(v);
                    w.line(&format!("enc.text({s:?}); break;"));
                }
                TypeRef::Primitive(Primitive::Uint) => {
                    w.line("enc.uint((val as any).value); break;")
                }
                TypeRef::Primitive(Primitive::Int) => w.line("enc.int((val as any).value); break;"),
                TypeRef::Primitive(Primitive::Bool) => {
                    w.line("enc.bool((val as any).value); break;")
                }
                TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => {
                    w.line("enc.float32((val as any).value); break;")
                }
                TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => {
                    w.line("enc.float64((val as any).value); break;")
                }
                TypeRef::Named(n) => {
                    let ef = encode_fn_name(n);
                    w.line(&format!("enc.writeBytes({ef}((val as any).value)); break;"));
                }
                _ => w.line("enc.null(); break;"),
            }
            w.dedent();
        }
        w.dedent();
        w.line("}");
        w.line("return enc.finish();");
    }
    w.dedent();
    w.line("}");
    w.blank();

    // Decode function
    w.line(&format!(
        "export function {dname}(data: Uint8Array): {type_name} {{"
    ));
    w.indent();
    if opts.is_json() {
        w.line("const inner: unknown = JSON.parse(new TextDecoder().decode(data));");
    } else {
        w.line("const val: unknown = cborgDecode(data);");
        if let Some(tag) = e.tagged {
            w.line("if (!Array.isArray(val)) throw new Error('Expected CBOR tag');");
            w.line(&format!(
                "if (val[0] !== {}n) throw new Error('Unexpected CBOR tag');",
                tag.0
            ));
            w.line("const inner: unknown = val[1];");
        } else {
            w.line("const inner: unknown = val;");
        }
    }

    let has_str = e
        .variants
        .iter()
        .any(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr | Primitive::Null)));
    let is_int =
        |v: &&EnumVariant| matches!(&v.ty, TypeRef::Primitive(Primitive::Uint | Primitive::Int));
    let is_float = |v: &&EnumVariant| {
        matches!(
            &v.ty,
            TypeRef::Primitive(
                Primitive::Float | Primitive::Float16 | Primitive::Float32 | Primitive::Float64
            )
        )
    };
    let has_int = e.variants.iter().any(|v| is_int(&v));

    if has_str {
        w.line("if (typeof inner === 'string') {");
        w.indent();
        w.line("switch (inner) {");
        w.indent();
        for v in &e.variants {
            if matches!(&v.ty, TypeRef::Primitive(Primitive::Tstr | Primitive::Null)) {
                let vname = to_pascal_case(&v.name);
                let cddl_str = variant_str(v);
                w.line(&format!("case {cddl_str:?}: return {{ kind: {vname:?} }};"));
            }
        }
        w.line(&format!(
            "default: throw new Error(`Unknown {type_name} variant: ${{inner}}`);"
        ));
        w.dedent();
        w.line("}");
        w.dedent();
        w.line("}");
    }

    if opts.is_json() {
        // JSON has one number type: an integer goes to the integer variant if there is
        // one, any other number to the float variant.
        let int_v = e.variants.iter().find(is_int);
        let float_v = e.variants.iter().find(is_float);
        if int_v.is_some() || float_v.is_some() {
            w.line("if (typeof inner === 'number') {");
            w.indent();
            match (int_v, float_v) {
                (Some(i), Some(f)) => {
                    let (iname, fname_) = (to_pascal_case(&i.name), to_pascal_case(&f.name));
                    w.line(&format!(
                        "if (Number.isInteger(inner)) return {{ kind: {iname:?}, value: inner }};"
                    ));
                    w.line(&format!("return {{ kind: {fname_:?}, value: inner }};"));
                }
                (Some(v), None) | (None, Some(v)) => {
                    let vname = to_pascal_case(&v.name);
                    w.line(&format!("return {{ kind: {vname:?}, value: inner }};"));
                }
                (None, None) => unreachable!(),
            }
            w.dedent();
            w.line("}");
        }
    } else if has_int {
        w.line("if (typeof inner === 'bigint') {");
        w.indent();
        if let Some(v) = e.variants.iter().find(is_int) {
            let vname = to_pascal_case(&v.name);
            w.line(&format!("return {{ kind: {vname:?}, value: inner }};"));
        }
        w.dedent();
        w.line("}");
    }
    if let Some(v) = e
        .variants
        .iter()
        .find(|v| matches!(&v.ty, TypeRef::Primitive(Primitive::Bool)))
    {
        let vname = to_pascal_case(&v.name);
        w.line("if (typeof inner === 'boolean') {");
        w.indent();
        w.line(&format!("return {{ kind: {vname:?}, value: inner }};"));
        w.dedent();
        w.line("}");
    }
    if opts.is_json() {
        // Map and array variants: the first whose decoder accepts the value.
        let named: Vec<&EnumVariant> = e
            .variants
            .iter()
            .filter(|v| matches!(&v.ty, TypeRef::Named(_)))
            .collect();
        if !named.is_empty() {
            w.line("if (typeof inner === 'object' && inner !== null) {");
            w.indent();
            w.line("const bytes = new TextEncoder().encode(JSON.stringify(inner));");
            for v in named {
                let TypeRef::Named(n) = &v.ty else {
                    unreachable!()
                };
                let vname = to_pascal_case(&v.name);
                let df = decode_fn_name(n);
                w.line(&format!("try {{ return {{ kind: {vname:?}, value: {df}(bytes) }}; }} catch {{ /* next variant */ }}"));
            }
            w.dedent();
            w.line("}");
        }
    }

    w.line(&format!("throw new Error(`Cannot decode {type_name}`);"));
    w.dedent();
    w.line("}");
}

// ── Array ─────────────────────────────────────────────────────────────────────

fn emit_array(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);
    let elem_ty = ts_typeref(&a.element, opts);
    let fname = encode_fn_name(&a.name);
    let dname = decode_fn_name(&a.name);

    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Block { open: "/**" });
    if a.deprecated {
        w.line("/** @deprecated */");
    }

    w.line(&format!(
        "export type {type_name} = ReadonlyArray<{elem_ty}>;"
    ));
    w.blank();

    // Encode
    w.line(&format!(
        "export function {fname}(val: {type_name}): Uint8Array {{"
    ));
    w.indent();
    if opts.is_json() {
        let conv = ts_json_encode_expr(&a.element, "item", opts);
        if conv == "item" {
            w.line("return new TextEncoder().encode(JSON.stringify(val));");
        } else {
            w.line(&format!(
                "return new TextEncoder().encode(JSON.stringify(val.map(item => {conv})));"
            ));
        }
    } else if let Some(tag) = a.tagged {
        w.line(&format!("return cborgEncode([{}n, val]);", tag.0));
    } else {
        w.line("return cborgEncode(val);");
    }
    w.dedent();
    w.line("}");
    w.blank();

    // Decode
    w.line(&format!(
        "export function {dname}(data: Uint8Array): {type_name} {{"
    ));
    w.indent();
    if opts.is_json() {
        w.line("const arr: unknown = JSON.parse(new TextDecoder().decode(data));");
        w.line("if (!Array.isArray(arr)) throw new Error('Expected JSON array');");
        let conv = ts_json_decode_expr(&a.element, "item", &format!("{type_name} item"), opts);
        w.line(&format!(
            "return (arr as unknown[]).map(item => {conv}) as {type_name};"
        ));
    } else {
        if let Some(_tag) = a.tagged {
            w.line("const decoded = cborgDecode(data);");
            w.line("if (!Array.isArray(decoded) || decoded[0] !== {}n) throw new Error('Unexpected tag');", );
            w.line("const arr: unknown = decoded[1];");
        } else {
            w.line("const arr: unknown = cborgDecode(data);");
        }
        w.line("if (!Array.isArray(arr)) throw new Error('Expected CBOR array');");
        let conv = ts_convert_expr(&a.element, "item", opts);
        w.line(&format!(
            "return (arr as unknown[]).map(item => {conv}) as {type_name};"
        ));
    }
    w.dedent();
    w.line("}");
}

// ── Alias ─────────────────────────────────────────────────────────────────────

fn emit_alias(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);
    let fname = encode_fn_name(&a.name);
    let dname = decode_fn_name(&a.name);
    let inner_ty = ts_typeref(&a.ty, opts);

    cddlc_codegen::emit_doc_lines(w, a.doc.as_deref(), cddlc_codegen::DocStyle::Block { open: "/**" });
    if a.deprecated {
        w.line("/** @deprecated */");
    }

    // Type alias
    if a.constraints.is_empty() && a.tagged.is_none() {
        w.line(&format!("export type {type_name} = {inner_ty};"));
        w.blank();
        w.line(&format!(
            "export function {fname}(val: {type_name}): Uint8Array {{"
        ));
        w.indent();
        if opts.is_json() {
            let expr = ts_json_encode_expr(&a.ty, "val", opts);
            w.line(&format!(
                "return new TextEncoder().encode(JSON.stringify({expr}));"
            ));
        } else {
            w.line("const enc = new CborEncoder();");
            emit_ts_encode_value(w, &a.ty, "val", &Occurrence::Required, opts);
            w.line("return enc.finish();");
        }
        w.dedent();
        w.line("}");
        w.blank();
        w.line(&format!(
            "export function {dname}(data: Uint8Array): {type_name} {{"
        ));
        w.indent();
        if opts.is_json() {
            w.line("const raw: unknown = JSON.parse(new TextDecoder().decode(data));");
            let conv = ts_json_decode_expr(&a.ty, "raw", &type_name, opts);
            w.line(&format!("return {conv};"));
        } else {
            w.line("const raw: unknown = cborgDecode(data);");
            let conv = ts_convert_expr(&a.ty, "raw", opts);
            w.line(&format!("return {conv};"));
        }
        w.dedent();
        w.line("}");
        return;
    }

    // Branded type for constrained/tagged aliases
    w.line(&format!(
        "export type {type_name} = {inner_ty} & {{ readonly __brand: '{type_name}' }};"
    ));
    w.blank();

    // Validate function
    w.line(&format!(
        "export function validate{type_name}(val: {inner_ty}): {type_name} {{"
    ));
    w.indent();
    for c in &a.constraints {
        if let Some(check) = ts_constraint_check(c, "val", opts.is_json()) {
            let msg = ts_constraint_description(c, opts.is_json());
            w.line(&format!(
                "if (!({check})) throw new ValidationError({type_name:?}, {msg:?});"
            ));
        }
    }
    w.line(&format!("return val as {type_name};"));
    w.dedent();
    w.line("}");
    w.blank();

    // Encode
    w.line(&format!(
        "export function {fname}(val: {type_name}): Uint8Array {{"
    ));
    w.indent();
    if opts.is_json() {
        let expr = ts_json_encode_expr(&a.ty, "val", opts);
        w.line(&format!(
            "return new TextEncoder().encode(JSON.stringify({expr}));"
        ));
    } else {
        w.line("const enc = new CborEncoder();");
        if let Some(tag) = a.tagged {
            w.line(&format!("enc.tag({});", tag.0));
        }
        emit_ts_encode_primitive_or_named(w, &a.ty, "val", opts);
        w.line("return enc.finish();");
    }
    w.dedent();
    w.line("}");
    w.blank();

    // Decode with validation
    w.line(&format!(
        "export function {dname}(data: Uint8Array): {type_name} {{"
    ));
    w.indent();
    if opts.is_json() {
        w.line("const raw: unknown = JSON.parse(new TextDecoder().decode(data));");
        let conv = ts_json_decode_expr(&a.ty, "raw", &type_name, opts);
        w.line(&format!("const val = {conv};"));
    } else if let Some(tag) = a.tagged {
        w.line("const decoded = cborgDecode(data);");
        w.line("if (!Array.isArray(decoded)) throw new Error('Expected CBOR tag');");
        w.line(&format!(
            "if (decoded[0] !== {}n) throw new Error('Unexpected CBOR tag');",
            tag.0
        ));
        w.line("const raw: unknown = decoded[1];");
        let conv = ts_convert_expr(&a.ty, "raw", opts);
        w.line(&format!("const val = {conv};"));
    } else {
        w.line("const raw: unknown = cborgDecode(data);");
        let conv = ts_convert_expr(&a.ty, "raw", opts);
        w.line(&format!("const val = {conv};"));
    }
    w.line(&format!("return validate{type_name}(val);"));
    w.dedent();
    w.line("}");
}

// ── JSON encode/decode expression helpers ─────────────────────────────────────

/// Produce an expression that converts a TS value to its JSON-serializable form.
fn ts_json_encode_expr(ty: &TypeRef, expr: &str, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => format!("toBase64Url({expr})"),
        TypeRef::Named(n) => {
            let ef = encode_fn_name(n);
            // encode returns Uint8Array; convert to JSON string and parse back
            format!("JSON.parse(new TextDecoder().decode({ef}({expr})))")
        }
        TypeRef::Tagged(_, inner) => ts_json_encode_expr(inner, expr, opts),
        _ => expr.to_owned(),
    }
}

/// Produce an expression that converts a JSON-decoded `unknown` value to the correct TS type.
/// The decoding of a JSON value `raw_expr` of type `ty`. `at` names where the value is (a
/// field, an array's items, a type) for type errors.
fn ts_json_decode_expr(ty: &TypeRef, raw_expr: &str, at: &str, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => ts_json_convert_primitive(p, raw_expr, at),
        TypeRef::Named(n) => {
            let df = decode_fn_name(n);
            format!("{df}(new TextEncoder().encode(JSON.stringify({raw_expr})))")
        }
        TypeRef::Tagged(_, inner) => ts_json_decode_expr(inner, raw_expr, at, opts),
        TypeRef::Choice(_) => format!("{raw_expr} as unknown"),
    }
}

/// A checked JSON primitive (issue #2): the value with its type, or an error naming `at`.
fn ts_json_convert_primitive(p: &Primitive, expr: &str, at: &str) -> String {
    match p {
        Primitive::Uint => format!("expectUint({expr}, {at:?})"),
        Primitive::Int => format!("expectInt({expr}, {at:?})"),
        Primitive::Bool => format!("expectBool({expr}, {at:?})"),
        Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float => {
            format!("expectNumber({expr}, {at:?})")
        }
        Primitive::Tstr => format!("expectString({expr}, {at:?})"),
        Primitive::Bstr => format!("fromBase64Url(expectString({expr}, {at:?}))"),
        // `any` keeps its historical form: a base64url string.
        Primitive::Any => format!("fromBase64Url(String({expr}))"),
        Primitive::Null => format!("expectNull({expr}, {at:?})"),
        Primitive::Undefined => "null".into(),
    }
}

// ── CBOR encode helpers ────────────────────────────────────────────────────────

fn emit_ts_encode_value(
    w: &mut IndentWriter,
    ty: &TypeRef,
    expr: &str,
    occ: &Occurrence,
    opts: &CodegenOptions,
) {
    match occ {
        Occurrence::Optional => {
            w.line(&format!("if ({expr} !== undefined) {{"));
            w.indent();
            emit_ts_encode_primitive_or_named(w, ty, expr, opts);
            w.dedent();
            w.line("} else { enc.null(); }");
        }
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => {
            w.line(&format!("enc.startArray({expr}.length);"));
            w.line(&format!("for (const item of {expr}) {{"));
            w.indent();
            emit_ts_encode_primitive_or_named(w, ty, "item", opts);
            w.dedent();
            w.line("}");
            w.line("enc.endArray();");
        }
        Occurrence::Required => {
            emit_ts_encode_primitive_or_named(w, ty, expr, opts);
        }
    }
}

fn emit_ts_encode_primitive_or_named(
    w: &mut IndentWriter,
    ty: &TypeRef,
    expr: &str,
    opts: &CodegenOptions,
) {
    match ty {
        TypeRef::Primitive(p) => {
            let call = ts_encode_call(p, expr);
            w.line(&format!("{call};"));
        }
        TypeRef::Named(n) => {
            let ef = encode_fn_name(n);
            w.line(&format!("enc.writeBytes({ef}({expr}));"));
        }
        TypeRef::Tagged(tag, inner) => {
            w.line(&format!("enc.tag({});", tag.0));
            emit_ts_encode_primitive_or_named(w, inner, expr, opts);
        }
        TypeRef::Choice(_) => {
            w.line(&format!("enc.writeBytes(cborgEncode({expr}));"));
        }
    }
}

fn ts_encode_call(p: &Primitive, expr: &str) -> String {
    match p {
        Primitive::Bool => format!("enc.bool({expr})"),
        Primitive::Null | Primitive::Undefined => "enc.null()".into(),
        Primitive::Uint => format!("enc.uint({expr})"),
        Primitive::Int => format!("enc.int({expr})"),
        Primitive::Float16 => format!("enc.float32(Number({expr}))"),
        Primitive::Float32 => format!("enc.float32({expr})"),
        Primitive::Float64 | Primitive::Float => format!("enc.float64({expr})"),
        Primitive::Tstr => format!("enc.text({expr})"),
        Primitive::Bstr | Primitive::Any => format!("enc.bytes({expr})"),
    }
}

// ── CBOR Convert/decode helpers ───────────────────────────────────────────────

fn emit_ts_validation(
    w: &mut IndentWriter,
    constraints: &[Constraint],
    expr: &str,
    field_name: &str,
    json: bool,
) {
    for c in constraints {
        if let Some(check) = ts_constraint_check(c, expr, json) {
            let msg = ts_constraint_description(c, json);
            w.line(&format!(
                "if (!({check})) throw new ValidationError({field_name:?}, {msg:?});"
            ));
        }
    }
}

fn ts_convert_expr(ty: &TypeRef, raw_expr: &str, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => ts_convert_primitive(p, raw_expr),
        TypeRef::Named(n) => {
            let df = decode_fn_name(n);
            format!("{df}(cborgEncode({raw_expr}))")
        }
        TypeRef::Tagged(_, inner) => ts_convert_expr(inner, raw_expr, opts),
        TypeRef::Choice(_) => format!("{raw_expr} as unknown"),
    }
}

fn ts_convert_primitive(p: &Primitive, expr: &str) -> String {
    match p {
        Primitive::Uint | Primitive::Int => format!("BigInt({expr} as string | number | bigint)"),
        Primitive::Bool => format!("Boolean({expr})"),
        Primitive::Float16 | Primitive::Float32 => format!("Number({expr}) as number"),
        Primitive::Float64 | Primitive::Float => format!("Number({expr}) as number"),
        Primitive::Tstr => format!("String({expr})"),
        Primitive::Bstr | Primitive::Any => format!("{expr} as Uint8Array"),
        Primitive::Null | Primitive::Undefined => "null".into(),
    }
}

// ── Constraint helpers ────────────────────────────────────────────────────────

fn ts_constraint_check(c: &Constraint, expr: &str, json: bool) -> Option<String> {
    // JSON integers are `number`; CBOR integers are `bigint`.
    let n = if json { "" } else { "n" };
    match c {
        Constraint::SizeExact(n) => Some(format!("{expr}.length === {n}")),
        Constraint::SizeRange { min, max } => match (min, max) {
            (Some(lo), Some(hi)) => Some(format!("{expr}.length >= {lo} && {expr}.length <= {hi}")),
            (Some(lo), None) => Some(format!("{expr}.length >= {lo}")),
            (None, Some(hi)) => Some(format!("{expr}.length <= {hi}")),
            (None, None) => None,
        },
        Constraint::ValueRangeInt {
            min,
            max,
            inclusive,
        } => {
            let (ge, le) = if *inclusive { (">=", "<=") } else { (">", "<") };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{expr} {ge} {lo}{n} && {expr} {le} {hi}{n}")),
                (Some(lo), None) => Some(format!("{expr} {ge} {lo}{n}")),
                (None, Some(hi)) => Some(format!("{expr} {le} {hi}{n}")),
                (None, None) => None,
            }
        }
        Constraint::ValueRangeUint {
            min,
            max,
            inclusive,
        } => {
            let (ge, le) = if *inclusive { (">=", "<=") } else { (">", "<") };
            match (min, max) {
                (Some(lo), Some(hi)) => Some(format!("{expr} {ge} {lo}{n} && {expr} {le} {hi}{n}")),
                (Some(lo), None) => Some(format!("{expr} {ge} {lo}{n}")),
                (None, Some(hi)) => Some(format!("{expr} {le} {hi}{n}")),
                (None, None) => None,
            }
        }
        Constraint::Eq(v) => Some(format!("{expr} === {}", ts_literal(v, json))),
        Constraint::Ne(v) => Some(format!("{expr} !== {}", ts_literal(v, json))),
        Constraint::Regexp { hook: Some(h), .. } => Some(format!("{h}({expr})")),
        _ => None,
    }
}

fn ts_constraint_description(c: &Constraint, json: bool) -> String {
    match c {
        Constraint::SizeExact(n) => format!("length must be {n}"),
        Constraint::SizeRange { min, max } => format!("length out of range ({min:?}..{max:?})"),
        Constraint::ValueRangeInt { min, max, .. } => {
            format!("value out of range ({min:?}..{max:?})")
        }
        Constraint::ValueRangeUint { min, max, .. } => {
            format!("value out of range ({min:?}..{max:?})")
        }
        Constraint::Eq(v) => format!("must equal {}", ts_literal(v, json)),
        Constraint::Ne(v) => format!("must not equal {}", ts_literal(v, json)),
        _ => "constraint violated".into(),
    }
}

fn ts_literal(v: &LiteralValue, json: bool) -> String {
    let suffix = if json { "" } else { "n" };
    match v {
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Null => "null".into(),
        LiteralValue::Uint(n) => format!("{n}{suffix}"),
        LiteralValue::Int(n) => format!("{n}{suffix}"),
        LiteralValue::Float(f) => format!("{f}"),
        LiteralValue::Text(s) => format!("{s:?}"),
        LiteralValue::Bytes(_) => "new Uint8Array()".into(),
    }
}

// ── TypeScript type helpers ───────────────────────────────────────────────────

fn ts_field_type(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    let base = ts_typeref(ty, opts);
    match occ {
        Occurrence::Optional => base, // field marked with `?` in interface
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => format!("ReadonlyArray<{base}>"),
        Occurrence::Required => base,
    }
}

fn ts_typeref(ty: &TypeRef, opts: &CodegenOptions) -> String {
    match ty {
        TypeRef::Primitive(p) => ts_primitive(p, opts).to_owned(),
        TypeRef::Named(n) => to_pascal_case(n),
        TypeRef::Tagged(_, inner) => ts_typeref(inner, opts),
        TypeRef::Choice(cs) => {
            if cs.is_empty() {
                return "unknown".into();
            }
            cs.iter()
                .map(|c| ts_typeref(c, opts))
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }
}

fn ts_primitive(p: &Primitive, opts: &CodegenOptions) -> &'static str {
    match p {
        Primitive::Bool => "boolean",
        Primitive::Null | Primitive::Undefined => "null",
        Primitive::Uint | Primitive::Int => {
            if opts.is_json() {
                "number"
            } else {
                "bigint"
            }
        }
        Primitive::Float16 | Primitive::Float32 => "number",
        Primitive::Float64 | Primitive::Float => "number",
        Primitive::Tstr => "string",
        Primitive::Bstr | Primitive::Any => "Uint8Array",
    }
}

// ── Name helpers ──────────────────────────────────────────────────────────────

fn encode_fn_name(name: &str) -> String {
    format!("encode{}", to_pascal_case(name))
}

fn decode_fn_name(name: &str) -> String {
    format!("decode{}", to_pascal_case(name))
}

// ── Test file ─────────────────────────────────────────────────────────────────

fn emit_tests(module: &IrModule, opts: &CodegenOptions) -> String {
    let name = module.name.as_str();
    let mut w = IndentWriter::new(2);

    for l in cddlc_codegen::header_lines(opts) {
        w.line(&format!("// {l}"));
    }
    w.blank();

    // Build import list
    let mut imports: Vec<String> = vec!["ValidationError".into()];
    for (_, def) in &module.types {
        let n = def.name();
        imports.push(to_pascal_case(n));
        imports.push(encode_fn_name(n));
        imports.push(decode_fn_name(n));
        if matches!(def, TypeDef::Alias(a) if !a.constraints.is_empty()) {
            imports.push(format!("validate{}", to_pascal_case(n)));
        }
    }
    imports.dedup();
    w.line(&format!(
        "import {{ {} }} from '../src/{name}.js';",
        imports.join(", ")
    ));
    w.blank();

    for (_, def) in &module.types {
        emit_type_tests(&mut w, def, opts);
        w.blank();
    }

    w.finish()
}

fn emit_type_tests(w: &mut IndentWriter, def: &TypeDef, opts: &CodegenOptions) {
    match def {
        TypeDef::Struct(s) => emit_struct_tests(w, s, opts),
        TypeDef::Enum(e) => emit_enum_tests(w, e, opts),
        TypeDef::Alias(a) => emit_alias_tests(w, a, opts),
        TypeDef::Array(a) => emit_array_tests(w, a, opts),
        TypeDef::Map(_) => {}
    }
}

fn emit_struct_tests(w: &mut IndentWriter, s: &StructDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&s.name);
    let fname = encode_fn_name(&s.name);
    let dname = decode_fn_name(&s.name);

    w.line(&format!("describe('{type_name}', () => {{"));
    w.indent();

    w.line("test('roundtrip default values', () => {");
    w.indent();
    w.line(&format!("const original: {type_name} = {{"));
    w.indent();
    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let val = ts_default_value(&f.ty, &f.occurrence, opts);
        w.line(&format!("{prop}: {val},"));
    }
    w.dedent();
    w.line("};");
    w.line(&format!("const encoded = {fname}(original);"));
    w.line(&format!("const decoded = {dname}(encoded);"));
    emit_ts_roundtrip_assert(w, s);
    w.dedent();
    w.line("});");
    w.blank();

    w.line("test('roundtrip explicit values', () => {");
    w.indent();
    w.line(&format!("const original: {type_name} = {{"));
    w.indent();
    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        let val = ts_explicit_value(&f.ty, &f.occurrence, opts);
        w.line(&format!("{prop}: {val},"));
    }
    w.dedent();
    w.line("};");
    w.line(&format!("const encoded = {fname}(original);"));
    w.line(&format!("const decoded = {dname}(encoded);"));
    emit_ts_roundtrip_assert(w, s);
    w.dedent();
    w.line("});");

    if let Some(tag) = s.tagged {
        w.blank();
        w.line(&format!("test('encode contains tag {}', () => {{", tag.0));
        w.indent();
        w.line(&format!("const val: {type_name} = {{"));
        w.indent();
        for f in &s.fields {
            let prop = to_snake_case(&f.name);
            let v = ts_default_value(&f.ty, &f.occurrence, opts);
            w.line(&format!("{prop}: {v},"));
        }
        w.dedent();
        w.line("};");
        w.line(&format!("const encoded = {fname}(val);"));
        w.line("expect(encoded.length).toBeGreaterThan(0);");
        w.dedent();
        w.line("});");
    }

    w.dedent();
    w.line("});");
}

fn emit_ts_roundtrip_assert(w: &mut IndentWriter, s: &StructDef) {
    for f in &s.fields {
        let prop = to_snake_case(&f.name);
        w.line(&format!("expect(decoded.{prop}).toEqual(original.{prop});"));
    }
}

fn emit_enum_tests(w: &mut IndentWriter, e: &EnumDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&e.name);
    let fname = encode_fn_name(&e.name);
    let dname = decode_fn_name(&e.name);

    w.line(&format!("describe('{type_name}', () => {{"));
    w.indent();

    for v in &e.variants {
        let vname = to_pascal_case(&v.name);
        let construct = match &v.ty {
            TypeRef::Primitive(Primitive::Tstr | Primitive::Null) => {
                format!("{{ kind: {vname:?} }} as {type_name}")
            }
            TypeRef::Primitive(Primitive::Uint) => {
                if opts.is_json() {
                    format!("{{ kind: {vname:?}, value: 42 }} as {type_name}")
                } else {
                    format!("{{ kind: {vname:?}, value: 42n }} as {type_name}")
                }
            }
            TypeRef::Primitive(Primitive::Int) => {
                if opts.is_json() {
                    format!("{{ kind: {vname:?}, value: -1 }} as {type_name}")
                } else {
                    format!("{{ kind: {vname:?}, value: -1n }} as {type_name}")
                }
            }
            TypeRef::Primitive(Primitive::Bool) => {
                format!("{{ kind: {vname:?}, value: false }} as {type_name}")
            }
            _ => format!("{{ kind: {vname:?} }} as {type_name}"),
        };

        w.line(&format!("test('roundtrip {vname}', () => {{"));
        w.indent();
        w.line(&format!("const original: {type_name} = {construct};"));
        w.line(&format!("const decoded = {dname}({fname}(original));"));
        w.line("expect(decoded.kind).toEqual(original.kind);");
        w.dedent();
        w.line("});");
    }

    w.dedent();
    w.line("});");
}

fn emit_alias_tests(w: &mut IndentWriter, a: &AliasDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);
    let fname = encode_fn_name(&a.name);
    let dname = decode_fn_name(&a.name);

    w.line(&format!("describe('{type_name}', () => {{"));
    w.indent();

    let valid_val = ts_valid_constrained_value(&a.constraints, &a.ty, opts);
    w.line("test('roundtrip valid value', () => {");
    w.indent();
    if a.constraints.is_empty() && a.tagged.is_none() {
        w.line(&format!("const original = {valid_val};"));
        w.line(&format!("const decoded = {dname}({fname}(original));"));
        w.line("expect(decoded).toEqual(original);");
    } else {
        w.line(&format!(
            "const original = validate{type_name}({valid_val});"
        ));
        w.line(&format!("const decoded = {dname}({fname}(original));"));
        w.line("expect(decoded).toEqual(original);");
    }
    w.dedent();
    w.line("});");

    if !a.constraints.is_empty() {
        w.blank();
        let bad_val = ts_invalid_constrained_value(&a.constraints, &a.ty, opts);
        w.line("test('rejects invalid value', () => {");
        w.indent();
        w.line(&format!(
            "expect(() => validate{type_name}({bad_val})).toThrow(ValidationError);"
        ));
        w.dedent();
        w.line("});");
    }

    w.dedent();
    w.line("});");
}

fn emit_array_tests(w: &mut IndentWriter, a: &ArrayDef, opts: &CodegenOptions) {
    let type_name = to_pascal_case(&a.name);
    let fname = encode_fn_name(&a.name);
    let dname = decode_fn_name(&a.name);

    w.line(&format!("describe('{type_name}', () => {{"));
    w.indent();

    w.line("test('roundtrip empty array', () => {");
    w.indent();
    w.line(&format!("const original: {type_name} = [];"));
    w.line(&format!("const decoded = {dname}({fname}(original));"));
    w.line("expect(decoded).toEqual(original);");
    w.dedent();
    w.line("});");

    w.dedent();
    w.line("});");

    let _ = opts; // suppress unused warning
}

// ── Test value helpers ────────────────────────────────────────────────────────

fn ts_default_value(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::Optional => "undefined".into(),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => "[]".into(),
        Occurrence::Required => match ty {
            TypeRef::Primitive(Primitive::Uint | Primitive::Int) => {
                if opts.is_json() {
                    "0".into()
                } else {
                    "0n".into()
                }
            }
            TypeRef::Primitive(Primitive::Bool) => "false".into(),
            TypeRef::Primitive(
                Primitive::Float32 | Primitive::Float16 | Primitive::Float64 | Primitive::Float,
            ) => "0".into(),
            TypeRef::Primitive(Primitive::Tstr) => "''".into(),
            TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => "new Uint8Array()".into(),
            TypeRef::Primitive(Primitive::Null | Primitive::Undefined) => "null".into(),
            TypeRef::Named(n) => format!("{{}} as {}", to_pascal_case(n)),
            _ => "null".into(),
        },
    }
}

fn ts_explicit_value(ty: &TypeRef, occ: &Occurrence, opts: &CodegenOptions) -> String {
    match occ {
        Occurrence::Optional => "undefined".into(),
        Occurrence::ZeroOrMore { .. }
        | Occurrence::OneOrMore { .. }
        | Occurrence::Bounded { .. } => "[]".into(),
        Occurrence::Required => match ty {
            TypeRef::Primitive(Primitive::Uint) => {
                if opts.is_json() {
                    "42".into()
                } else {
                    "42n".into()
                }
            }
            TypeRef::Primitive(Primitive::Int) => {
                if opts.is_json() {
                    "-1".into()
                } else {
                    "-1n".into()
                }
            }
            TypeRef::Primitive(Primitive::Bool) => "true".into(),
            TypeRef::Primitive(Primitive::Float32 | Primitive::Float16) => "3.14".into(),
            TypeRef::Primitive(Primitive::Float64 | Primitive::Float) => "3.14".into(),
            TypeRef::Primitive(Primitive::Tstr) => "'hello'".into(),
            TypeRef::Primitive(Primitive::Bstr | Primitive::Any) => {
                "new Uint8Array([0x01, 0x02])".into()
            }
            _ => ts_default_value(ty, occ, opts),
        },
    }
}

fn ts_valid_constrained_value(
    constraints: &[Constraint],
    ty: &TypeRef,
    opts: &CodegenOptions,
) -> String {
    for c in constraints {
        match c {
            Constraint::SizeExact(n) => match ty {
                TypeRef::Primitive(Primitive::Tstr) => return format!("'a'.repeat({})", n),
                TypeRef::Primitive(Primitive::Bstr) => return format!("new Uint8Array({})", n),
                _ => {}
            },
            Constraint::ValueRangeInt { min, .. } => {
                if let Some(lo) = min {
                    return if opts.is_json() {
                        format!("{lo}")
                    } else {
                        format!("{lo}n")
                    };
                }
            }
            Constraint::ValueRangeUint { min, .. } => {
                if let Some(lo) = min {
                    return if opts.is_json() {
                        format!("{lo}")
                    } else {
                        format!("{lo}n")
                    };
                }
            }
            _ => {}
        }
    }
    ts_default_value(ty, &Occurrence::Required, opts)
}

fn ts_invalid_constrained_value(
    constraints: &[Constraint],
    ty: &TypeRef,
    opts: &CodegenOptions,
) -> String {
    for c in constraints {
        match c {
            Constraint::SizeExact(n) => match ty {
                TypeRef::Primitive(Primitive::Tstr) => {
                    return format!("'a'.repeat({} + 1) as any", n)
                }
                TypeRef::Primitive(Primitive::Bstr) => {
                    return format!("new Uint8Array({} + 1) as any", n)
                }
                _ => {}
            },
            Constraint::ValueRangeInt { min, .. } => {
                if let Some(lo) = min {
                    return if opts.is_json() {
                        format!("{} as any", lo - 1)
                    } else {
                        format!("{}n as any", lo - 1)
                    };
                }
            }
            Constraint::ValueRangeUint { min, .. } => {
                if let Some(lo) = min {
                    if *lo > 0 {
                        return if opts.is_json() {
                            format!("{} as any", lo - 1)
                        } else {
                            format!("{}n as any", lo - 1)
                        };
                    }
                }
            }
            _ => {}
        }
    }
    "null as any".into()
}
