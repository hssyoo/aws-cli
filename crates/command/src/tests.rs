#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use awscli_argparse::chain::Outcome;
use awscli_argparse::tables::{CommandKind, Node, Roots};
use awscli_argparse::{Argument, Options, ParseResult};
use cli_core::cli_error::{Category, CommandFailure, PythonReason};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

/// An invocation that writes its bound text, so a test can tell which
/// binding produced it.
#[derive(Debug)]
struct Echo(String);
impl Invocation for Echo {
    fn run(&self, cx: &mut Context<'_>) -> Result<u8, CommandFailure> {
        writeln!(cx.stdout, "{}", self.0).map_err(|_| CommandFailure::new(Category::General))?;
        Ok(0)
    }
}

const THING_ARGUMENTS: &[Argument] = &[Argument {
    required: true,
    ..Argument::new("thing-id")
}];
/// A model source stand-in: one root, `zsvc`, whose operations come in
/// model order with tables built on demand.
#[derive(Debug)]
struct Fake;
impl OperationSource for Fake {
    fn is_root(&self, name: &str) -> bool {
        // A source that also claims a pinned root's or a spec's name never
        // shadows it.
        matches!(name, "zsvc" | "s3" | "zlocal")
    }
    fn operations(&self, root: &str) -> Option<Cow<'_, [&str]>> {
        self.is_root(root)
            .then(|| vec!["get-thing", "list-things"].into())
    }
    fn arguments(&self, root: &str, operation: &str) -> Option<Cow<'_, [Argument<'_>]>> {
        match (root, operation) {
            ("zsvc", "get-thing") => Some(THING_ARGUMENTS.to_vec().into()),
            ("zsvc", "list-things") => Some(Cow::Borrowed(&[])),
            _ => None,
        }
    }
    fn overrides(
        &self,
        _root: &str,
        _operation: &str,
    ) -> Cow<'_, [awscli_argparse::tables::RequiredOverride<'_>]> {
        Cow::Borrowed(&[])
    }
    fn reproduces_failures(&self, _root: &str, _operation: Option<&str>) -> bool {
        true
    }
    fn needs(&self, root: &str) -> Needs {
        Needs::Client {
            service: root.to_owned().into(),
        }
    }
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        match parsed.values.string("thing_id") {
            Some("refused") => Err(Admission::Python(PythonReason::UnsupportedValue)),
            Some(id) => Ok(Box::new(Echo(id.to_owned()))),
            None => Ok(Box::new(Echo("all".into()))),
        }
    }
}

fn show(_: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
    Ok(Box::new(Echo("shown".into())))
}
const SHOW_ARGUMENTS: &[Argument] = &[Argument::new("key")];
static SHOW: CommandSpec = CommandSpec {
    name: "show",
    arguments: SHOW_ARGUMENTS,
    subcommands: &[],
    needs: Needs::Client {
        service: Cow::Borrowed("zlocal"),
    },
    streams: StreamPolicy::Formatted,
    bind: Some(show),
};
static PINNED_ONLY: CommandSpec = CommandSpec {
    name: "pinned-only",
    bind: None,
    ..SHOW_SPEC_BASE
};
const SHOW_SPEC_BASE: CommandSpec = CommandSpec {
    name: "",
    arguments: &[],
    subcommands: &[],
    needs: Needs::Client {
        service: Cow::Borrowed("zlocal"),
    },
    streams: StreamPolicy::Formatted,
    bind: None,
};
static ZLOCAL: CommandSpec = CommandSpec {
    name: "zlocal",
    subcommands: &[&SHOW, &PINNED_ONLY],
    ..SHOW_SPEC_BASE
};
static SPECS: [&CommandSpec; 1] = [&ZLOCAL];
static PINNED: [&awscli_argparse::tables::Command<'static>; 1] = [&awscli_argparse::tables::S3];

fn registry(source: &Fake) -> Registry<'_> {
    Registry::new(&PINNED, &SPECS, Some(source))
}

#[test]
fn a_spec_lineage_resolves_to_its_spec() {
    let source = Fake;
    let registry = registry(&source);
    assert!(matches!(
        registry.resolve(&["zlocal", "show"]),
        Resolution::Spec(spec) if std::ptr::eq(spec, &SHOW)
    ));
    assert!(matches!(
        registry.resolve(&["zlocal"]),
        Resolution::Spec(spec) if std::ptr::eq(spec, &ZLOCAL)
    ));
}

#[test]
fn a_source_operation_resolves_to_its_operation() {
    let source = Fake;
    let registry = registry(&source);
    let Resolution::Operation(operation) = registry.resolve(&["zsvc", "get-thing"]) else {
        panic!("a source operation resolves to an operation");
    };
    assert_eq!((operation.root, operation.name), ("zsvc", "get-thing"));
    assert_eq!(
        operation.needs(),
        Needs::Client {
            service: "zsvc".into()
        }
    );
}

#[test]
fn pinned_partial_and_unknown_lineages_resolve_to_nothing() {
    let source = Fake;
    let registry = registry(&source);
    for lineage in [
        &["zsvc"][..],
        &["zsvc", "nosuch"],
        &["zsvc", "get-thing", "deeper"],
        &["zlocal", "nosuch"],
        &["s3", "ls"],
        &["s3"],
        &["nosuch"],
        &[],
    ] {
        assert!(
            matches!(registry.resolve(lineage), Resolution::Unknown),
            "{lineage:?}"
        );
    }
}

fn root<'a>(registry: &Registry<'a>, name: &str) -> Node<'a> {
    registry
        .root(name)
        .unwrap_or_else(|| panic!("{name} is a root"))
}

#[test]
fn roots_are_pinned_then_specs_then_the_source() {
    let source = Fake;
    let registry = registry(&source);
    for name in ["s3", "zlocal", "zsvc"] {
        assert!(registry.is_root(name), "{name}");
    }
    assert!(!registry.is_root("ec2"));
    let pinned = root(&registry, "s3");
    assert_eq!(pinned.kind, CommandKind::Basic);
    assert_eq!(
        pinned.subcommands.as_deref().map(<[&str]>::len),
        awscli_argparse::tables::S3.subcommands.map(<[_]>::len)
    );
    let spec = root(&registry, "zlocal");
    assert_eq!(spec.kind, CommandKind::Basic);
    let children = ["show", "pinned-only"];
    assert_eq!(spec.subcommands.as_deref(), Some(&children[..]));
    let service = root(&registry, "zsvc");
    assert_eq!(service.kind, CommandKind::Service);
    let operations = ["get-thing", "list-things"];
    assert_eq!(service.subcommands.as_deref(), Some(&operations[..]));
}

#[test]
fn children_come_from_the_tree_that_owns_the_root() {
    let source = Fake;
    let registry = registry(&source);
    let child = |lineage: &[&str], name: &str| {
        registry
            .subcommand(lineage, name)
            .unwrap_or_else(|| panic!("{lineage:?} has {name}"))
    };
    let operation = child(&["zsvc"], "get-thing");
    assert_eq!(operation.kind, CommandKind::Operation);
    assert_eq!(&*operation.arguments, THING_ARGUMENTS);
    assert!(operation.subcommands.is_none());
    assert_eq!(&*child(&["zlocal"], "show").arguments, SHOW_ARGUMENTS);
    assert_eq!(child(&["s3"], "ls").kind, CommandKind::Basic);
    assert!(registry.subcommand(&["zsvc"], "nosuch").is_none());
    assert!(
        registry
            .subcommand(&["zsvc", "get-thing"], "deeper")
            .is_none()
    );
}

#[test]
fn failures_render_natively_only_where_the_registry_binds() {
    let source = Fake;
    let registry = registry(&source);
    for (lineage, native) in [
        (&["zsvc"][..], true),
        (&["zsvc", "get-thing"], true),
        (&["zlocal"], true),
        (&["zlocal", "show"], true),
        (&["zlocal", "pinned-only"], false),
        (&["s3"], false),
        (&["s3", "ls"], false),
        (&["nosuch"], false),
    ] {
        assert_eq!(registry.reproduces_failures(lineage), native, "{lineage:?}");
    }
}

fn parse<'a>(registry: &'a Registry<'_>, args: &[&str]) -> ParseResult<'a> {
    let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let mut parsed = awscli_argparse::parse(&argv, &Options::default(), &registry.tables());
    let variables = std::collections::HashMap::new();
    let environment = awscli_argparse::unpack::Environment {
        directory: Path::new("."),
        variables: &variables,
        files: &awscli_argparse::unpack::FsParamfiles,
        preferred_encoding: awscli_argparse::unpack::PreferredEncoding::Locale,
    };
    assert!(!awscli_argparse::complete(&mut parsed, &environment));
    parsed
}

fn run(invocation: &dyn Invocation) -> (Result<u8, CommandFailure>, Vec<u8>) {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let mut cx = Context {
        env: &BTreeMap::<String, String>::new(),
        shared_files: &awscli_config::ParsedFiles::default(),
        globals: &cli_core::command::Globals::default(),
        directory: Path::new("."),
        stdout: &mut stdout,
        stderr: &mut stderr,
    };
    let status = invocation.run(&mut cx);
    (status, stdout)
}

#[test]
fn a_parsed_lineage_binds_through_its_spec_or_its_source() {
    let source = Fake;
    let registry = registry(&source);
    for (args, text) in [
        (&["zsvc", "get-thing", "--thing-id", "t-1"][..], "t-1\n"),
        (&["zsvc", "list-things"], "all\n"),
        (&["zlocal", "show", "--key", "k"], "shown\n"),
    ] {
        let parsed = parse(&registry, args);
        assert_eq!(parsed.outcome, Outcome::Parsed, "{args:?}");
        let names: Vec<_> = parsed.lineage.iter().map(|node| &*node.name).collect();
        let bound = match registry.resolve(&names) {
            Resolution::Operation(operation) => operation.bind(&parsed),
            Resolution::Spec(CommandSpec {
                bind: Some(bind), ..
            }) => bind(&parsed),
            other => panic!("{args:?}: {other:?}"),
        };
        let Ok(invocation) = bound else {
            panic!("{args:?} binds");
        };
        let (status, stdout) = run(invocation.as_ref());
        assert_eq!(status, Ok(0));
        assert_eq!(stdout, text.as_bytes(), "{args:?}");
    }
    let parsed = parse(&registry, &["zsvc", "get-thing", "--thing-id", "refused"]);
    let Resolution::Operation(operation) = registry.resolve(&["zsvc", "get-thing"]) else {
        panic!("get-thing resolves");
    };
    assert!(matches!(
        operation.bind(&parsed),
        Err(Admission::Python(PythonReason::UnsupportedValue))
    ));
}

#[test]
fn a_model_operation_formats_its_output() {
    let source = Fake;
    let registry = registry(&source);
    let Resolution::Operation(operation) = registry.resolve(&["zsvc", "list-things"]) else {
        panic!("list-things resolves");
    };
    assert_eq!(operation.streams(), StreamPolicy::Formatted);
}
