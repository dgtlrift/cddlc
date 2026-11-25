//! Runtime validation of decoded JSON/CBOR data against a resolved
//! [`cddlc_ir::IrModule`].
//!
//! # Prelude types
//!
//! `cddlc-ir` lowers the RFC 8610 Appendix D prelude names to the types the
//! appendix defines them as: `number` is `int / float`, `tdate` is
//! `#6.0(tstr)`, `biguint` is `#6.2(bstr)`, and so on. A bignum that fits in a
//! 64-bit integer reaches the validator as that integer (the CBOR decoder drops
//! its tag), so `biguint` and `bignint` also accept a plain integer.

pub mod value;
pub mod error;
pub mod engine;
pub mod cbor;

pub use error::ValidationError;
pub use value::Value;

use cddlc_ir::IrModule;

/// Where a [`Value`] was decoded from, which decides how tags are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// JSON has no tags: a tagged type matches its untagged payload.
    Json,
    /// CBOR: a tagged type requires its tag (RFC 8610 §3.6).
    Cbor,
}

/// Validate a decoded [`Value`] against the named type in `module`, as
/// [`validate_from`] with [`Source::Json`]: tags are checked when present, and
/// an untagged value matches a tagged type.
///
/// Pass `type_name = None` to validate against the schema's declared root
/// type ([`IrModule::root`]).
pub fn validate(
    module:    &IrModule,
    type_name: Option<&str>,
    value:     &Value,
) -> Result<(), Vec<ValidationError>> {
    validate_from(module, type_name, value, Source::Json)
}

/// Validate a decoded [`Value`] against the named type in `module`. With
/// [`Source::Cbor`], a value of a tagged type must carry its tag.
pub fn validate_from(
    module:    &IrModule,
    type_name: Option<&str>,
    value:     &Value,
    source:    Source,
) -> Result<(), Vec<ValidationError>> {
    let root = type_name.unwrap_or(module.root.as_str());
    engine::validate_named(module, root, value, source == Source::Cbor)
}
