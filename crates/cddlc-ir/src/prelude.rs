/// CDDL standard prelude (RFC 8610, Appendix D).
///
/// These names are always in scope and resolve to primitives without needing
/// a rule definition.  The resolver checks this set before emitting
/// `UndefinedType` errors.

use std::collections::HashSet;

use crate::types::{CborTag, Primitive, TypeRef};

/// Returns the set of all standard prelude type names.
pub fn prelude_names() -> HashSet<&'static str> {
    [
        // Core primitives
        "any", "uint", "int", "bstr", "bytes", "tstr", "text",
        "bool", "nil", "null", "undefined", "true", "false",
        // Float variants
        "float16", "float32", "float64", "float",
        "float16-32",   // float16 | float32
        "float32-64",   // float32 | float64
        "bignint",      // #6.3(bstr)
        "integer",      // int | bigint
        "unsigned",     // uint | biguint
        "decfrac",      // #6.4([e10: int, m: integer])
        // Numeric
        "number",       // uint | int | float
        // Byte string aliases
        "b64legacy",    // base64-encoded byte string
        // RFC 8949 well-known tags
        "tdate",        // #6.0(tstr)  — date/time string
        "time",         // #6.1(number) — epoch-based date/time
        "number",       // numeric
        "biguint",      // #6.2(bstr)
        "bigint",       // biguint / bignint
        "bigfloat",     // #6.5(array)
        "eb64url",      // #6.21
        "eb64legacy",   // #6.22
        "eb16",         // #6.23
        "encoded-cbor", // #6.24(bstr)
        "uri",          // #6.32(tstr)
        "b64url",       // #6.33(tstr)
        "b64legacy",    // #6.34(tstr)
        "regexp",       // #6.35(tstr)
        "mime-message", // #6.36(tstr)
        "cbor-any",     // #6.55799(any)
    ]
    .iter()
    .copied()
    .collect()
}

/// Returns true if `name` is a CDDL prelude identifier.
pub fn is_prelude(name: &str) -> bool {
    // Fast path for the most common cases
    matches!(
        name,
        "any" | "uint" | "int" | "bstr" | "bytes" | "tstr" | "text"
        | "bool" | "nil" | "null" | "undefined" | "true" | "false"
        | "float16" | "float32" | "float64" | "float"
        | "number" | "b64legacy" | "tdate" | "time"
        | "biguint" | "bigint" | "bigfloat"
        | "eb64url" | "eb64legacy" | "eb16"
        | "encoded-cbor" | "uri" | "b64url" | "regexp"
        | "mime-message" | "cbor-any" | "float16-32"
        | "float32-64" | "bignint" | "integer" | "unsigned" | "decfrac"
    )
}

/// The type a prelude name stands for, as RFC 8610 Appendix D defines it, for
/// the names that aren't a single [`Primitive`]: choices (`number = int /
/// float`) and tagged types (`tdate = #6.0(tstr)`). `None` for any other name.
pub fn prelude_typeref(name: &str) -> Option<TypeRef> {
    use Primitive::*;
    let p = TypeRef::Primitive;
    let tag = |n: u64, t: TypeRef| TypeRef::Tagged(CborTag(n), Box::new(t));
    let number = || TypeRef::Choice(vec![p(Int), p(Float)]);
    Some(match name {
        "number"       => number(),
        "float16-32"   => TypeRef::Choice(vec![p(Float16), p(Float32)]),
        "float32-64"   => TypeRef::Choice(vec![p(Float32), p(Float64)]),
        "tdate"        => tag(0, p(Tstr)),
        "time"         => tag(1, number()),
        "biguint"      => tag(2, p(Bstr)),
        "bignint"      => tag(3, p(Bstr)),
        "bigint"       => TypeRef::Choice(vec![tag(2, p(Bstr)), tag(3, p(Bstr))]),
        "unsigned"     => TypeRef::Choice(vec![p(Uint), tag(2, p(Bstr))]),
        "integer"      => TypeRef::Choice(vec![p(Int), tag(2, p(Bstr)), tag(3, p(Bstr))]),
        // [exponent, mantissa] arrays: the array isn't modelled inline.
        "decfrac"      => tag(4, p(Any)),
        "bigfloat"     => tag(5, p(Any)),
        "eb64url"      => tag(21, p(Any)),
        "eb64legacy"   => tag(22, p(Any)),
        "eb16"         => tag(23, p(Any)),
        "encoded-cbor" => tag(24, p(Bstr)),
        "uri"          => tag(32, p(Tstr)),
        "b64url"       => tag(33, p(Tstr)),
        "b64legacy"    => tag(34, p(Tstr)),
        "regexp"       => tag(35, p(Tstr)),
        "mime-message" => tag(36, p(Tstr)),
        "cbor-any"     => tag(55799, p(Any)),
        _ => return None,
    })
}
