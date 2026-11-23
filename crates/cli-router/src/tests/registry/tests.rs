//! The generic route: a root and its operations that only a registry's
//! `OperationSource` knows, through the same ordered stages as the pinned
//! `s3` commands, to `Verdict::Run`.
use super::*;
use ::command::{Admission, Context, Invocation, Needs, OperationSource, Registry, StreamPolicy};
use awscli_argparse::Argument;
use cli_core::cli_error::{Category, CommandFailure, Rendering};
use std::borrow::Cow;

/// An invocation that writes what it was bound with.
#[derive(Debug)]
struct Echo(String);
impl Invocation for Echo {
    fn run(&self, cx: &mut Context<'_>) -> Result<u8, CommandFailure> {
        writeln!(cx.stdout, "{}", self.0).map_err(|_| CommandFailure::new(Category::General))?;
        Ok(0)
    }
}

/// The operation names of the payload's sts model through `xform_name`,
/// in model order, as the Python route's ServiceArgParser lists them.
const STS_OPERATIONS: &[&str] = &[
    "assume-role",
    "assume-role-with-saml",
    "assume-role-with-web-identity",
    "assume-root",
    "decode-authorization-message",
    "get-access-key-info",
    "get-caller-identity",
    "get-delegated-access-token",
    "get-federation-token",
    "get-session-token",
    "get-web-identity-token",
];
const ASSUME_ROLE: &[Argument] = &[
    Argument {
        required: true,
        ..Argument::new("role-arn")
    },
    Argument {
        required: true,
        ..Argument::new("role-session-name")
    },
    Argument::new("policy"),
    Argument {
        cli_type: Some(awscli_argparse::CliType::Deferred),
        nargs: Some(awscli_argparse::Nargs::ZeroOrMore),
        ..Argument::new("tags")
    },
];

/// A stand-in for the model source (`command::models`): `sts` with its real
/// operation names, two of them with tables.
#[derive(Debug)]
struct Fake;
impl OperationSource for Fake {
    fn is_root(&self, name: &str) -> bool {
        name == "sts"
    }
    fn operations(&self, root: &str) -> Option<Cow<'_, [&str]>> {
        self.is_root(root).then_some(Cow::Borrowed(STS_OPERATIONS))
    }
    fn arguments(&self, root: &str, operation: &str) -> Option<Cow<'_, [Argument<'_>]>> {
        if !self.is_root(root) {
            return None;
        }
        match operation {
            "assume-role" => Some(ASSUME_ROLE.to_vec().into()),
            name if STS_OPERATIONS.contains(&name) => Some(Cow::Borrowed(&[])),
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
    /// Every table but get-session-token's, which stands for one a
    /// customization edits.
    fn reproduces_failures(&self, _root: &str, operation: Option<&str>) -> bool {
        operation != Some("get-session-token")
    }
    fn paginated(&self, _root: &str, operation: &str) -> bool {
        operation == "decode-authorization-message"
    }
    fn needs(&self, root: &str) -> Needs {
        Needs::Client {
            service: root.to_owned().into(),
        }
    }
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        match parsed.values.string("role_session_name") {
            Some("refused") => Err(Admission::Python(PythonReason::UnsupportedOption)),
            Some(name) => Ok(Box::new(Echo(name.to_owned()))),
            None => Ok(Box::new(Echo("identity".into()))),
        }
    }
}

static PINNED: [&tables::Command<'static>; 1] = [&tables::S3];

fn evaluate_in(
    args: &[&str],
    env: &BTreeMap<String, String>,
    files: &ParsedFiles,
) -> (Verdict, ParseResult<'static>) {
    const REGISTRY: Registry<'static> = Registry::new(&PINNED, Some(&Fake));
    let argv = args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let mut result = awscli_argparse::parse(&argv, &Options::default(), &REGISTRY.tables());
    let vars = env
        .iter()
        .map(|(name, value)| (name.clone(), value.into()))
        .collect();
    let verdict = route(
        &mut result,
        &unpack::Environment {
            directory: Path::new("."),
            variables: &vars,
            files: &unpack::FsParamfiles,
            preferred_encoding: unpack::PreferredEncoding::Locale,
        },
        &Configuration {
            env,
            shared_files: files,
            boto_config_credentials: false,
            embedded_trust: true,
            registry: &REGISTRY,
        },
    );
    (verdict, result)
}
fn evaluate_with(args: &[&str], env: &BTreeMap<String, String>, files: &ParsedFiles) -> Verdict {
    evaluate_in(args, env, files).0
}
fn run_of(verdict: Verdict) -> Box<Run> {
    match verdict {
        Verdict::Run(run) => run,
        other => panic!("{other:?}, expected Verdict::Run"),
    }
}
fn output(run: &Run) -> Vec<u8> {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = run.invocation.run(&mut Context {
        env: &BTreeMap::<String, String>::new(),
        shared_files: &files(""),
        globals: &run.globals,
        directory: Path::new("."),
        stdout: &mut stdout,
        stderr: &mut stderr,
    });
    assert_eq!(status, Ok(0));
    stdout
}

#[test]
fn a_root_only_the_registry_knows_parses_binds_and_runs() {
    let run = run_of(evaluate_with(
        &["--region", "us-west-2", "sts", "get-caller-identity"],
        &BTreeMap::new(),
        &files(""),
    ));
    assert_eq!(
        run.needs,
        Needs::Client {
            service: "sts".into()
        }
    );
    assert_eq!(run.streams, StreamPolicy::Formatted);
    assert_eq!(run.globals.region.as_deref(), Some("us-west-2"));
    assert_eq!(output(&run), b"identity\n");
    let args = [
        "sts",
        "assume-role",
        "--role-arn",
        "arn",
        "--role-session-name",
        "session",
    ];
    assert_eq!(
        output(&run_of(evaluate_with(&args, &BTreeMap::new(), &files("")))),
        b"session\n"
    );
}

#[test]
fn parse_failures_of_a_derived_command_render_as_python_does() {
    const HELP: &str = "usage: aws [options] <command> <subcommand> [<subcommand> ...] \
        [parameters]\nTo see help text, you can run:\n\n  aws help\n  aws <command> help\n  \
        aws <command> <subcommand> help\n";
    let validation = "\naws: [ERROR]: An error occurred (ParamValidation): ";
    for (args, expected) in [
        (
            &["sts", "nosuch"][..],
            format!("{validation}argument operation: Found invalid choice 'nosuch'\n\n\n{HELP}"),
        ),
        (
            &["sts", "get-caller-identity", "--nosuch"],
            format!("\n{HELP}\n\naws: [ERROR]: Unknown options: --nosuch\n"),
        ),
        (
            &["sts", "assume-role", "--role-arn", "x"],
            format!(
                "{validation}the following arguments are required: --role-session-name\n\n{HELP}"
            ),
        ),
    ] {
        let Verdict::Failure(failure) = evaluate_with(args, &BTreeMap::new(), &files("")) else {
            panic!("{args:?} fails natively");
        };
        let mut stderr = Vec::new();
        let rendering = Rendering {
            format: cli_core::cli_error::ErrorFormat::Enhanced,
            color: Default::default(),
        };
        cli_core::cli_error::render(&mut stderr, &failure, rendering).unwrap_or_else(|_| {
            panic!("{args:?} renders");
        });
        assert_eq!(String::from_utf8_lossy(&stderr), expected, "{args:?}");
        let category = cli_core::cli_error::Category::from_class(failure.class);
        assert_eq!(cli_core::cli_error::exit_status(category), 252, "{args:?}");
    }
}

/// The leftover check and the unpack loop run after the parse, as part of
/// completing it; a failure there of a table a customization edits is
/// Python's to report, as the parse's own would be.
#[test]
fn a_customized_table_s_late_failures_are_python_s() {
    let verdict = evaluate_with(
        &["sts", "get-session-token", "--nosuch"],
        &BTreeMap::new(),
        &files(""),
    );
    assert!(
        matches!(
            verdict,
            Verdict::Python(PythonReason::UnsupportedSubcommand)
        ),
        "{verdict:?}"
    );
}

#[test]
fn an_alias_naming_the_root_hands_it_to_python() {
    let aliases = |text: &str| {
        ParsedFiles::parse(awscli_config::Inputs {
            aliases: Some(awscli_config::Contents {
                source: "alias",
                bytes: text.as_bytes(),
            }),
            ..Default::default()
        })
    };
    for alias in [
        "[toplevel]\nsts = !replacement\n",
        "[command sts]\nwhoami = get-caller-identity\n",
        "[command sts get-caller-identity]\nfoo = bar\n",
    ] {
        for args in [&["sts", "get-caller-identity"][..], &["sts", "nosuch"]] {
            assert!(
                matches!(
                    evaluate_with(args, &BTreeMap::new(), &aliases(alias)),
                    Verdict::Python(PythonReason::CommandAlias)
                ),
                "{alias:?} {args:?}"
            );
        }
    }
    for alias in [
        "[toplevel]\ns3 = !replacement\n",
        "[command sts]\n",
        "[command STS]\nfoo = bar\n",
    ] {
        let verdict = evaluate_with(
            &["sts", "get-caller-identity"],
            &BTreeMap::new(),
            &aliases(alias),
        );
        assert!(matches!(verdict, Verdict::Run(_)), "{alias:?}: {verdict:?}");
    }
}

/// One refusal: argv, the environment, the config file, and the reason.
type Refusal<'a> = (
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
    &'a str,
    PythonReason,
);
const IDENTITY: &[&str] = &["sts", "get-caller-identity"];
fn assert_refused(cases: &[Refusal<'_>]) {
    for &(args, variables, config, reason) in cases {
        let env = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let verdict = evaluate_with(args, &env, &files(config));
        assert!(
            matches!(verdict, Verdict::Python(actual) if actual == reason),
            "{args:?} {variables:?} {config:?}: {verdict:?}, expected {reason:?}"
        );
    }
}

/// Each stage of the route refuses ahead of `Run`, in the order the s3
/// commands meet them; these are the stages argv decides, after the
/// session fallback.
#[test]
fn the_prologue_and_the_command_line_refuse_before_run() {
    assert_refused(&[
        (IDENTITY, &[], "[plugins]\nx = y\n", PythonReason::Plugins),
        (IDENTITY, &[], "[broken", PythonReason::ConfigParseFailure),
        (
            &["ec2", "describe-instances"],
            &[],
            "",
            PythonReason::UnsupportedCommand,
        ),
        (&["sts", "help"], &[], "", PythonReason::HelpRequested),
        (
            &["--help", "sts", "get-caller-identity"],
            &[],
            "",
            PythonReason::HelpRequested,
        ),
        (
            &["--query", "Account", "sts", "get-caller-identity"],
            &[],
            "",
            PythonReason::PythonOnlyGlobal,
        ),
        // A paginated operation pages unless --no-paginate says not to,
        // which only Python does.
        (
            &["--no-paginate", "sts", "decode-authorization-message"],
            &[],
            "",
            PythonReason::PythonOnlyGlobal,
        ),
        (
            &["--no-verify-ssl", "sts", "get-caller-identity"],
            &[],
            "",
            PythonReason::PythonOnlyGlobal,
        ),
    ]);
}

/// Completion defers a value, or stops at an option only Python binds,
/// then binding refuses.
#[test]
fn completion_and_binding_refuse_before_run() {
    let python_bound = [
        "sts",
        "assume-role",
        "--role-arn",
        "a",
        "--role-session-name",
        "s",
        "--tags",
        "t",
    ];
    let deferred = [
        "sts",
        "assume-role",
        "--role-arn",
        "fileb://Cargo.toml",
        "--role-session-name",
        "s",
    ];
    let refused = [
        "sts",
        "assume-role",
        "--role-arn",
        "a",
        "--role-session-name",
        "refused",
    ];
    assert_refused(&[
        (&deferred, &[], "", PythonReason::UnsupportedValue),
        (&python_bound, &[], "", PythonReason::UnsupportedOption),
        (&refused, &[], "", PythonReason::UnsupportedOption),
    ]);
}

/// The settings stage, the output format, then the endpoint checks for
/// the service the bound command needs.
#[test]
fn settings_output_and_endpoint_refuse_before_run() {
    let output = PythonReason::OutputFormat;
    let endpoint = PythonReason::EndpointSetting;
    assert_refused(&[
        (
            IDENTITY,
            &[("AWS_CONFIG_FILE", "$HOME/config")],
            "",
            PythonReason::SharedFilePath,
        ),
        (
            IDENTITY,
            &[],
            "[default]\ncli_history = enabled\n",
            PythonReason::HistoryEnabled,
        ),
        (
            &["--output", "table", "sts", "get-caller-identity"],
            &[],
            "",
            output,
        ),
        (
            &["--output", "yaml", "sts", "get-caller-identity"],
            &[],
            "",
            output,
        ),
        (IDENTITY, &[], "[default]\noutput = table\n", output),
        (
            IDENTITY,
            &[],
            "[default]\noutput =\n  nested = yes\n",
            output,
        ),
        (
            IDENTITY,
            &[("AWS_DEFAULT_OUTPUT", "yaml-stream")],
            "",
            output,
        ),
        (
            IDENTITY,
            &[("AWS_DEFAULT_OUTPUT", "")],
            "[default]\noutput = json\n",
            output,
        ),
        (
            &["--region", "not a region", "sts", "get-caller-identity"],
            &[],
            "",
            endpoint,
        ),
        (
            IDENTITY,
            &[("AWS_ENDPOINT_URL_STS", "http://")],
            "",
            endpoint,
        ),
        (
            IDENTITY,
            &[("AWS_ENDPOINT_URL_STS", "ftp://host")],
            "",
            endpoint,
        ),
        (IDENTITY, &[], "[default]\nservices = missing\n", endpoint),
    ]);
}

#[test]
fn globals_are_classified_for_a_formatted_command() {
    for globals in [
        &["--output", "json"][..],
        &["--output", "text"],
        &["--output", "off"],
        &["--color", "on"],
        &["--no-cli-pager"],
        &["--debug"],
        &["--no-sign-request"],
        &["--cli-connect-timeout", "5"],
        &["--cli-error-format", "legacy"],
        // Python ignores it for an operation that does not paginate.
        &["--no-paginate"],
    ] {
        let args = [globals, &["sts", "get-caller-identity"]].concat();
        let verdict = evaluate_with(&args, &BTreeMap::new(), &files(""));
        assert!(matches!(verdict, Verdict::Run(_)), "{args:?}: {verdict:?}");
    }
    let config = "[default]\noutput = text\n";
    let verdict = evaluate_with(
        &["sts", "get-caller-identity"],
        &BTreeMap::new(),
        &files(config),
    );
    assert!(matches!(verdict, Verdict::Run(_)), "{verdict:?}");
    // The command line's format outranks the profile's.
    let verdict = evaluate_with(
        &["--output", "json", "sts", "get-caller-identity"],
        &BTreeMap::new(),
        &files("[default]\noutput = table\n"),
    );
    assert!(matches!(verdict, Verdict::Run(_)), "{verdict:?}");
}

#[test]
fn the_session_s_start_precedes_a_derived_command_s_failure_and_its_run() {
    let env = BTreeMap::from([("AWS_METADATA_SERVICE_TIMEOUT".to_owned(), "1.5".to_owned())]);
    for args in [
        &["sts", "get-caller-identity"][..],
        &["sts", "get-caller-identity", "--nosuch"],
        &["sts", "assume-role"],
        &["sts", "nosuch"],
        &[
            "sts",
            "assume-role",
            "--role-arn",
            "file://missing",
            "--role-session-name",
            "s",
        ],
    ] {
        match evaluate_with(args, &env, &files("")) {
            Verdict::Failure(failure) => {
                assert_eq!(failure.kind, "session_initialized", "{args:?}");
            }
            other => panic!("{args:?}: {other:?}"),
        }
    }
    let args = [
        "sts",
        "assume-role",
        "--role-arn",
        "file://missing",
        "--role-session-name",
        "s",
    ];
    match evaluate_with(&args, &BTreeMap::new(), &files("")) {
        Verdict::Failure(failure) => assert_eq!(failure.kind, "paramfile_load"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_main_parse_failure_after_a_registered_root_renders_natively() {
    for (args, native) in [
        (&["sts", "get-caller-identity", "--region"][..], true),
        (&["s3", "ls", "--region"], true),
        (&["ec2", "describe-instances", "--region"], false),
    ] {
        let verdict = evaluate_with(args, &BTreeMap::new(), &files(""));
        match (verdict, native) {
            (Verdict::Failure(failure), true) => assert_eq!(failure.kind, "parse", "{args:?}"),
            (Verdict::Python(PythonReason::UnsupportedCommand), false) => {}
            (other, _) => panic!("{args:?}: {other:?}"),
        }
    }
}

#[test]
fn the_pinned_s3_commands_keep_their_native_verdicts_beside_a_source() {
    let verdict = evaluate_with(&["s3", "ls", "s3://bucket"], &BTreeMap::new(), &files(""));
    let Verdict::Native(native) = verdict else {
        panic!("{verdict:?}");
    };
    assert!(matches!(native.command, Command::S3Ls(_)));
    assert!(matches!(
        evaluate_with(&["s3", "mb", "s3://bucket"], &BTreeMap::new(), &files("")),
        Verdict::Python(PythonReason::UnsupportedSubcommand)
    ));
}

mod settings;
