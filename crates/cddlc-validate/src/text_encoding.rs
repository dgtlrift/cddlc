//! Decoding the text of RFC 9741's control operators (dgtlrift/cddlc#109):
//! `.b64u`, `.b64c` and their sloppy forms, `.hex`, `.hexlc`, `.hexuc`, `.b32`,
//! `.h32`, `.b45`, `.base10` and `.json`.
//!
//! The operators are strict (RFC 9741 §2.1): each matches only the encoding its
//! defining document mandates. Base64url and base32 have no padding, classic
//! base64 has it, and unused bits must be zero, except that the sloppy forms
//! don't check those bits.

use base64::engine::{general_purpose::GeneralPurposeConfig, DecodePaddingMode, GeneralPurpose};
use base64::{alphabet, Engine as _};
use cddlc_ir::TextEncoding;

use crate::value::Value;

/// The value `text` holds in `encoding`: bytes, an integer, or for `.json`
/// the JSON value. `Err` says why it doesn't decode.
pub fn decode(encoding: TextEncoding, text: &str) -> Result<Value, String> {
    use TextEncoding as E;
    match encoding {
        E::B64u => base64(text, &alphabet::URL_SAFE, DecodePaddingMode::RequireNone, false),
        E::B64uSloppy => base64(text, &alphabet::URL_SAFE, DecodePaddingMode::RequireNone, true),
        E::B64c => base64(text, &alphabet::STANDARD, DecodePaddingMode::RequireCanonical, false),
        E::B64cSloppy => base64(text, &alphabet::STANDARD, DecodePaddingMode::RequireCanonical, true),
        E::Hex => hex(text, |c| c.is_ascii_hexdigit()),
        E::HexLc => hex(text, |c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
        E::HexUc => hex(text, |c| c.is_ascii_digit() || matches!(c, 'A'..='F')),
        E::B32 => base32(text, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"),
        E::H32 => base32(text, b"0123456789ABCDEFGHIJKLMNOPQRSTUV"),
        E::B45 => base45(text),
        E::Base10 => base10(text),
        E::Json => serde_json::from_str::<serde_json::Value>(text)
            .map(Value::from)
            .map_err(|e| format!("not JSON: {e}")),
    }
}

fn base64(text: &str, alpha: &alphabet::Alphabet, padding: DecodePaddingMode, sloppy: bool) -> Result<Value, String> {
    let config = GeneralPurposeConfig::new()
        .with_decode_padding_mode(padding)
        .with_decode_allow_trailing_bits(sloppy);
    GeneralPurpose::new(alpha, config)
        .decode(text)
        .map(Value::Bytes)
        .map_err(|e| format!("not {}: {e}", if padding == DecodePaddingMode::RequireNone {
            "base64url without padding"
        } else {
            "base64 with padding"
        }))
}

fn hex(text: &str, digit: impl Fn(char) -> bool) -> Result<Value, String> {
    if let Some(c) = text.chars().find(|&c| !digit(c)) {
        return Err(format!("'{c}' is not a hex digit allowed here"));
    }
    if text.len() % 2 != 0 {
        return Err("an odd number of hex digits".into());
    }
    Ok(Value::Bytes(
        (0..text.len()).step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("checked digits"))
            .collect(),
    ))
}

/// RFC 4648 §6 and §7, without padding; unused bits must be zero.
fn base32(text: &str, alphabet: &[u8; 32]) -> Result<Value, String> {
    let mut bits: u64 = 0;
    let mut n = 0u32;
    let mut out = Vec::new();
    for c in text.bytes() {
        let v = alphabet.iter().position(|&a| a == c)
            .ok_or_else(|| format!("'{}' is not in the base32 alphabet", c as char))?;
        bits = (bits << 5) | v as u64;
        n += 5;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
            bits &= (1 << n) - 1;
        }
    }
    // The lengths without padding: 0, 2, 4, 5 or 7 characters past a multiple of 8.
    if matches!(text.len() % 8, 1 | 3 | 6) {
        return Err(format!("{} characters is not a base32 length", text.len()));
    }
    if bits != 0 {
        return Err("the unused bits of the last character are not zero".into());
    }
    Ok(Value::Bytes(out))
}

/// RFC 9285: each two bytes are three characters, a last odd byte two.
fn base45(text: &str) -> Result<Value, String> {
    const ALPHABET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
    let digits: Vec<u32> = text.bytes()
        .map(|c| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32)
            .ok_or_else(|| format!("'{}' is not in the base45 alphabet", c as char)))
        .collect::<Result<_, _>>()?;
    if digits.len() % 3 == 1 {
        return Err(format!("{} characters is not a base45 length", digits.len()));
    }
    let mut out = Vec::new();
    for chunk in digits.chunks(3) {
        let n = chunk.iter().rev().fold(0u32, |acc, d| acc * 45 + d);
        if chunk.len() == 3 {
            if n > 0xffff {
                return Err("a group of three characters is more than two bytes".into());
            }
            out.extend([(n >> 8) as u8, n as u8]);
        } else {
            if n > 0xff {
                return Err("the last two characters are more than one byte".into());
            }
            out.push(n as u8);
        }
    }
    Ok(Value::Bytes(out))
}

/// `0|-?[1-9][0-9]*`.
fn base10(text: &str) -> Result<Value, String> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let ok = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" && !text.starts_with('-') || !digits.starts_with('0'));
    if !ok {
        return Err(format!("'{text}' is not a decimal integer (0|-?[1-9][0-9]*)"));
    }
    if text.starts_with('-') {
        text.parse::<i64>().map(Value::Int).map_err(|_| format!("'{text}' is out of range"))
    } else {
        text.parse::<u64>().map(Value::Uint).map_err(|_| format!("'{text}' is out of range"))
    }
}
