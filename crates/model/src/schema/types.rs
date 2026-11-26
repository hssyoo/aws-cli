//! Shape types and the Smithy prelude.

/// A shape's type, as smithy-rs's `ShapeType` plus `Unknown`. Enums are
/// strings or integers with an enum trait. Deliberately exhaustive: a port
/// of a `ShapeKind::Structure` match decides `Document` and `Union` itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeType {
    Boolean,
    Byte,
    Short,
    Integer,
    Long,
    Float,
    Double,
    BigInteger,
    BigDecimal,
    String,
    Blob,
    Timestamp,
    Document,
    List,
    Map,
    Structure,
    Union,
    /// A botocore `type` with no botocore class: loaded, and it fails only
    /// when used. Its text is the schema's `type_name`.
    Unknown,
}

impl ShapeType {
    /// botocore's `type` text for the type, when the front-end gives none.
    /// `None` for `Unknown`, whose text only the model knows.
    pub const fn default_type_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Boolean => "boolean",
            Self::Byte => "byte",
            Self::Short => "short",
            Self::Integer => "integer",
            Self::Long => "long",
            Self::Float => "float",
            Self::Double => "double",
            Self::BigInteger => "bigInteger",
            Self::BigDecimal => "bigDecimal",
            Self::String => "string",
            Self::Blob => "blob",
            Self::Timestamp => "timestamp",
            Self::Document | Self::Structure | Self::Union => "structure",
            Self::List => "list",
            Self::Map => "map",
            Self::Unknown => return None,
        })
    }

    /// Whether the type may hold members: a structure, a union, or a
    /// document (C2J's `document` structures keep their `members`).
    pub const fn is_aggregate(self) -> bool {
        matches!(self, Self::Structure | Self::Union | Self::Document)
    }

    /// Whether Smithy bounds it with `range` rather than `length`.
    pub const fn is_number(self) -> bool {
        matches!(
            self,
            Self::Byte
                | Self::Short
                | Self::Integer
                | Self::Long
                | Self::Float
                | Self::Double
                | Self::BigInteger
                | Self::BigDecimal
        )
    }
}

/// The prelude's namespace.
pub const PRELUDE_NAMESPACE: &str = "smithy.api";

/// The prelude shapes every set holds after its other schemas, outside the
/// named range: no name lookup, error walk or registry sees them.
pub const PRELUDE: [(&str, ShapeType); 21] = [
    ("String", ShapeType::String),
    ("Blob", ShapeType::Blob),
    ("Boolean", ShapeType::Boolean),
    ("Byte", ShapeType::Byte),
    ("Short", ShapeType::Short),
    ("Integer", ShapeType::Integer),
    ("Long", ShapeType::Long),
    ("Float", ShapeType::Float),
    ("Double", ShapeType::Double),
    ("BigInteger", ShapeType::BigInteger),
    ("BigDecimal", ShapeType::BigDecimal),
    ("Timestamp", ShapeType::Timestamp),
    ("Document", ShapeType::Document),
    ("Unit", ShapeType::Structure),
    ("PrimitiveBoolean", ShapeType::Boolean),
    ("PrimitiveByte", ShapeType::Byte),
    ("PrimitiveShort", ShapeType::Short),
    ("PrimitiveInteger", ShapeType::Integer),
    ("PrimitiveLong", ShapeType::Long),
    ("PrimitiveFloat", ShapeType::Float),
    ("PrimitiveDouble", ShapeType::Double),
];

/// The index of `Unit` in [`PRELUDE`]: an operation input or output that
/// targets it has none.
pub(super) const UNIT: usize = 13;
