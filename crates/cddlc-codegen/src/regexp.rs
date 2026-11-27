//! A text that a `.regexp` constraint accepts, for generated test values
//! (C# and C++ check `.regexp` with their own regex engines).

use regex_syntax::hir::{Class, Hir, HirKind, Look};

/// The shortest text `pattern` accepts as a whole (CDDL's `.regexp` matches
/// the entire text, as XSD's do): each repetition at its minimum, an
/// alternation's first branch, a class's first character. `None` when the
/// pattern doesn't parse or the text it builds isn't accepted.
pub fn regexp_sample(pattern: &str) -> Option<String> {
    let hir = regex_syntax::Parser::new().parse(pattern).ok()?;
    let mut out = String::new();
    build(&hir, &mut out)?;
    let re = regex::Regex::new(&format!(r"\A(?:{pattern})\z")).ok()?;
    re.is_match(&out).then_some(out)
}

fn build(hir: &Hir, out: &mut String) -> Option<()> {
    match hir.kind() {
        HirKind::Empty => {}
        HirKind::Literal(l) => out.push_str(std::str::from_utf8(&l.0).ok()?),
        HirKind::Class(Class::Unicode(c)) => out.push(c.ranges().first()?.start()),
        HirKind::Class(Class::Bytes(c)) => out.push(char::from(c.ranges().first()?.start())),
        // Anchors and word boundaries add no text.
        HirKind::Look(
            Look::Start | Look::End | Look::StartLF | Look::EndLF | Look::StartCRLF | Look::EndCRLF,
        ) => {}
        HirKind::Look(_) => {}
        HirKind::Repetition(r) => {
            for _ in 0..r.min {
                build(&r.sub, out)?;
            }
        }
        HirKind::Capture(c) => build(&c.sub, out)?,
        HirKind::Concat(hs) => {
            for h in hs {
                build(h, out)?;
            }
        }
        HirKind::Alternation(hs) => build(hs.first()?, out)?,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::regexp_sample;

    #[test]
    fn samples_match_their_pattern() {
        assert_eq!(regexp_sample("[a-z]+[0-9]*").as_deref(), Some("a"));
        assert_eq!(regexp_sample("ab|cd").as_deref(), Some("ab"));
        assert_eq!(regexp_sample(r"\d{3}-\d{4}").as_deref(), Some("000-0000"));
        assert_eq!(regexp_sample("(foo)?bar").as_deref(), Some("bar"));
        assert!(regexp_sample("[").is_none());
    }
}

/// A test value that meets a primitive member's constraints, for a backend
/// to write in its own syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum SampleValue {
    Uint(u64),
    Int(i64),
    Float(f64),
    Text(String),
}

/// A value of `p` meeting `constraints`, when they ask for one other than a
/// plain default: a range's lower bound (or its upper, when it has only
/// that), a text `.size`'s length, a `.regexp`'s [`regexp_sample`]. `None`
/// when there's no such constraint.
pub fn constrained_sample(
    p: &cddlc_ir::Primitive,
    constraints: &[cddlc_ir::Constraint],
) -> Option<SampleValue> {
    use cddlc_ir::{Constraint, Primitive};
    match p {
        Primitive::Tstr => {
            if let Some(s) = constraints.iter().find_map(|c| match c {
                Constraint::Regexp { pattern, .. } => regexp_sample(pattern),
                _ => None,
            }) {
                return Some(SampleValue::Text(s));
            }
            constraints.iter().find_map(|c| match c {
                Constraint::SizeExact(n) | Constraint::SizeRange { min: Some(n), .. } => {
                    Some(SampleValue::Text("a".repeat(*n)))
                }
                _ => None,
            })
        }
        // A `uint`'s bounds may be an int range (`.le 10`) or, past i64, a
        // uint range; and an `int`'s the same.
        Primitive::Uint | Primitive::Int => {
            let (lo, hi, inclusive) = constraints.iter().find_map(|c| match c {
                Constraint::ValueRangeInt {
                    min,
                    max,
                    inclusive,
                } => Some((min.map(i128::from), max.map(i128::from), *inclusive)),
                Constraint::ValueRangeUint {
                    min,
                    max,
                    inclusive,
                } => Some((min.map(i128::from), max.map(i128::from), *inclusive)),
                _ => None,
            })?;
            let v = match (lo, hi) {
                (Some(lo), _) => lo,
                (None, Some(hi)) if inclusive => hi,
                (None, Some(hi)) => hi - 1,
                (None, None) => return None,
            };
            if matches!(p, Primitive::Uint) {
                Some(SampleValue::Uint(u64::try_from(v.max(0)).ok()?))
            } else {
                Some(SampleValue::Int(i64::try_from(v).ok()?))
            }
        }
        Primitive::Float | Primitive::Float16 | Primitive::Float32 | Primitive::Float64 => {
            constraints.iter().find_map(|c| match c {
                Constraint::ValueRangeF64 { min: Some(lo), .. } => Some(SampleValue::Float(*lo)),
                Constraint::ValueRangeF64 {
                    min: None,
                    max: Some(hi),
                    inclusive,
                } => Some(SampleValue::Float(if *inclusive { *hi } else { hi - 1.0 })),
                _ => None,
            })
        }
        _ => None,
    }
}
