//! A CBOR decoder (RFC 8949) for validation (dgtlrift/cddlc#91).
//!
//! It keeps what a generic CBOR library's value model drops and validation
//! needs: `undefined` apart from `null`, the width each float was encoded at
//! (RFC 8610 Appendix D: `float16 = #7.25`, `float32 = #7.26`,
//! `float64 = #7.27`), and duplicate map keys, which make a map invalid
//! (RFC 8949 §5.6) and are reported as an error.

use crate::value::Value;

/// The one data item in `bytes`. Trailing bytes are an error.
pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let mut d = Decoder { bytes, pos: 0, depth: 0 };
    let v = d.item()?;
    if d.pos != bytes.len() {
        return Err(format!("{} bytes after the data item", bytes.len() - d.pos));
    }
    Ok(v)
}

/// The data items in `bytes`, one after another (a CBOR sequence, RFC 8742).
pub fn decode_seq(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let mut d = Decoder { bytes, pos: 0, depth: 0 };
    let mut out = Vec::new();
    while d.pos < bytes.len() {
        out.push(d.item().map_err(|e| format!("item {}: {e}", out.len()))?);
    }
    Ok(out)
}

/// Deeper nesting than this is an error, not a stack overflow.
const MAX_DEPTH: usize = 512;

struct Decoder<'a> {
    bytes: &'a [u8],
    pos:   usize,
    depth: usize,
}

impl Decoder<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| format!("truncated at byte {}", self.pos))?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    /// The argument of a head with additional information `ai`; `None` for an
    /// indefinite length (31).
    fn argument(&mut self, ai: u8) -> Result<Option<u64>, String> {
        Ok(Some(match ai {
            0..=23 => ai as u64,
            24 => self.byte()? as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Ok(None),
            _ => return Err(format!("reserved additional information {ai} at byte {}", self.pos - 1)),
        }))
    }

    fn len(&mut self, n: u64) -> Result<usize, String> {
        usize::try_from(n).ok().filter(|&n| n <= self.bytes.len())
            .ok_or_else(|| format!("length {n} is longer than the input"))
    }

    fn item(&mut self) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let v = self.item_inner();
        self.depth -= 1;
        v
    }

    fn item_inner(&mut self) -> Result<Value, String> {
        let at = self.pos;
        let head = self.byte()?;
        let (major, ai) = (head >> 5, head & 0x1f);
        if major == 7 {
            return self.simple(ai, at);
        }
        let arg = self.argument(ai)?;
        match (major, arg) {
            (0, Some(n)) => Ok(Value::Uint(n)),
            (1, Some(n)) => Ok(match i64::try_from(n) {
                Ok(n) => Value::Int(-1 - n),
                // Below i64: approximate, as the IR can't name such a value.
                Err(_) => Value::Float(-1.0 - n as f64),
            }),
            (2, Some(n)) => { let n = self.len(n)?; Ok(Value::Bytes(self.take(n)?.to_vec())) }
            (3, Some(n)) => {
                let n = self.len(n)?;
                String::from_utf8(self.take(n)?.to_vec())
                    .map(Value::Text)
                    .map_err(|_| format!("text at byte {at} is not UTF-8"))
            }
            (2, None) => Ok(Value::Bytes(self.chunks(2)?)),
            (3, None) => String::from_utf8(self.chunks(3)?)
                .map(Value::Text)
                .map_err(|_| format!("text at byte {at} is not UTF-8")),
            (4, Some(n)) => {
                let n = self.len(n)?;
                (0..n).map(|_| self.item()).collect::<Result<_, _>>().map(Value::Array)
            }
            (4, None) => {
                let mut items = Vec::new();
                while !self.at_break()? {
                    items.push(self.item()?);
                }
                Ok(Value::Array(items))
            }
            (5, n) => {
                let mut entries: Vec<(Value, Value)> = Vec::new();
                let mut i = 0u64;
                loop {
                    match n {
                        Some(n) if i == n => break,
                        None if self.at_break()? => break,
                        _ => {}
                    }
                    let key_at = self.pos;
                    let k = self.item()?;
                    if entries.iter().any(|(e, _)| same_key(e, &k)) {
                        return Err(format!("duplicate map key {} at byte {key_at}", describe(&k)));
                    }
                    let v = self.item()?;
                    entries.push((k, v));
                    i += 1;
                }
                Ok(Value::Map(entries))
            }
            (6, Some(t)) => Ok(Value::Tag(t, Box::new(self.item()?))),
            _ => Err(format!("indefinite length not allowed for major type {major} at byte {at}")),
        }
    }

    /// Major type 7: simple values and floats.
    fn simple(&mut self, ai: u8, at: usize) -> Result<Value, String> {
        Ok(match ai {
            20 => Value::Bool(false),
            21 => Value::Bool(true),
            22 => Value::Null,
            23 => Value::Undefined,
            0..=19 => Value::Simple(ai),
            24 => match self.byte()? {
                n if n < 32 => return Err(format!("simple value {n} in two bytes at byte {at}")),
                n => Value::Simple(n),
            },
            25 => Value::SizedFloat(f16_to_f64(u16::from_be_bytes(self.take(2)?.try_into().unwrap())), 16),
            26 => Value::SizedFloat(f32::from_be_bytes(self.take(4)?.try_into().unwrap()) as f64, 32),
            27 => Value::SizedFloat(f64::from_be_bytes(self.take(8)?.try_into().unwrap()), 64),
            31 => return Err(format!("unexpected break at byte {at}")),
            _ => return Err(format!("reserved simple value {ai} at byte {at}")),
        })
    }

    /// The break code ends an indefinite-length item.
    fn at_break(&mut self) -> Result<bool, String> {
        match self.bytes.get(self.pos) {
            Some(0xff) => { self.pos += 1; Ok(true) }
            Some(_) => Ok(false),
            None => Err("truncated: indefinite-length item has no break".into()),
        }
    }

    /// The chunks of an indefinite-length byte or text string, each a definite
    /// string of the same major type.
    fn chunks(&mut self, major: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        while !self.at_break()? {
            let at = self.pos;
            let head = self.byte()?;
            if head >> 5 != major {
                return Err(format!("a chunk of another type at byte {at}"));
            }
            let n = self.argument(head & 0x1f)?
                .ok_or_else(|| format!("a nested indefinite-length chunk at byte {at}"))?;
            let n = self.len(n)?;
            out.extend_from_slice(self.take(n)?);
        }
        Ok(out)
    }
}

/// Whether two keys are the same data item: equal values of the same type.
fn same_key(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::SizedFloat(x, _), Value::SizedFloat(y, _)) => x == y || (x.is_nan() && y.is_nan()),
        _ => a == b,
    }
}

fn describe(k: &Value) -> String {
    match k {
        Value::Text(t) => format!("{t:?}"),
        Value::Uint(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        other => format!("{other:?}"),
    }
}

/// IEEE 754 half precision.
fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f64;
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 if frac == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + frac / 1024.0) * 2f64.powi(e - 15),
    }
}

/// The value as CBOR, each float at its width (`SizedFloat`), or 64 bits when
/// it has none. The inverse of [`decode`].
pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(v, &mut out);
    out
}

fn head(major: u8, n: u64, out: &mut Vec<u8>) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend([m | 24, n as u8]),
        0x100..=0xffff => { out.push(m | 25); out.extend((n as u16).to_be_bytes()); }
        0x1_0000..=0xffff_ffff => { out.push(m | 26); out.extend((n as u32).to_be_bytes()); }
        _ => { out.push(m | 27); out.extend(n.to_be_bytes()); }
    }
}

fn write(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Uint(n) => head(0, *n, out),
        Value::Int(n) if *n >= 0 => head(0, *n as u64, out),
        Value::Int(n) => head(1, (-1 - *n) as u64, out),
        Value::Bytes(b) => { head(2, b.len() as u64, out); out.extend(b); }
        Value::Text(t) => { head(3, t.len() as u64, out); out.extend(t.as_bytes()); }
        Value::Array(a) => { head(4, a.len() as u64, out); a.iter().for_each(|x| write(x, out)); }
        Value::Map(m) => {
            head(5, m.len() as u64, out);
            for (k, x) in m { write(k, out); write(x, out); }
        }
        Value::Tag(t, inner) => { head(6, *t, out); write(inner, out); }
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Null => out.push(0xf6),
        Value::Undefined => out.push(0xf7),
        Value::Simple(n) if *n < 24 => out.push(0xe0 | n),
        Value::Simple(n) => out.extend([0xf8, *n]),
        Value::SizedFloat(x, 16) => { out.push(0xf9); out.extend(f64_to_f16(*x).to_be_bytes()); }
        Value::SizedFloat(x, 32) => { out.push(0xfa); out.extend((*x as f32).to_be_bytes()); }
        Value::SizedFloat(x, _) | Value::Float(x) => { out.push(0xfb); out.extend(x.to_be_bytes()); }
    }
}

/// IEEE 754 half precision, rounding to nearest even.
fn f64_to_f16(x: f64) -> u16 {
    let f = x as f32;
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 { return sign; }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1 << (shift - 1);
        let mut r = m >> shift;
        let rem = m & ((1 << shift) - 1);
        if rem > half || (rem == half && r & 1 == 1) { r += 1; }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && r & 1 == 1) { r += 1; }
    sign | r as u16
}
