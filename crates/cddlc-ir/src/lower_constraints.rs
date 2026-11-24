/// Constraint lowering.
///
/// Converts `TypeOp` entries from the parser AST into typed [`Constraint`]
/// values on IR nodes.  Also handles the range operator (`..` / `...`).

use cddlc_parser::{CtlOp, MemberKind, Type2, TypeOp, Value};

use crate::error::{IrError, IrWarning, IrWarningKind};
use crate::types::{Constraint, LiteralValue, TypeRef};

/// Lower a single `TypeOp` into one or more `Constraint`s.
///
/// `rule_name` is used only for error messages.
/// `regex_hook` is taken from the `@regex-hook` pragma on the rule, if any.
pub fn lower_type_op(
    op:         &TypeOp,
    base:       &Type2,
    rule_name:  &str,
    regex_hook: Option<&str>,
    warnings:   &mut Vec<IrWarning>,
) -> Result<Vec<Constraint>, IrError> {
    match op {
        TypeOp::Range { upper, inclusive } => {
            lower_range(base, upper, *inclusive, rule_name)
        }

        TypeOp::Control { op, operand } => {
            if let (CtlOp::Size, Some(signed)) = (&op.node, integer_base(base)) {
                // An integer's size bounds its value; with a range of byte
                // counts, the largest one does.
                match size_operand(&operand.node) {
                    Some(SizeOperand::Exact(bytes)) => return Ok(integer_size(bytes, signed)),
                    Some(SizeOperand::Range { max: Some(bytes), .. }) => return Ok(integer_size(bytes, signed)),
                    Some(SizeOperand::Range { max: None, .. }) => return Ok(vec![]),
                    None => {}
                }
            }
            lower_control_op(&op.node, &operand.node, rule_name, regex_hook, warnings)
        }
    }
}

// ── .size on integers ─────────────────────────────────────────────────────────

/// `Some(signed)` when `base` is the prelude `uint` (`false`) or `int` / `number`
/// (`true`), whose `.size` bounds the value rather than a length.
fn integer_base(base: &Type2) -> Option<bool> {
    match base {
        Type2::Typename { name, args } if args.is_empty() => match name.node.as_str() {
            "uint" => Some(false),
            "int" | "number" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

/// A `.size` operand: a byte count, or a range of them.
enum SizeOperand {
    Exact(u64),
    /// `max: None` when the range has no upper bound that fits.
    Range { min: u64, max: Option<u64> },
}

/// A non-negative integer literal.
fn count_literal(t: &Type2) -> Option<u64> {
    match t {
        Type2::Value(Value::Uint(n)) => Some(*n),
        Type2::Value(Value::Int(n)) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

/// The operand of `.size`: `N`, or a parenthesized `(N)`, `(lo..hi)` or
/// `(lo...hi)` (dgtlrift/cddlc#70). The parser reads the parentheses as a
/// group holding one type.
fn size_operand(operand: &Type2) -> Option<SizeOperand> {
    if let Some(n) = count_literal(operand) {
        return Some(SizeOperand::Exact(n));
    }
    let Type2::Group(g) = operand else { return None };
    let [choice] = g.node.choices.as_slice() else { return None };
    let [entry] = choice.entries.as_slice() else { return None };
    if entry.node.occurrence.is_some() { return None; }
    let MemberKind::Type(t) = &entry.node.kind else { return None };
    let [t1] = t.node.choices.as_slice() else { return None };
    let lo = count_literal(&t1.node.base.node)?;
    match t1.node.ops.as_slice() {
        [] => Some(SizeOperand::Exact(lo)),
        [TypeOp::Range { upper, inclusive }] => {
            let hi = count_literal(&upper.node)?;
            let max = if *inclusive { Some(hi) } else { hi.checked_sub(1) };
            Some(SizeOperand::Range { min: lo, max })
        }
        _ => None,
    }
}

/// `.size N` on an integer limits it to what fits in N bytes (RFC 8610 §3.8.1):
/// `0 ..= 256^N - 1` for `uint`, the N-byte two's-complement range for `int`
/// (dgtlrift/cddlc#21). As a value range, every backend checks it as one,
/// instead of as a length (`val.len()` on a `u64`). 8 or more bytes admits
/// every 64-bit value, so it needs no check.
fn integer_size(bytes: u64, signed: bool) -> Vec<Constraint> {
    if bytes >= 8 {
        return vec![];
    }
    let bits = bytes as u32 * 8;
    let c = match (signed, bits) {
        (false, _) => Constraint::ValueRangeUint {
            min: None, max: Some((1u64 << bits) - 1), inclusive: true,
        },
        (true, 0) => Constraint::ValueRangeInt { min: Some(0), max: Some(0), inclusive: true },
        (true, _) => Constraint::ValueRangeInt {
            min: Some(-(1i64 << (bits - 1))), max: Some((1i64 << (bits - 1)) - 1), inclusive: true,
        },
    };
    vec![c]
}

/// Lower a range operator (`lo..hi` / `lo...hi`).
///
/// `base` is the range's lower bound (the `Type1::base` preceding the range
/// operator). When it's a numeric literal it becomes `min`; otherwise (e.g. a
/// typename base we can't statically resolve) `min` stays `None`, matching the
/// existing behaviour for unresolved bounds elsewhere in this module.
fn lower_range(
    base:      &Type2,
    upper:     &cddlc_parser::Spanned<Type2>,
    inclusive: bool,
    rule_name: &str,
) -> Result<Vec<Constraint>, IrError> {
    let min_lit = match base {
        Type2::Value(v) => Some(lower_value(v)),
        _ => None,
    };

    // The upper bound must be a literal value for us to emit a static constraint.
    match &upper.node {
        Type2::Value(v) => {
            let hi = lower_value(v);
            let c = match (min_lit, hi) {
                (Some(LiteralValue::Uint(lo)), LiteralValue::Uint(hi)) =>
                    Constraint::ValueRangeUint { min: Some(lo), max: Some(hi), inclusive },
                // A uint lower bound with a negative upper one: nothing is in it.
                (Some(LiteralValue::Uint(lo)), LiteralValue::Int(hi)) => match i64::try_from(lo) {
                    Ok(lo) => Constraint::ValueRangeInt { min: Some(lo), max: Some(hi), inclusive },
                    Err(_) => empty_uint_range(),
                },
                // An upper bound past i64::MAX bounds no 64-bit int: leave it open
                // rather than wrap it negative (dgtlrift/cddlc#72).
                (Some(LiteralValue::Int(lo)),  LiteralValue::Uint(hi)) =>
                    Constraint::ValueRangeInt  { min: Some(lo), max: i64::try_from(hi).ok(), inclusive },
                (Some(LiteralValue::Int(lo)),  LiteralValue::Int(hi)) =>
                    Constraint::ValueRangeInt  { min: Some(lo), max: Some(hi), inclusive },
                (Some(LiteralValue::Float(lo)), LiteralValue::Float(hi)) =>
                    Constraint::ValueRangeF64  { min: Some(lo), max: Some(hi), inclusive },
                (None, LiteralValue::Uint(hi)) =>
                    Constraint::ValueRangeUint { min: None, max: Some(hi), inclusive },
                (None, LiteralValue::Int(hi)) =>
                    Constraint::ValueRangeInt  { min: None, max: Some(hi), inclusive },
                (None, LiteralValue::Float(hi)) =>
                    Constraint::ValueRangeF64  { min: None, max: Some(hi), inclusive },
                _ => return Err(IrError::BadRangeConstraint {
                    rule:   rule_name.to_owned(),
                    reason: "range bounds must be numeric literals of compatible type".into(),
                }),
            };
            Ok(vec![c])
        }
        _ => Err(IrError::BadRangeConstraint {
            rule:   rule_name.to_owned(),
            reason: "range upper bound must be a literal value".into(),
        }),
    }
}

fn lower_control_op(
    op:         &CtlOp,
    operand:    &Type2,
    rule_name:  &str,
    regex_hook: Option<&str>,
    warnings:   &mut Vec<IrWarning>,
) -> Result<Vec<Constraint>, IrError> {
    match op {
        CtlOp::Size => lower_size(operand, rule_name),

        CtlOp::Lt => numeric_bound(operand, rule_name, BoundSide::Lt),
        CtlOp::Le => numeric_bound(operand, rule_name, BoundSide::Le),
        CtlOp::Gt => numeric_bound(operand, rule_name, BoundSide::Gt),
        CtlOp::Ge => numeric_bound(operand, rule_name, BoundSide::Ge),

        CtlOp::Eq => {
            let v = require_literal(operand, rule_name, ".eq")?;
            Ok(vec![Constraint::Eq(lower_value(v))])
        }
        CtlOp::Ne => {
            let v = require_literal(operand, rule_name, ".ne")?;
            Ok(vec![Constraint::Ne(lower_value(v))])
        }

        CtlOp::Regexp => {
            let pattern = match operand {
                Type2::Value(Value::Text(s)) => s.clone(),
                _ => return Err(IrError::InvalidControlOp {
                    op:   ".regexp".into(),
                    ty:   "non-text operand".into(),
                    rule: rule_name.to_owned(),
                }),
            };
            // Warn if no hook provided — validation silently skipped on target
            if regex_hook.is_none() {
                warnings.push(IrWarning {
                    rule:    Some(rule_name.to_owned()),
                    message: format!(
                        "'.regexp' on '{}' has no @regex-hook pragma — \
                         regex validation will be skipped in generated code",
                        rule_name
                    ),
                    kind: IrWarningKind::NoRegexHook,
                });
            }
            Ok(vec![Constraint::Regexp {
                pattern,
                hook: regex_hook.map(|s| s.to_owned()),
            }])
        }

        CtlOp::Cbor => {
            // .cbor T — operand is a type name or inline type
            let inner = lower_type2_to_typeref(operand, rule_name)?;
            Ok(vec![Constraint::CborEmbedded(Box::new(inner))])
        }

        CtlOp::CborSeq => {
            let inner = lower_type2_to_typeref(operand, rule_name)?;
            Ok(vec![Constraint::CborSeq(Box::new(inner))])
        }

        CtlOp::Default => {
            let v = require_literal(operand, rule_name, ".default")?;
            Ok(vec![Constraint::Default(lower_value(v))])
        }

        CtlOp::Within | CtlOp::And | CtlOp::Bits => {
            // Not emitted as constraints in the IR — they affect type
            // checking only.  Silently ignore for now.
            Ok(vec![])
        }

        CtlOp::Other(name) => {
            warnings.push(IrWarning {
                rule:    Some(rule_name.to_owned()),
                message: format!("unknown control operator '.{name}' — ignored"),
                kind:    IrWarningKind::UnknownPragma,
            });
            Ok(vec![])
        }
    }
}

// ── .size lowering ────────────────────────────────────────────────────────────

fn lower_size(operand: &Type2, rule_name: &str) -> Result<Vec<Constraint>, IrError> {
    match size_operand(operand) {
        Some(SizeOperand::Exact(n)) => return Ok(vec![Constraint::SizeExact(n as usize)]),
        Some(SizeOperand::Range { min, max }) => {
            return Ok(vec![Constraint::SizeRange {
                min: Some(min as usize),
                max: max.map(|m| m as usize),
            }]);
        }
        None => {}
    }
    match operand {
        // .size N  — exact
        Type2::Value(Value::Uint(n)) => {
            Ok(vec![Constraint::SizeExact(*n as usize)])
        }
        // .size lo..hi  or  .size lo...hi — range (encoded as a nested Type1
        // in the parser, but by the time we see Type2 it's a range on the
        // operand).  For now handle the common Uint literal case.
        Type2::Value(Value::Int(n)) if *n >= 0 => {
            Ok(vec![Constraint::SizeExact(*n as usize)])
        }
        // Typename reference — e.g. `.size max-len` where max-len is a uint
        // constant.  We can't resolve this statically without constant folding;
        // emit a warning and skip.
        Type2::Typename { name: _, .. } => {
            // Common prelude constants: uint .size uint — leave unresolved
            Ok(vec![Constraint::SizeRange {
                min: None,
                max: None,
            }])
        }
        _ => Err(IrError::BadSizeConstraint {
            rule:   rule_name.to_owned(),
            reason: "the operand must be a byte count (`.size 8`) or a range of them (`.size (1..8)`)".into(),
        }),
    }
}

// ── Numeric bound helpers ─────────────────────────────────────────────────────

/// `<= n`: an int range when `n` fits an i64, else a uint range.
fn uint_upper(n: u64) -> Constraint {
    match i64::try_from(n) {
        Ok(n) => Constraint::ValueRangeInt { min: None, max: Some(n), inclusive: true },
        Err(_) => Constraint::ValueRangeUint { min: None, max: Some(n), inclusive: true },
    }
}

/// `>= n`: an int range when `n` fits an i64, else a uint range.
fn uint_lower(n: u64) -> Constraint {
    match i64::try_from(n) {
        Ok(n) => Constraint::ValueRangeInt { min: Some(n), max: None, inclusive: true },
        Err(_) => Constraint::ValueRangeUint { min: Some(n), max: None, inclusive: true },
    }
}

/// A range no value is in (`uint .gt 18446744073709551615`).
fn empty_uint_range() -> Constraint {
    Constraint::ValueRangeUint { min: Some(u64::MAX), max: Some(u64::MAX), inclusive: false }
}

enum BoundSide { Lt, Le, Gt, Ge }

fn numeric_bound(
    operand:   &Type2,
    rule_name: &str,
    side:      BoundSide,
) -> Result<Vec<Constraint>, IrError> {
    let v = require_literal(operand, rule_name, "numeric bound")?;

    // Normalise: a uint literal that fits an i64 becomes ValueRangeInt, so it
    // merges with int bounds in the backends; one past i64::MAX stays
    // ValueRangeUint instead of wrapping negative (dgtlrift/cddlc#72).
    let c = match (v, &side) {
        // Nothing is below 0 (for a uint) or above u64::MAX: an empty range.
        (Value::Uint(0), BoundSide::Lt) =>
            Constraint::ValueRangeInt  { min: None, max: Some(-1), inclusive: true },
        (Value::Uint(n), BoundSide::Gt) if *n == u64::MAX => empty_uint_range(),
        (Value::Uint(n), BoundSide::Lt) => uint_upper(*n - 1),
        (Value::Uint(n), BoundSide::Le) => uint_upper(*n),
        (Value::Uint(n), BoundSide::Gt) => uint_lower(*n + 1),
        (Value::Uint(n), BoundSide::Ge) => uint_lower(*n),
        // Int bounds
        // Nothing is below i64::MIN: an empty range rather than an overflow.
        (Value::Int(n),  BoundSide::Lt) if *n == i64::MIN => empty_uint_range(),
        (Value::Int(n),  BoundSide::Lt) => Constraint::ValueRangeInt  { min: None,            max: Some(*n - 1), inclusive: true },
        (Value::Int(n),  BoundSide::Le) => Constraint::ValueRangeInt  { min: None,            max: Some(*n),     inclusive: true },
        (Value::Int(n),  BoundSide::Gt) => Constraint::ValueRangeInt  { min: Some(*n + 1),    max: None,         inclusive: true },
        (Value::Int(n),  BoundSide::Ge) => Constraint::ValueRangeInt  { min: Some(*n),        max: None,         inclusive: true },
        // Float bounds
        (Value::Float(f), BoundSide::Lt) => Constraint::ValueRangeF64 { min: None,            max: Some(*f),     inclusive: false },
        (Value::Float(f), BoundSide::Le) => Constraint::ValueRangeF64 { min: None,            max: Some(*f),     inclusive: true  },
        (Value::Float(f), BoundSide::Gt) => Constraint::ValueRangeF64 { min: Some(*f),        max: None,         inclusive: false },
        (Value::Float(f), BoundSide::Ge) => Constraint::ValueRangeF64 { min: Some(*f),        max: None,         inclusive: true  },
        _ => return Err(IrError::BadRangeConstraint {
            rule:   rule_name.to_owned(),
            reason: "numeric bound operand must be a number literal".into(),
        }),
    };
    Ok(vec![c])
}

// ── Value helpers ─────────────────────────────────────────────────────────────

pub fn lower_value(v: &Value) -> LiteralValue {
    match v {
        Value::Uint(n)   => LiteralValue::Uint(*n),
        Value::Int(n)    => LiteralValue::Int(*n),
        Value::Float(f)  => LiteralValue::Float(*f),
        Value::Text(s)   => LiteralValue::Text(s.clone()),
        Value::Bytes(b)  => LiteralValue::Bytes(b.clone()),
    }
}

fn require_literal<'a>(
    t2:        &'a Type2,
    rule_name: &str,
    op_name:   &str,
) -> Result<&'a Value, IrError> {
    match t2 {
        Type2::Value(v) => Ok(v),
        _ => Err(IrError::InvalidControlOp {
            op:   op_name.to_owned(),
            ty:   "non-literal operand".into(),
            rule: rule_name.to_owned(),
        }),
    }
}

fn lower_type2_to_typeref(t2: &Type2, rule_name: &str) -> Result<TypeRef, IrError> {
    use crate::types::Primitive;
    match t2 {
        Type2::Typename { name, .. } => {
            let n = name.node.as_str();
            if let Some(p) = Primitive::from_prelude(n) {
                Ok(TypeRef::Primitive(p))
            } else {
                Ok(TypeRef::Named(n.into()))
            }
        }
        _ => Err(IrError::InvalidControlOp {
            op:   ".cbor/.cborseq".into(),
            ty:   "complex type operand (only type names supported)".into(),
            rule: rule_name.to_owned(),
        }),
    }
}
