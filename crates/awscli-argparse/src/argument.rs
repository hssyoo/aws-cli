//! Argument definitions and destination normalization.
use crate::Value;
/// Namespace update performed by an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Store,
    StoreTrue,
    StoreFalse,
    Append,
    AppendNamed,
    Command,
    Version,
}
/// Argument token cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nargs {
    Optional,
    ZeroOrMore,
    OneOrMore,
    Count(u8),
}
/// Deferred modeled conversion: the unpack loop converts a value argparse
/// stored as text (`unpack`), after the parse. argparse's own `type=`
/// conversion is [`crate::ValueType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliType {
    Integer,
    Blob,
    Map,
}
/// Constant values representable in pinned and derived tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultValue<'a> {
    /// No default given: argparse's own, which a boolean action replaces
    /// with its opposite value.
    #[default]
    None,
    /// An explicit `default=None`, which a boolean action keeps
    /// (`BooleanArgument.add_to_parser`).
    Null,
    Boolean(bool),
    Integer(i64),
    String(&'a str),
}
impl DefaultValue<'_> {
    /// Own the table default for a namespace.
    pub fn value(self) -> Value {
        match self {
            Self::None | Self::Null => Value::Null,
            Self::Boolean(v) => v.into(),
            Self::Integer(v) => v.into(),
            Self::String(v) => v.into(),
        }
    }
}
/// One action in an argparse table. A pinned table borrows `'static`
/// text; a table derived from a model borrows from whatever holds the
/// model for the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argument<'a> {
    pub name: &'a str,
    pub dest: Option<&'a str>,
    pub positional: bool,
    pub action: Action,
    pub nargs: Option<Nargs>,
    pub constant: Option<DefaultValue<'a>>,
    pub default: DefaultValue<'a>,
    pub choices: &'a [&'a str],
    pub required: bool,
    /// `_UNDOCUMENTED`: parsed as any other, but no suggestion names it
    /// (`get_option_suggestion_candidates`).
    pub hidden: bool,
    /// argparse's `type=`: the conversion the value goes through while it
    /// is parsed (`convert`). The pinned tables carry none.
    pub value_type: Option<crate::ValueType>,
    pub cli_type: Option<CliType>,
}
impl<'a> Argument<'a> {
    /// Define a scalar argument with argparse defaults.
    pub const fn new(name: &'a str) -> Self {
        Self {
            name,
            dest: None,
            positional: false,
            action: Action::Store,
            nargs: None,
            constant: None,
            default: DefaultValue::None,
            choices: &[],
            required: false,
            hidden: false,
            value_type: None,
            cli_type: None,
        }
    }
    /// Destination after the default dash replacement.
    pub fn destination(self) -> String {
        self.dest
            .map_or_else(|| self.name.replace('-', "_"), str::to_owned)
    }
    /// Canonical diagnostic spelling.
    pub fn spelling(self) -> String {
        if self.positional {
            self.dest.unwrap_or(self.name).to_owned()
        } else {
            format!("--{}", self.name)
        }
    }
    /// Test a destination in const contexts.
    pub const fn matches_dest(self, dest: &str) -> bool {
        let source = match self.dest {
            Some(v) => v,
            None => self.name,
        }
        .as_bytes();
        let target = dest.as_bytes();
        if source.len() != target.len() {
            return false;
        }
        let mut i = 0;
        while i < source.len() {
            let c = if self.dest.is_none() && source[i] == b'-' {
                b'_'
            } else {
                source[i]
            };
            if c != target[i] {
                return false;
            }
            i += 1;
        }
        true
    }
    /// Whether missing this argument fails parsing.
    pub const fn is_required(self) -> bool {
        if !self.positional {
            return self.required;
        }
        !matches!(self.nargs, Some(Nargs::Optional | Nargs::ZeroOrMore))
    }
    /// Effective arity includes implicit zero operand flags.
    pub const fn arity(self) -> Option<Nargs> {
        match self.nargs {
            Some(v) => Some(v),
            None => match self.action {
                Action::StoreTrue | Action::StoreFalse | Action::Version => Some(Nargs::Count(0)),
                _ => None,
            },
        }
    }
    /// Default after boolean action defaults are applied.
    pub fn default_value(self) -> Value {
        match (self.default, self.action) {
            (DefaultValue::None, Action::StoreTrue) => false.into(),
            (DefaultValue::None, Action::StoreFalse) => true.into(),
            _ => self.default.value(),
        }
    }
}
#[cfg(test)]
mod tests;
