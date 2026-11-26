//! The keys a front-end did not interpret, and which a call reaches.
use super::{OperationSchema, Schema, SchemaSet};
use std::collections::HashSet;
use std::fmt;

/// Where an [`Uninterpreted`] key sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Site {
    /// The document's top level, its `metadata` or `clientContextParams`;
    /// for Smithy, the service shape.
    Service,
    /// An operation, by name, with its input, output and error references.
    Operation(Box<str>),
    /// A named shape by its short name, with its member, list member, map
    /// key and value references.
    Shape(Box<str>),
}

/// A key of the model no front-end interprets and no ignore list excuses:
/// botocore may act on it, so a client that reaches it cannot claim
/// botocore's behaviour. Admission refuses on the first one reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uninterpreted {
    pub site: Site,
    /// The key's path from its site, such as `http.timeout`,
    /// `metadata.newSetting`, `members.Name.newTrait` or a trait id.
    pub key: Box<str>,
}

impl fmt::Display for Uninterpreted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.site {
            Site::Service => write!(f, "{}", self.key),
            Site::Operation(name) => write!(f, "operation {name}: {}", self.key),
            Site::Shape(name) => write!(f, "shape {name}: {}", self.key),
        }
    }
}

impl SchemaSet {
    /// Every uninterpreted key, in the order the front-end recorded them.
    pub fn uninterpreted(&self) -> &[Uninterpreted] {
        &self.uninterpreted
    }

    /// The uninterpreted keys a call of `operation` reaches: the service's,
    /// the operation's own, and those of every named shape its input, output
    /// and errors reach. The shapes are walked only when some shape has one.
    pub fn uninterpreted_reached(&self, operation: OperationSchema<'_>) -> Vec<&Uninterpreted> {
        let shaped = self
            .uninterpreted
            .iter()
            .any(|key| matches!(key.site, Site::Shape(_)));
        let reached = if shaped {
            reached_names(operation)
        } else {
            HashSet::new()
        };
        self.uninterpreted
            .iter()
            .filter(|key| match &key.site {
                Site::Service => true,
                Site::Operation(name) => **name == *operation.name(),
                Site::Shape(name) => reached.contains(&**name),
            })
            .collect()
    }
}

/// The short names of the shapes `operation`'s input, output and errors
/// reach, through members, list members and map keys and values.
fn reached_names<'s>(operation: OperationSchema<'s>) -> HashSet<&'s str> {
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    let roots = operation.input().into_iter().chain(operation.output());
    let mut stack: Vec<Schema<'s>> = roots.chain(operation.errors()).collect();
    while let Some(schema) = stack.pop() {
        let schema = schema.target();
        if !seen.insert(schema.index()) {
            continue;
        }
        names.insert(schema.id().name());
        stack.extend(schema.members());
        stack.extend(schema.list_member());
        stack.extend(schema.map_key().into_iter().chain(schema.map_value()));
    }
    names
}
