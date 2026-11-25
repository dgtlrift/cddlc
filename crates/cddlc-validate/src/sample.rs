//! Example values of a schema's types (dgtlrift/cddlc#110).
//!
//! [`sample`] builds a [`Value`] the type accepts from the IR: literals and
//! `.default`s where the schema gives them, the smallest size or count a
//! constraint allows, and a plain value of each primitive otherwise. Recursive
//! types end at a depth past which optional members are left out, repeated
//! ones get their minimum count, and a choice takes the first alternative that
//! ends. The caller should check the result with [`crate::validate_from`]:
//! some constraints (`.regexp`) aren't solved, only tried.

use base64::Engine as _;
use cddlc_ir::{
    Constraint, IrModule, LiteralValue, Occurrence, Primitive, StructEncoding, TypeDef, TypeRef,
};
use cddlc_ir::types::FieldKey;

use std::collections::HashMap;

use crate::value::Value;

/// What [`sample`] puts in.
#[derive(Debug, Clone, Copy, Default)]
pub struct SampleOptions {
    /// Leave out optional members and give repeated ones their minimum count,
    /// at every depth (by default only past [`SHALLOW_DEPTH`]).
    pub minimal: bool,
}

/// Past this depth, a sample is as small as the schema allows, so that a
/// recursive type ends.
pub const SHALLOW_DEPTH: usize = 4;

/// Past this depth, a type that can't end is an error.
const MAX_DEPTH: usize = 64;

/// An example value of the type `type_name` (the schema's root type if `None`).
pub fn sample(module: &IrModule, type_name: Option<&str>, opts: SampleOptions) -> Result<Value, String> {
    let name = type_name.unwrap_or(module.root.as_str());
    if !module.types.contains_key(name) {
        return Err(format!("no type named '{name}'"));
    }
    let heights = heights(module);
    if heights.get(name).copied().unwrap_or(INF) == INF {
        return Err(format!("'{name}' doesn't end: every choice recurses"));
    }
    Sampler { module, opts, heights }.named(name, 0)
}

const INF: usize = usize::MAX;

/// The depth of the smallest value of each type: optional members left out,
/// repeated ones at their minimum count, the shallowest alternative of each
/// choice. `INF` for a type with no finite value (`r = [uint, r]`).
fn heights(module: &IrModule) -> HashMap<String, usize> {
    let mut h: HashMap<String, usize> = module.types.keys().map(|k| (k.to_string(), INF)).collect();
    loop {
        let mut changed = false;
        for (name, def) in &module.types {
            let new = def_height(def, &h);
            if new < h[name.as_str()] {
                h.insert(name.to_string(), new);
                changed = true;
            }
        }
        if !changed {
            return h;
        }
    }
}

fn min_count(occ: &Occurrence) -> usize {
    match occ {
        Occurrence::Required | Occurrence::OneOrMore { .. } => 1,
        Occurrence::Optional | Occurrence::ZeroOrMore { .. } => 0,
        Occurrence::Bounded { min, .. } => *min as usize,
    }
}

fn plus1(h: usize) -> usize {
    h.saturating_add(1).min(INF)
}

fn def_height(def: &TypeDef, h: &HashMap<String, usize>) -> usize {
    let r = |t: &TypeRef, c: &[Constraint]| ref_height(t, c, h);
    plus1(match def {
        TypeDef::Struct(s) => {
            let fields = s.fields.iter()
                .filter(|f| min_count(&f.occurrence) > 0 && f.default.is_none())
                .map(|f| r(&f.ty, &f.constraints));
            let rest = s.rest.iter()
                .filter(|m| min_count(&m.occurrence) > 0)
                .map(|m| r(&m.key_ty, &[]).max(r(&m.val_ty, &[])));
            fields.chain(rest).max().unwrap_or(0)
        }
        TypeDef::Enum(e) => e.variants.iter().map(|v| r(&v.ty, &v.constraints)).min().unwrap_or(INF),
        TypeDef::Array(a) if min_count(&a.occurrence) == 0 => 0,
        TypeDef::Array(a) => r(&a.element, &[]),
        TypeDef::Map(m) if min_count(&m.occurrence) == 0 => 0,
        TypeDef::Map(m) => r(&m.key_ty, &[]).max(r(&m.val_ty, &[])),
        TypeDef::Alias(a) => r(&a.ty, &a.constraints),
    })
}

fn ref_height(ty: &TypeRef, constraints: &[Constraint], h: &HashMap<String, usize>) -> usize {
    if constraints.iter().any(|c| matches!(c, Constraint::Eq(_) | Constraint::Default(_))) {
        return 0;
    }
    let embedded = constraints.iter().find_map(|c| match c {
        Constraint::CborEmbedded(t) | Constraint::CborSeq(t) => Some(plus1(ref_height(t, &[], h))),
        Constraint::TextEncoded { ty, constraints, .. } => Some(plus1(ref_height(ty, constraints, h))),
        _ => None,
    });
    match ty {
        TypeRef::Primitive(_) => embedded.unwrap_or(0),
        TypeRef::Named(n) => h.get(n.as_str()).copied().unwrap_or(INF),
        TypeRef::Tagged(_, inner) => ref_height(inner, constraints, h),
        TypeRef::Choice(alts) => alts.iter().map(|a| ref_height(a, constraints, h)).min().unwrap_or(INF),
    }
}

struct Sampler<'a> {
    module:  &'a IrModule,
    opts:    SampleOptions,
    heights: HashMap<String, usize>,
}

impl Sampler<'_> {
    fn height(&self, ty: &TypeRef, constraints: &[Constraint]) -> usize {
        ref_height(ty, constraints, &self.heights)
    }

    /// The alternative to sample: the first with a finite value, or once the
    /// sample is to be small, the shallowest.
    fn pick<'t, T>(&self, alts: &'t [T], height: impl Fn(&T) -> usize, depth: usize) -> Option<&'t T> {
        if self.small(depth) {
            alts.iter().filter(|a| height(a) < INF).min_by_key(|a| height(a))
        } else {
            alts.iter().find(|a| height(a) < INF)
        }
    }

    fn small(&self, depth: usize) -> bool {
        self.opts.minimal || depth >= SHALLOW_DEPTH
    }

    /// How many of a repeated item to put in.
    fn count(&self, occ: &Occurrence, depth: usize) -> usize {
        let (min, max) = match occ {
            Occurrence::Required => (1, Some(1)),
            Occurrence::Optional => (0, Some(1)),
            Occurrence::ZeroOrMore { .. } => (0, None),
            Occurrence::OneOrMore { .. } => (1, None),
            Occurrence::Bounded { min, max, .. } => (*min as usize, max.map(|m| m as usize)),
        };
        if self.small(depth) { min } else { min.max(1).min(max.unwrap_or(usize::MAX)) }
    }

    fn named(&self, name: &str, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(format!("'{name}' doesn't end: every choice recurses"));
        }
        let def = self.module.types.get(name)
            .ok_or_else(|| format!("no type named '{name}'"))?;
        let v = match def {
            TypeDef::Struct(s) => {
                let mut entries: Vec<(Value, Value)> = Vec::new();
                let mut items: Vec<Value> = Vec::new();
                for f in &s.fields {
                    let ends = f.default.is_some() || self.height(&f.ty, &f.constraints) < INF;
                    let n = match &f.occurrence {
                        Occurrence::Optional if self.small(depth) || !ends => 0,
                        Occurrence::Optional => 1,
                        occ if !ends => min_count(occ),
                        occ => self.count(occ, depth),
                    };
                    for _ in 0..n {
                        let v = match &f.default {
                            Some(d) => literal(d),
                            None => self.typed(&f.ty, &f.constraints, depth + 1)?,
                        };
                        match s.encoding {
                            StructEncoding::Array => items.push(v),
                            StructEncoding::Map => entries.push((key(&f.key), v)),
                        }
                    }
                }
                // The other entries a map must have (`+ tstr => uint`), under keys
                // its named members don't use.
                if let Some(rest) = &s.rest {
                    for _ in 0..min_count(&rest.occurrence) {
                        let mut k = self.typed(&rest.key_ty, &[], depth + 1)?;
                        let mut i = 0;
                        while entries.iter().any(|(e, _)| *e == k) {
                            i += 1;
                            k = match k {
                                Value::Text(t) => Value::Text(format!("{}{i}", t.trim_end_matches(char::is_numeric))),
                                Value::Uint(n) => Value::Uint(n + 1),
                                Value::Int(n) => Value::Int(n - 1),
                                other => return Err(format!("'{name}': no free key for its other entries ({})", to_diag(&other))),
                            };
                        }
                        let v = self.typed(&rest.val_ty, &[], depth + 1)?;
                        entries.push((k, v));
                    }
                }
                match s.encoding {
                    StructEncoding::Array => Value::Array(items),
                    StructEncoding::Map => Value::Map(entries),
                }
            }
            TypeDef::Enum(e) => {
                let v = self.pick(&e.variants, |v| self.height(&v.ty, &v.constraints), depth)
                    .ok_or_else(|| format!("'{name}' doesn't end: every choice recurses"))?;
                self.typed(&v.ty, &v.constraints, depth + 1)?
            }
            TypeDef::Array(a) => {
                let n = if self.height(&a.element, &[]) < INF { self.count(&a.occurrence, depth) } else { 0 };
                let items: Vec<Value> = (0..n).map(|_| self.typed(&a.element, &[], depth + 1)).collect::<Result<_, _>>()?;
                // A repeated group's members sit inline (dgtlrift/cddlc#90).
                if a.flat {
                    Value::Array(items.into_iter().flat_map(|v| match v {
                        Value::Array(members) => members,
                        other => vec![other],
                    }).collect())
                } else {
                    Value::Array(items)
                }
            }
            TypeDef::Map(m) => {
                let n = self.count(&m.occurrence, depth);
                let mut entries = Vec::new();
                for _ in 0..n {
                    entries.push((self.typed(&m.key_ty, &[], depth + 1)?, self.typed(&m.val_ty, &[], depth + 1)?));
                }
                Value::Map(entries)
            }
            TypeDef::Alias(a) => self.typed(&a.ty, &a.constraints, depth + 1)?,
        };
        Ok(match def.tag() {
            Some(t) => Value::Tag(t.0, Box::new(v)),
            None => v,
        })
    }

    fn typed(&self, ty: &TypeRef, constraints: &[Constraint], depth: usize) -> Result<Value, String> {
        // A literal or default is the example.
        for c in constraints {
            if let Constraint::Eq(l) | Constraint::Default(l) = c {
                return Ok(literal(l));
            }
        }
        match ty {
            TypeRef::Primitive(p) => self.primitive(p, constraints, depth),
            TypeRef::Named(n) => self.named(n, depth),
            TypeRef::Tagged(t, inner) => Ok(Value::Tag(t.0, Box::new(self.typed(inner, constraints, depth)?))),
            TypeRef::Choice(alts) => {
                let a = self.pick(alts, |a| self.height(a, constraints), depth)
                    .ok_or_else(|| "a choice that doesn't end".to_string())?;
                self.typed(a, constraints, depth + 1)
            }
        }
    }

    fn primitive(&self, p: &Primitive, constraints: &[Constraint], depth: usize) -> Result<Value, String> {
        let ne = |v: &Value| constraints.iter().any(|c| matches!(c, Constraint::Ne(l) if literal(l) == *v));
        Ok(match p {
            Primitive::Bool => {
                let v = Value::Bool(false);
                if ne(&v) { Value::Bool(true) } else { v }
            }
            Primitive::Null => Value::Null,
            Primitive::Undefined => Value::Undefined,
            Primitive::Uint | Primitive::Int => {
                let (lo, hi) = int_range(constraints, *p == Primitive::Uint);
                let mut n = 0i128.clamp(lo, hi.max(lo));
                while ne(&int128(n)) && n < hi { n += 1; }
                int128(n)
            }
            Primitive::Float16 | Primitive::Float32 | Primitive::Float64 | Primitive::Float => {
                let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
                for c in constraints {
                    if let Constraint::ValueRangeF64 { min, max, inclusive } = c {
                        if let Some(m) = min { lo = lo.max(*m); }
                        if let Some(m) = max {
                            // An exclusive upper bound: a little below it.
                            hi = hi.min(if *inclusive { *m } else { *m - (m.abs() * 1e-9).max(1e-9) });
                        }
                    }
                }
                let x = 0f64.clamp(lo, hi.max(lo));
                // `float16` / `float32` / `float64` are written at their width
                // (RFC 8610 Appendix D); `float` is any.
                match p {
                    Primitive::Float16 => Value::SizedFloat(x, 16),
                    Primitive::Float32 => Value::SizedFloat(x, 32),
                    Primitive::Float64 => Value::SizedFloat(x, 64),
                    _ => Value::Float(x),
                }
            }
            Primitive::Tstr => {
                // RFC 9741: text holding a value of the operand, encoded.
                for c in constraints {
                    if let Constraint::TextEncoded { encoding, ty, constraints: inner } = c {
                        let v = self.typed(ty, inner, depth + 1)?;
                        return crate::text_encoding::encode(*encoding, &v)
                            .map(Value::Text)
                            .ok_or_else(|| format!("{} of {}", encoding.control(), to_diag(&v)));
                    }
                }
                let n = size(constraints, 4).unwrap_or(4);
                let t: String = "text".chars().cycle().take(n).collect();
                Value::Text(t)
            }
            Primitive::Bstr => {
                for c in constraints {
                    match c {
                        Constraint::CborEmbedded(t) => return Ok(Value::Bytes(to_cbor(&self.typed(t, &[], depth + 1)?))),
                        Constraint::CborSeq(t) => return Ok(Value::Bytes(to_cbor(&self.typed(t, &[], depth + 1)?))),
                        _ => {}
                    }
                }
                Value::Bytes(vec![0; size(constraints, 0).unwrap_or(0)])
            }
            Primitive::Any => Value::Uint(0),
        })
    }
}

/// The size a `.size` constraint asks for: `preferred`, or the nearest it allows.
fn size(constraints: &[Constraint], preferred: usize) -> Option<usize> {
    constraints.iter().find_map(|c| match c {
        Constraint::SizeExact(n) => Some(*n),
        Constraint::SizeRange { min, max } => {
            let lo = min.unwrap_or(0);
            Some(preferred.clamp(lo, max.unwrap_or(usize::MAX).max(lo)))
        }
        _ => None,
    })
}

/// The integers the constraints allow: `[lo, hi]`.
fn int_range(constraints: &[Constraint], unsigned: bool) -> (i128, i128) {
    let (mut lo, mut hi) = (if unsigned { 0 } else { i64::MIN as i128 }, u64::MAX as i128);
    if !unsigned { hi = i64::MAX as i128; }
    for c in constraints {
        let (min, max, inclusive) = match c {
            Constraint::ValueRangeInt { min, max, inclusive } =>
                (min.map(i128::from), max.map(i128::from), *inclusive),
            Constraint::ValueRangeUint { min, max, inclusive } =>
                (min.map(i128::from), max.map(i128::from), *inclusive),
            _ => continue,
        };
        if let Some(m) = min { lo = lo.max(m); }
        if let Some(m) = max { hi = hi.min(if inclusive { m } else { m - 1 }); }
    }
    (lo, hi)
}

fn int128(n: i128) -> Value {
    if n >= 0 { Value::Uint(n as u64) } else { Value::Int(n as i64) }
}

fn int(n: i64) -> Value {
    if n >= 0 { Value::Uint(n as u64) } else { Value::Int(n) }
}

fn key(k: &FieldKey) -> Value {
    match k {
        FieldKey::Text(t) => Value::Text(t.to_string()),
        FieldKey::Int(n) => int(*n),
        FieldKey::Uint(n) => Value::Uint(*n),
    }
}

fn literal(l: &LiteralValue) -> Value {
    match l {
        LiteralValue::Bool(b) => Value::Bool(*b),
        LiteralValue::Null => Value::Null,
        LiteralValue::Uint(n) => Value::Uint(*n),
        LiteralValue::Int(n) => int(*n),
        LiteralValue::Float(x) => Value::Float(*x),
        LiteralValue::Bytes(b) => Value::Bytes(b.clone()),
        LiteralValue::Text(t) => Value::Text(t.clone()),
    }
}

// ── Output ────────────────────────────────────────────────────────────────────

/// The value as CBOR, each float at the width its type has.
pub fn to_cbor(v: &Value) -> Vec<u8> {
    crate::cbor::encode(v)
}

/// The value as JSON (RFC 8949 §6.1): byte strings are base64url text, tags are
/// left out, and integer map keys become text.
pub fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null | Value::Undefined | Value::Simple(_) => J::Null,
        Value::SizedFloat(x, _) => to_json(&Value::Float(*x)),
        Value::Bool(b) => J::Bool(*b),
        Value::Uint(n) => J::from(*n),
        Value::Int(n) => J::from(*n),
        Value::Float(x) => serde_json::Number::from_f64(*x).map(J::Number).unwrap_or(J::Null),
        Value::Text(t) => J::String(t.clone()),
        Value::Bytes(b) => J::String(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)),
        Value::Array(a) => J::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => J::Object(m.iter().map(|(k, v)| (json_key(k), to_json(v))).collect()),
        Value::Tag(_, inner) => to_json(inner),
    }
}

fn json_key(k: &Value) -> String {
    match k {
        Value::Text(t) => t.clone(),
        Value::Uint(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        other => to_diag(other),
    }
}

/// The value in CBOR diagnostic notation (RFC 8949 §8): `{"id": 1, "key": h'00'}`.
pub fn to_diag(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Undefined => "undefined".into(),
        Value::Simple(n) => format!("simple({n})"),
        // RFC 8949 §8.1 encoding indicators: _1, _2, _3 for 16, 32, 64 bits.
        Value::SizedFloat(x, w) => format!("{}_{}", to_diag(&Value::Float(*x)), match w { 16 => 1, 32 => 2, _ => 3 }),
        Value::Bool(b) => b.to_string(),
        Value::Uint(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(x) if x.is_nan() => "NaN".into(),
        Value::Float(x) if x.is_infinite() => if *x > 0.0 { "Infinity".into() } else { "-Infinity".into() },
        Value::Float(x) => {
            let s = format!("{x:?}");
            if s.contains(['.', 'e', 'E']) { s } else { format!("{s}.0") }
        }
        Value::Text(t) => serde_json::Value::String(t.clone()).to_string(),
        Value::Bytes(b) => format!("h'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        Value::Array(a) => format!("[{}]", a.iter().map(to_diag).collect::<Vec<_>>().join(", ")),
        Value::Map(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, v)| format!("{}: {}", to_diag(k), to_diag(v))).collect::<Vec<_>>().join(", ")
        ),
        Value::Tag(t, inner) => format!("{t}({})", to_diag(inner)),
    }
}

/// [`to_json`] as pretty-printed text, with map members in the schema's order
/// (a `serde_json` map would sort them).
pub fn to_json_text(v: &Value) -> String {
    let mut out = String::new();
    write_json(&to_json_ordered(v), 0, &mut out);
    out.push('\n');
    out
}

/// JSON in the schema's order: an object is a list of members.
enum Ordered {
    Scalar(serde_json::Value),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

fn to_json_ordered(v: &Value) -> Ordered {
    match v {
        Value::Array(a) => Ordered::Array(a.iter().map(to_json_ordered).collect()),
        Value::Map(m) => Ordered::Object(m.iter().map(|(k, v)| (json_key(k), to_json_ordered(v))).collect()),
        Value::Tag(_, inner) => to_json_ordered(inner),
        other => Ordered::Scalar(to_json(other)),
    }
}

fn write_json(v: &Ordered, indent: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match v {
        Ordered::Scalar(s) => out.push_str(&s.to_string()),
        Ordered::Array(a) if a.is_empty() => out.push_str("[]"),
        Ordered::Object(m) if m.is_empty() => out.push_str("{}"),
        Ordered::Array(a) => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                write_json(x, indent + 1, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push(']');
        }
        Ordered::Object(m) => {
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                out.push_str(&serde_json::Value::String(k.clone()).to_string());
                out.push_str(": ");
                write_json(x, indent + 1, out);
                out.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
    }
}
