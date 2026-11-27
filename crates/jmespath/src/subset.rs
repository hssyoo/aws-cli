//! Subsets of the language a consumer accepts: a model's expression
//! outside its consumer's subset is refused before anything evaluates it.
use crate::parser::Node;

/// A construct of the language, as a subset admits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// `name` or `"name"`.
    Identifier,
    /// `a.b`.
    Subexpression,
    /// `[0]`.
    Index,
    /// `[0:2]`.
    Slice,
    /// `[*]`.
    ListWildcard,
    /// `*`.
    ObjectWildcard,
    /// `[?cond]`.
    Filter,
    /// `[]`.
    Flatten,
    /// `@`.
    Current,
    /// `` `json` `` and `'raw'`.
    Literal,
    /// `[a, b]`.
    MultiSelectList,
    /// `{k: a}`.
    MultiSelectHash,
    /// `||`, `&&` and `!`.
    Logic,
    /// `a | b`.
    Pipe,
    /// `==`, `<`, ...
    Comparator,
    /// `&expr`.
    ExpressionReference,
    /// A function call, of a function the subset names.
    Function,
}

/// The constructs and functions a consumer evaluates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subset {
    pub features: &'static [Feature],
    pub functions: &'static [&'static str],
}

/// Smithy's subset for `operationContextParams` paths: identifiers,
/// sub-expressions, list wildcards, multi-select lists, flattening and
/// `keys`.
pub const OPERATION_CONTEXT_PARAMS: Subset = Subset {
    features: &[
        Feature::Identifier,
        Feature::Subexpression,
        Feature::ListWildcard,
        Feature::MultiSelectList,
        Feature::Flatten,
        Feature::Current,
        Feature::Function,
    ],
    functions: &["keys"],
};

/// What a subset refuses in an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outside {
    Feature(Feature),
    Function(Box<str>),
}

impl std::fmt::Display for Outside {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Feature(feature) => write!(f, "{feature:?}"),
            Self::Function(name) => write!(f, "function {name}()"),
        }
    }
}

/// The first construct of `node` the subset refuses, outermost first.
pub(crate) fn check<V>(node: &Node<V>, subset: &Subset) -> Result<(), Outside> {
    let (feature, children): (Option<Feature>, Vec<&Node<V>>) = match node {
        Node::Field(_) => (Some(Feature::Identifier), vec![]),
        Node::Subexpression(children) => (Some(Feature::Subexpression), children.iter().collect()),
        Node::IndexExpression(children) => (None, children.iter().collect()),
        Node::Index(_) => (Some(Feature::Index), vec![]),
        Node::Slice(_) => (Some(Feature::Slice), vec![]),
        Node::Projection(left, right) => {
            let feature = match &**left {
                Node::Flatten(_) => None,
                Node::IndexExpression(_) => None,
                _ => Some(Feature::ListWildcard),
            };
            (feature, vec![left, right])
        }
        Node::ValueProjection(left, right) => (Some(Feature::ObjectWildcard), vec![left, right]),
        Node::FilterProjection(parts) => (Some(Feature::Filter), parts.iter().collect()),
        Node::Flatten(child) => (Some(Feature::Flatten), vec![child]),
        Node::Identity => (None, vec![]),
        Node::Current => (Some(Feature::Current), vec![]),
        Node::Literal(_) => (Some(Feature::Literal), vec![]),
        Node::MultiSelectList(children) => {
            (Some(Feature::MultiSelectList), children.iter().collect())
        }
        Node::MultiSelectDict(pairs) => (
            Some(Feature::MultiSelectHash),
            pairs.iter().map(|(_, child)| child).collect(),
        ),
        Node::Or(left, right) | Node::And(left, right) => (Some(Feature::Logic), vec![left, right]),
        Node::Not(child) => (Some(Feature::Logic), vec![child]),
        Node::Pipe(left, right) => (Some(Feature::Pipe), vec![left, right]),
        Node::Comparator(_, left, right) => (Some(Feature::Comparator), vec![left, right]),
        Node::Expref(child) => (Some(Feature::ExpressionReference), vec![child]),
        Node::Function(name, args) => {
            if subset.features.contains(&Feature::Function) && !subset.functions.contains(&&**name)
            {
                return Err(Outside::Function(name.clone()));
            }
            (Some(Feature::Function), args.iter().collect())
        }
    };
    if let Some(feature) = feature.filter(|feature| !subset.features.contains(feature)) {
        return Err(Outside::Feature(feature));
    }
    children
        .into_iter()
        .try_for_each(|child| check(child, subset))
}
