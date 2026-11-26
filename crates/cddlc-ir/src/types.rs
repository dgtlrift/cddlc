/// Target-agnostic Intermediate Representation.
///
/// After semantic analysis the entire rule set is expressed as an ordered
/// collection of [`TypeDef`] nodes.  Every reference is fully resolved; no
/// names are left dangling.  Generics have been monomorphized.  Pragmas have
/// been consumed into typed fields (capacity, constraints, tags, etc.).
///
/// This is the sole input to all code-generation backends.
use indexmap::IndexMap;
use smol_str::SmolStr;

// ── Module ────────────────────────────────────────────────────────────────────

/// A fully resolved IR module, ready for code generation.
#[derive(Debug, Clone)]
pub struct IrModule {
    /// Crate / module name (derived from the entry-point filename).
    pub name: SmolStr,

    /// The schema's root type: the name of the first rule declared in the
    /// source file, per RFC 8610's "first rule is the root" convention.
    /// Empty if the module defines no rules.
    pub root: SmolStr,

    /// Topologically sorted type definitions.
    /// Every [`TypeRef::Named`] points to a key present in this map.
    pub types: IndexMap<SmolStr, TypeDef>,

    /// Where each type and field was written (dgtlrift/cddlc#9).
    pub sources: Sources,
}

/// Where each type and field of a module was written, as the parser saw it
/// (dgtlrift/cddlc#9). A type made from an inline field (`<rule>-<field>`) has that
/// field's source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    /// By type name.
    pub types: IndexMap<SmolStr, SourceRef>,
    /// By type name and field name.
    pub fields: IndexMap<(SmolStr, SmolStr), SourceRef>,
}

/// A CDDL source line: its file, its 1-based line, and its text, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub text: String,
}

impl SourceRef {
    /// `cddl: <path>:<line>: <text>`, the path relative to `base` when it is under it.
    pub fn describe(&self, base: Option<&std::path::Path>) -> String {
        let path = base
            .and_then(|b| self.path.strip_prefix(b).ok())
            .unwrap_or(&self.path);
        format!("cddl: {}:{}: {}", path.display(), self.line, self.text)
    }
}

impl From<&cddlc_parser::SourceLine> for SourceRef {
    fn from(s: &cddlc_parser::SourceLine) -> Self {
        Self {
            path: s.path.clone(),
            line: s.line,
            text: s.text.clone(),
        }
    }
}

impl IrModule {
    pub fn new(name: impl Into<SmolStr>, root: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
            types: IndexMap::new(),
            sources: Sources::default(),
        }
    }

    pub fn insert(&mut self, name: SmolStr, def: TypeDef) {
        self.types.insert(name, def);
    }

    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }
}

// ── Top-level type definitions ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct(StructDef),
    Enum(EnumDef),
    Array(ArrayDef),
    Map(MapDef),
    Alias(AliasDef),
}

impl TypeDef {
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Struct(d) => &d.name,
            TypeDef::Enum(d) => &d.name,
            TypeDef::Array(d) => &d.name,
            TypeDef::Map(d) => &d.name,
            TypeDef::Alias(d) => &d.name,
        }
    }

    pub fn tag(&self) -> Option<CborTag> {
        match self {
            TypeDef::Struct(d) => d.tagged,
            TypeDef::Enum(d) => d.tagged,
            TypeDef::Array(d) => d.tagged,
            TypeDef::Map(d) => d.tagged,
            TypeDef::Alias(d) => d.tagged,
        }
    }

    pub fn doc(&self) -> Option<&str> {
        match self {
            TypeDef::Struct(d) => d.doc.as_deref(),
            TypeDef::Enum(d) => d.doc.as_deref(),
            TypeDef::Array(d) => d.doc.as_deref(),
            TypeDef::Map(d) => d.doc.as_deref(),
            TypeDef::Alias(d) => d.doc.as_deref(),
        }
    }
}

// ── Struct (CDDL map with named keys) ────────────────────────────────────────

/// A CDDL map rule with labelled key/value pairs.
///
/// ```cddl
/// sensor = { id: uint, label: tstr, value: float32 }
/// ```
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: SmolStr,
    pub fields: Vec<FieldDef>,
    /// A map (`{ a: uint }`), or an array of positional members, a record
    /// (`[a: uint, b: tstr]`, `[uint, tstr]`; RFC 8610 §3.4).
    pub encoding: StructEncoding,
    /// The entries a map has besides its named members, when it has a member
    /// with a computed key (`{ a: uint, * tstr => any }`).
    pub rest: Option<MapRest>,
    pub tagged: Option<CborTag>,
    /// Emit map keys in sorted order on encode (dCBOR / deterministic mode).
    pub dcbor: bool,
    pub doc: Option<String>,
    pub deprecated: bool,
}

/// The entries of a map besides its named members (`* tstr => any` in
/// `{ a: uint, * tstr => any }`): any number, as the occurrence allows, of
/// keys and values of the given types, none of them a named member's key.
#[derive(Debug, Clone)]
pub struct MapRest {
    /// The member that holds them in generated code (`extra`, or `extra_entries`
    /// when a named member is already called `extra`).
    pub name: SmolStr,
    pub key_ty: TypeRef,
    pub val_ty: TypeRef,
    pub occurrence: Occurrence,
}

/// How a [`StructDef`] is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StructEncoding {
    /// A map: each member is a key/value pair.
    #[default]
    Map,
    /// An array: the members' values in order, keys left out. Optional
    /// members may only come last, and are left off the end when absent.
    Array,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: SmolStr,
    /// The CBOR map key.  Usually matches `name` (tstr key), but can be a
    /// numeric key when the CDDL uses an integer literal key.
    pub key: FieldKey,
    pub ty: TypeRef,
    pub occurrence: Occurrence,
    pub constraints: Vec<Constraint>,
    pub default: Option<LiteralValue>,
    pub doc: Option<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKey {
    /// Text key (most common): `label: tstr`
    Text(SmolStr),
    /// Integer key: `1 => tstr`
    Int(i64),
    /// Unsigned integer key: `1 => tstr` (positive only)
    Uint(u64),
}

// ── Enum (type choice a / b / c) ─────────────────────────────────────────────

/// A CDDL type-choice rule.
///
/// ```cddl
/// status = "ok" / "warn" / "error"
/// sensor-value = float32 / int / tstr
/// ```
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: SmolStr,
    pub variants: Vec<EnumVariant>,
    pub tagged: Option<CborTag>,
    pub doc: Option<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    /// Synthesized name for anonymous variants (e.g. `"ok"` → `Ok`).
    pub name: SmolStr,
    pub ty: TypeRef,
    /// For a literal-valued variant (e.g. the `"ok"` in `"ok" / "warn"`),
    /// the exact value it must equal — `ty` alone only narrows to the
    /// primitive kind (e.g. any `tstr`), so this is what discriminates one
    /// variant from another with the same underlying primitive.
    pub constraints: Vec<Constraint>,
}

// ── Array ─────────────────────────────────────────────────────────────────────

/// A CDDL array rule.
///
/// ```cddl
/// ; @capacity 32
/// readings = [* float32]
/// ```
#[derive(Debug, Clone)]
pub struct ArrayDef {
    pub name: SmolStr,
    pub element: TypeRef,
    pub occurrence: Occurrence,
    /// A repeated group (`[* (a: uint, b: tstr)]`, RFC 8610 §2.1): `element`
    /// names a record (a [`StructDef`] with [`StructEncoding::Array`]) whose
    /// members sit inline in this array, with no array of their own, so
    /// `[1, "x", 2, "y"]` holds two elements. Its members are all required.
    pub flat: bool,
    pub tagged: Option<CborTag>,
    pub doc: Option<String>,
    pub deprecated: bool,
}

// ── Map (open, typed k/v) ─────────────────────────────────────────────────────

/// A CDDL open map rule with typed key and value.
///
/// ```cddl
/// ; @capacity 16
/// properties = { * tstr => any }
/// ```
#[derive(Debug, Clone)]
pub struct MapDef {
    pub name: SmolStr,
    pub key_ty: TypeRef,
    pub val_ty: TypeRef,
    pub occurrence: Occurrence,
    pub tagged: Option<CborTag>,
    pub doc: Option<String>,
    pub deprecated: bool,
}

// ── Alias ─────────────────────────────────────────────────────────────────────

/// A named alias for a primitive or another type, optionally with constraints.
///
/// ```cddl
/// device-id = tstr .size 16
/// temperature = int .ge -40 .le 85
/// ```
#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: SmolStr,
    pub ty: TypeRef,
    pub constraints: Vec<Constraint>,
    pub tagged: Option<CborTag>,
    pub doc: Option<String>,
    pub deprecated: bool,
}

// ── Shared sub-types ──────────────────────────────────────────────────────────

/// A fully resolved type reference.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeRef {
    /// A CBOR primitive.
    Primitive(Primitive),

    /// Reference to another named [`TypeDef`] in the same [`IrModule`].
    Named(SmolStr),

    /// Inline anonymous choice (used for struct field types that are choices).
    Choice(Vec<TypeRef>),

    /// A type with a CBOR tag wrapper (from `#6.NN(type)`).
    Tagged(CborTag, Box<TypeRef>),
}

impl TypeRef {
    pub fn is_primitive(&self) -> bool {
        matches!(self, TypeRef::Primitive(_))
    }
}

/// CBOR primitive types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Primitive {
    Bool,
    Null,
    Undefined,
    /// Unsigned integer (major type 0)
    Uint,
    /// Signed integer — may be negative (major type 0 or 1)
    Int,
    Float16,
    Float32,
    Float64,
    /// Any float (float16 | float32 | float64)
    Float,
    /// Byte string (major type 2)
    Bstr,
    /// Text string (major type 3)
    Tstr,
    /// Any CBOR data item (`#`)
    Any,
}

impl Primitive {
    /// Map a CDDL prelude name to a Primitive.
    pub fn from_prelude(name: &str) -> Option<Self> {
        match name {
            // `true` / `false` (#7.21 / #7.20) are bools that admit one value; the
            // lowering adds the `.eq` that pins it (dgtlrift/cddlc#15).
            "bool" | "true" | "false" => Some(Primitive::Bool),
            "null" | "nil" => Some(Primitive::Null),
            "undefined" => Some(Primitive::Undefined),
            "uint" => Some(Primitive::Uint),
            "int" => Some(Primitive::Int),
            "float16" => Some(Primitive::Float16),
            "float32" => Some(Primitive::Float32),
            "float64" => Some(Primitive::Float64),
            "float" => Some(Primitive::Float),
            "bstr" | "bytes" => Some(Primitive::Bstr),
            "tstr" | "text" => Some(Primitive::Tstr),
            "any" => Some(Primitive::Any),
            // `number` and the tagged prelude types aren't primitives; see
            // `prelude::prelude_typeref`.
            _ => None,
        }
    }
}

/// Occurrence / cardinality of a field or array element.
#[derive(Debug, Clone, PartialEq)]
pub enum Occurrence {
    /// Exactly once — field is required.
    Required,
    /// Zero or one — field is optional (`?`).
    Optional,
    /// Zero or more (`*`) — with resolved capacity for stack/arena targets.
    ZeroOrMore { capacity: Capacity },
    /// One or more (`+`) — with resolved capacity.
    OneOrMore { capacity: Capacity },
    /// Bounded range `n*m` — with resolved capacity.
    Bounded {
        min: u32,
        max: Option<u32>,
        capacity: Capacity,
    },
}

impl Occurrence {
    pub fn is_required(&self) -> bool {
        matches!(self, Occurrence::Required)
    }

    pub fn is_repeated(&self) -> bool {
        !matches!(self, Occurrence::Required | Occurrence::Optional)
    }

    pub fn capacity(&self) -> Option<&Capacity> {
        match self {
            Occurrence::ZeroOrMore { capacity } => Some(capacity),
            Occurrence::OneOrMore { capacity } => Some(capacity),
            Occurrence::Bounded { capacity, .. } => Some(capacity),
            _ => None,
        }
    }
}

/// How storage is allocated for repeated/variable-length items.
#[derive(Debug, Clone, PartialEq)]
pub enum Capacity {
    /// Exact bound known at compile time (from `n*n` or `@capacity n`).
    Fixed(usize),
    /// Upper bound from `@capacity` pragma or `--max-array` CLI flag.
    Bounded(usize),
    /// No static bound — requires heap/arena allocation.
    Dynamic,
}

/// An inline constraint derived from a CDDL control operator.
#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    /// `.size N` or `.size lo..hi`
    SizeExact(usize),
    SizeRange {
        min: Option<usize>,
        max: Option<usize>,
    },

    /// Numeric range from `..` range operator or `.lt`/`.le`/`.gt`/`.ge`
    ValueRangeInt {
        min: Option<i64>,
        max: Option<i64>,
        inclusive: bool,
    },
    ValueRangeUint {
        min: Option<u64>,
        max: Option<u64>,
        inclusive: bool,
    },
    ValueRangeF64 {
        min: Option<f64>,
        max: Option<f64>,
        inclusive: bool,
    },

    /// `.eq value`
    Eq(LiteralValue),
    /// `.ne value`
    Ne(LiteralValue),

    /// `.regexp "pattern"` with optional user-supplied hook function name.
    Regexp {
        pattern: String,
        hook: Option<String>,
    },

    /// `.cbor T` — inner bstr decodes as type T.
    CborEmbedded(Box<TypeRef>),
    /// `.cborseq T` — inner bstr decodes as a CBOR sequence of T.
    CborSeq(Box<TypeRef>),

    /// `.default value`
    Default(LiteralValue),

    /// `.bits` — only these bit numbers may be set (RFC 8610 §3.8.2): in an
    /// unsigned integer, bit n is the value 2^n; in a byte string, bit n is
    /// bit n % 8 (counting from the least significant) of byte n / 8.
    Bits(Vec<u64>),
    /// RFC 9741: text that holds another value, which must match `ty` and
    /// `constraints`: bytes in base64, hex, base32 or base45 (`tstr .b64u
    /// (bstr .size 32)`), an integer in decimal (`.base10`), or JSON (`.json`).
    TextEncoded {
        encoding: TextEncoding,
        ty: Box<TypeRef>,
        constraints: Vec<Constraint>,
    },
}

/// A literal constant value.
/// How [`Constraint::TextEncoded`] text holds its value (RFC 9741).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// `.b64u`: base64url, no padding (RFC 4648 §5).
    B64u,
    /// `.b64u-sloppy`: base64url, no padding, unused trailing bits not checked.
    B64uSloppy,
    /// `.b64c`: base64 with padding (RFC 4648 §4).
    B64c,
    /// `.b64c-sloppy`: base64 with padding, unused trailing bits not checked.
    B64cSloppy,
    /// `.hex`: hexadecimal, either case.
    Hex,
    /// `.hexlc`: hexadecimal, lower case.
    HexLc,
    /// `.hexuc`: hexadecimal, upper case.
    HexUc,
    /// `.b32`: base32, no padding (RFC 4648 §6).
    B32,
    /// `.h32`: base32 with the extended hex alphabet, no padding (RFC 4648 §7).
    H32,
    /// `.b45`: base45 (RFC 9285).
    B45,
    /// `.base10`: an integer in decimal: `0|-?[1-9][0-9]*`.
    Base10,
    /// `.json`: a JSON text.
    Json,
}

impl TextEncoding {
    /// The encoding a control operator names (`b64u` for `.b64u`).
    pub fn from_control(name: &str) -> Option<Self> {
        Some(match name {
            "b64u" => Self::B64u,
            "b64u-sloppy" => Self::B64uSloppy,
            "b64c" => Self::B64c,
            "b64c-sloppy" => Self::B64cSloppy,
            "hex" => Self::Hex,
            "hexlc" => Self::HexLc,
            "hexuc" => Self::HexUc,
            "b32" => Self::B32,
            "h32" => Self::H32,
            "b45" => Self::B45,
            "base10" => Self::Base10,
            "json" => Self::Json,
            _ => return None,
        })
    }

    /// The control operator (`.b64u`).
    pub fn control(self) -> &'static str {
        match self {
            Self::B64u => ".b64u",
            Self::B64uSloppy => ".b64u-sloppy",
            Self::B64c => ".b64c",
            Self::B64cSloppy => ".b64c-sloppy",
            Self::Hex => ".hex",
            Self::HexLc => ".hexlc",
            Self::HexUc => ".hexuc",
            Self::B32 => ".b32",
            Self::H32 => ".h32",
            Self::B45 => ".b45",
            Self::Base10 => ".base10",
            Self::Json => ".json",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Bool(bool),
    Null,
    Uint(u64),
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
}

impl LiteralValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            LiteralValue::Bool(_) => "bool",
            LiteralValue::Null => "null",
            LiteralValue::Uint(_) => "uint",
            LiteralValue::Int(_) => "int",
            LiteralValue::Float(_) => "float",
            LiteralValue::Bytes(_) => "bstr",
            LiteralValue::Text(_) => "tstr",
        }
    }
}

/// A CBOR tag number (`#6.NN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CborTag(pub u64);
