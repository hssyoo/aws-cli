//! Resolving a lineage to what binds it, and supplying the parse's
//! command tables.
use crate::spec::{Admission, CommandSpec, Invocation, Needs, StreamPolicy};
use awscli_argparse::tables::{self, CommandKind, Node, RequiredOverride, Roots, Tables};
use awscli_argparse::{Argument, ParseResult};
use std::borrow::Cow;

/// The commands a model supplies: `aws <service> <operation>`, derived for
/// the invocation. Tables and names borrow from the source, which holds
/// whatever it loaded for as long as the parse needs them.
pub trait OperationSource: std::fmt::Debug {
    /// Whether `name` is a root this source derives, cheaply: no model is
    /// loaded for the answer.
    fn is_root(&self, name: &str) -> bool;
    /// The operation names of `root` in command-table order, or `None` when
    /// the source derives no such root. They are the choices of its
    /// `operation` argument and the candidates of its close matches.
    fn operations(&self, root: &str) -> Option<Cow<'_, [&str]>>;
    /// The argument table of `[root, operation]`, or `None` for a pair the
    /// source does not derive.
    fn arguments(&self, root: &str, operation: &str) -> Option<Cow<'_, [Argument<'_>]>>;
    /// The options that lift `[root, operation]`'s required arguments.
    fn overrides(&self, root: &str, operation: &str) -> Cow<'_, [RequiredOverride<'_>]>;
    /// Whether the parse failures of `root` (no operation) or of
    /// `[root, operation]` are ones the derived tables reproduce byte for
    /// byte, so they render natively.
    fn reproduces_failures(&self, root: &str, operation: Option<&str>) -> bool;
    /// What every operation of `root` needs before it runs.
    fn needs(&self, root: &str) -> Needs;
    /// Bind the completed parse of one of this source's operations.
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission>;
}

/// The root table: pinned grammars, hand-written specs, then a model
/// source, in that precedence, so a hand-written row claims a path before
/// the model does.
#[derive(Debug, Clone, Copy)]
pub struct Registry<'s> {
    pinned: tables::Pinned<'static>,
    specs: &'static [&'static CommandSpec],
    source: Option<&'s dyn OperationSource>,
}

/// What a parsed lineage resolves to.
#[derive(Debug, Clone, Copy)]
pub enum Resolution<'r> {
    /// A hand-written command.
    Spec(&'static CommandSpec),
    /// An operation of a model source.
    Operation(Operation<'r>),
    /// Nothing the registry binds: a pinned grammar, which the router's own
    /// path binds, a source's root alone, or an unknown lineage.
    Unknown,
}

/// One `[root, operation]` of a model source.
#[derive(Debug, Clone, Copy)]
pub struct Operation<'r> {
    source: &'r dyn OperationSource,
    pub root: &'r str,
    pub name: &'r str,
}
impl Operation<'_> {
    /// What the operation needs before it runs.
    pub fn needs(&self) -> Needs {
        self.source.needs(self.root)
    }
    /// A model operation's response is formatted.
    pub const fn streams(&self) -> StreamPolicy {
        StreamPolicy::Formatted
    }
    /// Bind the completed parse through the source.
    pub fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        self.source.bind(parsed)
    }
}

impl<'s> Registry<'s> {
    /// A registry over `pinned` grammars, parsed natively and bound by the
    /// router's own path; hand-written `specs`; and an optional model
    /// `source`.
    pub const fn new(
        pinned: &'static [&'static tables::Command<'static>],
        specs: &'static [&'static CommandSpec],
        source: Option<&'s dyn OperationSource>,
    ) -> Self {
        Self {
            pinned: tables::Pinned(pinned),
            specs,
            source,
        }
    }
    /// The parse's tables: the pinned globals and this registry's roots.
    pub fn tables(&self) -> Tables<'_> {
        Tables {
            globals: tables::GLOBALS,
            roots: self,
        }
    }
    /// Whether `name` is a root of any kind.
    pub fn is_root(&self, name: &str) -> bool {
        self.pinned.find(&[name]).is_some()
            || self.spec_root(name).is_some()
            || self.source.is_some_and(|source| source.is_root(name))
    }
    /// What `lineage`, outermost first, resolves to.
    pub fn resolve<'r>(&'r self, lineage: &[&'r str]) -> Resolution<'r> {
        let Some((root, rest)) = lineage.split_first() else {
            return Resolution::Unknown;
        };
        if self.pinned.find(&[root]).is_some() {
            return Resolution::Unknown;
        }
        if let Some(spec) = self.spec_root(root) {
            return find_spec(spec, rest).map_or(Resolution::Unknown, Resolution::Spec);
        }
        match (self.source, rest) {
            (Some(source), [name]) if source.arguments(root, name).is_some() => {
                Resolution::Operation(Operation { source, root, name })
            }
            _ => Resolution::Unknown,
        }
    }
    /// Whether a parse failure at `lineage` is one the registry's own
    /// tables reproduce, and so renders natively: at a hand-written command
    /// that binds or has subcommands, at a source's root, whose
    /// ServiceArgParser takes only the operation, or at one of its
    /// operations. A pinned grammar's failures are the router's to judge.
    pub fn reproduces_failures(&self, lineage: &[&str]) -> bool {
        match self.resolve(lineage) {
            Resolution::Spec(spec) => spec.bind.is_some() || !spec.subcommands.is_empty(),
            Resolution::Operation(operation) => operation
                .source
                .reproduces_failures(operation.root, Some(operation.name)),
            Resolution::Unknown => match lineage {
                [root] => {
                    self.pinned.find(&[root]).is_none()
                        && self.spec_root(root).is_none()
                        && self.source.is_some_and(|source| {
                            source.is_root(root) && source.reproduces_failures(root, None)
                        })
                }
                _ => false,
            },
        }
    }
    fn spec_root(&self, name: &str) -> Option<&'static CommandSpec> {
        self.specs.iter().copied().find(|spec| spec.name == name)
    }
}

fn find_spec(root: &'static CommandSpec, rest: &[&str]) -> Option<&'static CommandSpec> {
    rest.iter().try_fold(root, |spec, name| {
        spec.subcommands
            .iter()
            .copied()
            .find(|child| child.name == *name)
    })
}

/// A hand-written command's node: a `BasicCommand`, whose children are its
/// subcommands.
fn spec_node<'a>(spec: &'static CommandSpec) -> Node<'a> {
    Node {
        name: Cow::Borrowed(spec.name),
        kind: CommandKind::Basic,
        arguments: Cow::Borrowed(spec.arguments),
        subcommands: (!spec.subcommands.is_empty())
            .then(|| Cow::Owned(spec.subcommands.iter().map(|child| child.name).collect())),
        overrides: Cow::Borrowed(&[]),
    }
}

impl<'a, 's: 'a> Roots<'a> for Registry<'s> {
    fn root(&self, name: &str) -> Option<Node<'a>> {
        if let Some(command) = self.pinned.find(&[name]) {
            return Some(Node::pinned(command));
        }
        if let Some(spec) = self.spec_root(name) {
            return Some(spec_node(spec));
        }
        let source = self.source?;
        Some(Node {
            name: Cow::Owned(name.to_owned()),
            kind: CommandKind::Service,
            arguments: Cow::Borrowed(&[]),
            subcommands: Some(source.operations(name)?),
            overrides: Cow::Borrowed(&[]),
        })
    }
    fn subcommand(&self, lineage: &[&str], name: &str) -> Option<Node<'a>> {
        let (root, rest) = lineage.split_first()?;
        if let Some(parent) = self.pinned.find(lineage) {
            return parent.subcommand(name).map(Node::pinned);
        }
        if let Some(spec) = self.spec_root(root) {
            let parent = find_spec(spec, rest)?;
            let child = parent
                .subcommands
                .iter()
                .copied()
                .find(|c| c.name == name)?;
            return Some(spec_node(child));
        }
        let source = self.source?;
        if !rest.is_empty() {
            return None;
        }
        Some(Node {
            name: Cow::Owned(name.to_owned()),
            kind: CommandKind::Operation,
            arguments: source.arguments(root, name)?,
            subcommands: None,
            overrides: source.overrides(root, name),
        })
    }
}
