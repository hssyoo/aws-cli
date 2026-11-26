//! The decoded rule set. Names resolve to scope slots and functions to
//! [`FnId`] at decode time, so evaluation does no hashing and no lookup
//! by name.
use std::sync::Arc;

/// A value during evaluation. Records keep their keys in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalValue {
    Str(Arc<str>),
    Bool(bool),
    Int(i64),
    Array(Arc<[EvalValue]>),
    Record(Arc<[(Box<str>, EvalValue)]>),
}

impl EvalValue {
    /// A string value.
    pub fn str(value: &str) -> Self {
        Self::Str(value.into())
    }
    /// The CRT engine's truthiness: `false` and zero are false, any other
    /// value true (an unset value is false, outside this type).
    pub(crate) fn truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::Str(_) | Self::Array(_) | Self::Record(_) => true,
        }
    }
    /// The value of the first `key` of a record.
    pub(crate) fn field(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Record(fields) => fields
                .iter()
                .find(|(name, _)| &**name == key)
                .map(|(_, v)| v),
            _ => None,
        }
    }
}

/// A scope slot: parameters take the first slots, in document order, and
/// each assignment the next free slot of its rule's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot(pub(crate) u16);

impl Slot {
    pub(crate) fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// A parameter's declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Boolean,
    StringArray,
}

/// A rule set parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterDef {
    pub(crate) name: Box<str>,
    pub(crate) ty: ParamType,
    pub(crate) builtin: Option<Box<str>>,
    pub(crate) required: bool,
    pub(crate) default: Option<EvalValue>,
    pub(crate) slot: Slot,
}

impl ParameterDef {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn ty(&self) -> ParamType {
        self.ty
    }
    /// The built-in that supplies it (`AWS::Region`, …), if any.
    pub fn builtin(&self) -> Option<&str> {
        self.builtin.as_deref()
    }
    pub fn required(&self) -> bool {
        self.required
    }
    /// The default, which the engine applies only to a required parameter
    /// (as the CRT engine does).
    pub fn default(&self) -> Option<&EvalValue> {
        self.default.as_ref()
    }
    pub fn slot(&self) -> Slot {
        self.slot
    }
}

/// A decoded `endpoint-rule-set-1.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSet {
    pub(crate) version: Box<str>,
    pub(crate) params: Box<[ParameterDef]>,
    pub(crate) rules: Box<[Rule]>,
    pub(crate) scope_len: u16,
}

impl RuleSet {
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rule {
    Endpoint {
        when: Box<[Condition]>,
        url: Expr,
        /// The `properties` object as the CRT engine resolves it: its
        /// compact JSON text with the template references inside its
        /// strings, re-read as JSON once resolved.
        properties: Option<Box<[TemplatePart]>>,
        headers: Box<[Header]>,
    },
    Error {
        when: Box<[Condition]>,
        message: Expr,
    },
    Tree {
        when: Box<[Condition]>,
        rules: Box<[Rule]>,
    },
}

impl Rule {
    pub(crate) fn when(&self) -> &[Condition] {
        match self {
            Self::Endpoint { when, .. } | Self::Error { when, .. } | Self::Tree { when, .. } => {
                when
            }
        }
    }
}

/// A header's name and the expressions of its values.
pub(crate) type Header = (Box<str>, Box<[Expr]>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Condition {
    /// An [`Expr::Fn`] or [`Expr::GetAttr`].
    pub(crate) call: Expr,
    pub(crate) assign: Option<Slot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expr {
    Lit(EvalValue),
    Template(Box<[TemplatePart]>),
    Ref(Slot),
    Fn(FnId, Box<[Expr]>),
    GetAttr(Box<Expr>, Box<[PathPart]>),
    Array(Box<[Expr]>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplatePart {
    Lit(Box<str>),
    /// `{name}`: a string in scope.
    Var(Slot),
    /// `{name#path}`: a string at `path` of a value in scope.
    Path(Slot, Box<[PathPart]>),
}

/// One step of a `getAttr` path: `name` or `[index]`. An index ends a
/// path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathPart {
    Key(Box<str>),
    Index(i64),
}

/// The functions of the rules language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FnId {
    BooleanEquals,
    StringEquals,
    IsSet,
    Not,
    IsValidHostLabel,
    ParseUrl,
    Substring,
    UriEncode,
}

impl FnId {
    /// The function a rule set names, and how many arguments it takes.
    pub(crate) fn lookup(name: &str) -> Option<(Self, usize)> {
        Some(match name {
            "booleanEquals" => (Self::BooleanEquals, 2),
            "stringEquals" => (Self::StringEquals, 2),
            "isSet" => (Self::IsSet, 1),
            "not" => (Self::Not, 1),
            "isValidHostLabel" => (Self::IsValidHostLabel, 2),
            "parseURL" => (Self::ParseUrl, 1),
            "substring" => (Self::Substring, 4),
            "uriEncode" => (Self::UriEncode, 1),
            _ => return None,
        })
    }
}
