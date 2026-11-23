//! Ordered AWS CLI parser stages, raw boundary, and completion.
use crate::close_matches;
use crate::failure::{Failure, FailureKind, Suggestion};
use crate::parser;
use crate::pyrepr;
use crate::tables::{self, CommandKind, Node, Roots, Tables};
use crate::token::{self, Token};
use crate::unpack::{self, Environment};
use crate::{Action, Argument, Namespace, Parser, Value};
/// argparse's default width: shutil's 80-column fallback minus a two-column
/// margin.
pub const FORMATTER_WIDTH_DEFAULT: usize = 78;
/// First-pass formatter inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The host's argv[0], whose basename the first-pass usage names. It
    /// stays an operating-system string, as `parse_os` keeps the arguments.
    pub prog: std::ffi::OsString,
    pub formatter_width: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            prog: "aws".into(),
            formatter_width: FORMATTER_WIDTH_DEFAULT,
        }
    }
}
/// Stage reached in upstream order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    AutoPrompt,
    FirstPass,
    Main,
    Dispatch,
    TopLevel,
    Probe,
    Table,
    Unpack,
    Leftover,
}
impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::AutoPrompt => "auto_prompt",
            Self::FirstPass => "first_pass",
            Self::Main => "main",
            Self::Dispatch => "dispatch",
            Self::TopLevel => "top_level",
            Self::Probe => "probe",
            Self::Table => "table",
            Self::Unpack => "unpack",
            Self::Leftover => "leftover",
        }
    }
}
/// Parse end-state; invalid argv is represented as a failure outcome.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Outcome {
    #[default]
    Parsed,
    Help,
    Version,
    /// A global option's value is a raw operating-system value other than an
    /// endpoint URL with a scheme, which client creation judges, and global
    /// resolution found no failure first. The native handlers carry Unicode
    /// globals only, so the router maps this outcome to a PythonReason and
    /// upstream runs the command, decoding the value with surrogate escapes.
    UnsupportedOsValue,
    Unresolved {
        command: String,
        remaining: Vec<Token>,
    },
    Failure(Failure),
}
/// One successful parser boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Level<'a> {
    pub table: Vec<Argument<'a>>,
    pub namespace: Namespace,
    pub seen: Vec<bool>,
    pub remaining: Vec<Token>,
}
impl Level<'_> {
    pub fn was_supplied(&self, name: &str) -> bool {
        self.table
            .iter()
            .zip(&self.seen)
            .find(|(a, _)| a.name == name)
            .is_some_and(|(_, s)| *s)
    }
}
/// Owned namespaces, values, lineage, and failure state.
#[derive(Debug, Clone, Default)]
pub struct ParseResult<'a> {
    pub first_pass: Level<'a>,
    pub globals: Level<'a>,
    pub lineage: Vec<Node<'a>>,
    pub args: Level<'a>,
    pub values: Namespace,
    pub failed: Option<Namespace>,
    pub stopped_dest: Option<String>,
    pub query_syntax_skipped: bool,
    pub stage: Stage,
    pub outcome: Outcome,
}
/// Parse host argv through the raw argument-table boundary.
pub fn parse<'a>(argv: &[String], options: &Options, tables: &Tables<'a>) -> ParseResult<'a> {
    let arguments: Vec<_> = argv.iter().map(std::ffi::OsString::from).collect();
    parse_os(&arguments, options, tables)
}
/// Parse native arguments without decoding filesystem operands as Unicode.
/// The command word is resolved through the tables' lookup; a word it does
/// not define leaves the parse unresolved, before global resolution.
pub fn parse_os<'a>(
    argv: &[std::ffi::OsString],
    options: &Options,
    tables: &Tables<'a>,
) -> ParseResult<'a> {
    assert!(argv.len() <= crate::ARG_COUNT_MAX);
    assert!(argv.iter().all(|a| a.len() <= crate::ARG_LEN_MAX));
    let argv = token::from_os(argv);
    let argv = argv.as_slice();
    let mut result = ParseResult::default();
    if argv.iter().any(|a| a == "--cli-auto-prompt")
        && argv.iter().any(|a| a == "--no-cli-auto-prompt")
    {
        result.outcome = Outcome::Failure(Failure::new(FailureKind::AutoPromptConflict));
        return result;
    }
    result.stage = Stage::FirstPass;
    let mut config = parser::Options::new(first_pass_usage(options, false));
    config.colored_usage = Some(first_pass_usage(options, true));
    let Some(first) = level(&mut result, tables::FIRST_PASS, argv, config) else {
        return result;
    };
    result.first_pass = first;
    result.stage = Stage::Main;
    let Some(globals) = main_level(&mut result, tables, argv) else {
        return result;
    };
    result.globals = globals;
    result.stage = Stage::Dispatch;
    let command = result
        .globals
        .namespace
        .string("command")
        .unwrap_or_default()
        .to_owned();
    let Some(root) = tables.roots.root(&command) else {
        result.outcome = Outcome::Unresolved {
            command,
            remaining: result.globals.remaining.clone(),
        };
        return result;
    };
    result.lineage.push(root);
    result.stage = Stage::TopLevel;
    if !resolve_globals(&mut result) {
        return result;
    }
    let args = result.globals.remaining.clone();
    parse_command(&mut result, tables.roots, &args);
    result
}
/// Construct MainArgParser's conflict-resolving table: every global, the
/// resolving --version, and the command.
pub fn main_table<'a>(tables: &Tables<'a>) -> Vec<Argument<'a>> {
    let mut table = Vec::with_capacity(tables.globals.len() + tables::MAIN_TABLE_EXTRA_COUNT);
    table.extend_from_slice(tables.globals);
    table.push(Argument {
        action: Action::Version,
        ..Argument::new("version")
    });
    table.push(Argument {
        positional: true,
        ..Argument::new("command")
    });
    table
}
/// MainArgParser's table in help mode, which renders help without the
/// command or any global's value: each option that takes one value takes it
/// optionally, and so does the command (argparser.py `MainArgParser._build`).
/// A global takes a value by storing or appending it.
pub fn help_mode_table(mut table: Vec<Argument<'_>>) -> Vec<Argument<'_>> {
    for argument in &mut table {
        let takes_value = matches!(argument.action, Action::Store | Action::Append);
        if argument.positional || (argument.nargs.is_none() && takes_value) {
            argument.nargs = Some(crate::Nargs::Optional);
        }
    }
    table
}
/// MainArgParser.parse_known_args: a parse that fails is parsed again in
/// help mode, whose result stands only when it asks for help, so `--help`
/// renders help whatever else fails to parse (argparser.py
/// `_parse_help_request`). Any other retry keeps the first failure; a
/// `--version` the retry reaches exits as it would have.
fn main_level<'a>(
    result: &mut ParseResult<'a>,
    tables: &Tables<'a>,
    argv: &[Token],
) -> Option<Level<'a>> {
    let table = main_table(tables);
    let options = || parser::Options::new(tables::USAGE);
    let globals = level(result, &table, argv, options());
    if globals.is_some() || !matches!(result.outcome, Outcome::Failure(_)) {
        return globals;
    }
    let mut retry = ParseResult::default();
    match level(&mut retry, &help_mode_table(table), argv, options()) {
        Some(globals) if globals.namespace.boolean("help") => {
            result.failed = None;
            result.outcome = Outcome::Parsed;
            Some(globals)
        }
        None if retry.outcome == Outcome::Version => {
            result.failed = None;
            result.outcome = Outcome::Version;
            None
        }
        _ => None,
    }
}
/// Build a parent table with its optional subcommand positional.
pub fn command_table<'a>(command: &Node<'a>) -> Vec<Argument<'a>> {
    let mut table = command.arguments.to_vec();
    if command.subcommands.is_some() {
        table.push(Argument {
            name: "subcommand",
            positional: true,
            action: Action::Command,
            nargs: Some(crate::Nargs::Optional),
            choices: &[],
            ..Argument::new("subcommand")
        });
    }
    table
}
fn level<'a>(
    result: &mut ParseResult<'a>,
    table: &[Argument<'a>],
    args: &[Token],
    options: parser::Options<'a>,
) -> Option<Level<'a>> {
    let parsed_table = Parser::new(table, options);
    assert!(parsed_table.is_ok(), "supplied parser table is invalid");
    let Ok(parser) = parsed_table else {
        return None;
    };
    match parser.parse_tokens(args) {
        Err(failed) => {
            result.failed = Some(failed.namespace);
            result.outcome = Outcome::Failure(Failure::parsing(failed.failure));
            None
        }
        Ok(parsed) => {
            if parsed.version {
                result.outcome = Outcome::Version;
                return None;
            }
            Some(Level {
                table: parser.arguments,
                namespace: parsed.namespace,
                seen: parsed.seen,
                remaining: parsed.remaining,
            })
        }
    }
}
/// Parse the lineage's last command from `args`, descending into the
/// child its subcommand or operation names.
fn parse_command<'a>(result: &mut ParseResult<'a>, roots: &dyn Roots<'a>, args: &[Token]) {
    let Some(command) = result.lineage.last().cloned() else {
        return;
    };
    if command.kind == CommandKind::Service {
        parse_service(result, roots, &command, args);
        return;
    }
    if let Some(children) = &command.subcommands {
        result.stage = Stage::Probe;
        let mut options = parser::Options::new(tables::USAGE);
        options.help_sentinel = true;
        options.ignore_required = true;
        options.command_choices = Some(children.to_vec());
        let Some(probe) = level(result, &command_table(&command), args, options) else {
            return;
        };
        if let Some(name) = probe.namespace.string("subcommand")
            && children.contains(&name)
        {
            let mut remaining = args.to_vec();
            if let Some(index) = remaining.iter().position(|value| value.is_text(name)) {
                remaining.remove(index);
            }
            if descend(result, roots, name, &remaining) {
                return;
            }
        }
    }
    parse_table(result, &command, args);
}
/// ServiceCommand.__call__: ServiceArgParser parses the operation name,
/// its one argument, whose choices are the service's operations and
/// `help`. Under `--help` the name is optional (`help_mode`), and an
/// absent name shows the service's help, as `help` does.
fn parse_service<'a>(
    result: &mut ParseResult<'a>,
    roots: &dyn Roots<'a>,
    command: &Node<'a>,
    args: &[Token],
) {
    result.stage = Stage::Probe;
    let help_mode = result.globals.namespace.boolean("help");
    let table = [Argument {
        positional: true,
        action: Action::Command,
        nargs: help_mode.then_some(crate::Nargs::Optional),
        ..Argument::new("operation")
    }];
    let mut choices = command.subcommands.as_deref().unwrap_or_default().to_vec();
    choices.push("help");
    let mut options = parser::Options::new(tables::USAGE);
    options.command_choices = Some(choices);
    let Some(parsed) = level(result, &table, args, options) else {
        return;
    };
    match parsed.namespace.string("operation") {
        None | Some("help") => result.outcome = Outcome::Help,
        Some(name) => {
            if !descend(result, roots, name, &parsed.remaining) {
                result.outcome = Outcome::Unresolved {
                    command: name.to_owned(),
                    remaining: parsed.remaining.clone(),
                };
            }
        }
    }
}
/// Push the child `name` of the lineage's last command and parse it from
/// `args`; false when the lookup does not supply that child.
fn descend<'a>(
    result: &mut ParseResult<'a>,
    roots: &dyn Roots<'a>,
    name: &str,
    args: &[Token],
) -> bool {
    let child = {
        let lineage: Vec<&str> = result.lineage.iter().map(|node| &*node.name).collect();
        roots.subcommand(&lineage, name)
    };
    let Some(child) = child else {
        return false;
    };
    assert!(result.lineage.len() < crate::LINEAGE_DEPTH_MAX);
    result.lineage.push(child);
    parse_command(result, roots, args);
    true
}
/// The command's own table: BasicCommand.__call__'s ArgTableArgParser, or
/// ServiceOperation.__call__'s, which adds an optional `help` positional
/// (`_add_help`) whose value, other than `help`, joins the end of the
/// leftovers.
fn parse_table<'a>(result: &mut ParseResult<'a>, command: &Node<'a>, args: &[Token]) {
    result.stage = Stage::Table;
    // BasicCommand.__call__ renders `--help` before it parses its table, and
    // ServiceOperation.__call__ before it parses its own, so no value of
    // this command's can fail it.
    if result.globals.namespace.boolean("help") {
        result.outcome = Outcome::Help;
        return;
    }
    let operation = command.kind == CommandKind::Operation;
    let mut table = command_table(command);
    if operation {
        table.push(Argument {
            positional: true,
            nargs: Some(crate::Nargs::Optional),
            ..Argument::new("help")
        });
    }
    let mut options = parser::Options::new(tables::USAGE);
    options.help_sentinel = true;
    options.ignore_required = command.overrides.iter().any(|lift| lift.applies(args));
    options.command_choices = command.subcommands.as_deref().map(<[&str]>::to_vec);
    let Some(mut parsed) = level(result, &table, args, options) else {
        return;
    };
    let help = if operation {
        operation_help(&mut parsed, args)
    } else {
        parsed.namespace.string("help").is_some()
    };
    result.outcome = if help { Outcome::Help } else { Outcome::Parsed };
    result.args = parsed;
}
/// ServiceOperation.__call__ on the `help` positional: `help` asks for the
/// operation's help page, and any other value is appended to the leftovers.
/// The positional is the parser's, not the table's, so it leaves the level.
fn operation_help(parsed: &mut Level<'_>, args: &[Token]) -> bool {
    let value = parsed.namespace.value("help").cloned();
    parsed
        .namespace
        .entries
        .retain(|entry| entry.dest != "help");
    if let Some(index) = parsed
        .table
        .iter()
        .position(|argument| argument.positional && argument.name == "help")
    {
        parsed.table.remove(index);
        parsed.seen.remove(index);
    }
    let Some(text) = value.as_ref().and_then(Value::as_os_str) else {
        return false;
    };
    if text == "help" {
        return true;
    }
    let consumed = args.iter().find(|token| {
        token.as_os_str() == text
            && !parsed
                .remaining
                .iter()
                .any(|left| left.original_index == token.original_index)
    });
    if let Some(token) = consumed {
        parsed.remaining.push(token.clone());
    }
    false
}
/// globalargs.resolve_types in upstream's order: the endpoint URL, then
/// verify_ssl, then the timeouts, whose argv bytes render as stderr writes
/// them when they fail. A global that is still not Unicode after these
/// succeed is then handed to Python, which runs the command.
fn resolve_globals(result: &mut ParseResult) -> bool {
    let ns = &mut result.globals.namespace;
    result.query_syntax_skipped = ns.is_explicit("query");
    if let Some(kind) = ns.os_string("endpoint_url").and_then(endpoint_url_failure) {
        result.outcome = Outcome::Failure(Failure::new(kind));
        return false;
    }
    if ns.boolean("verify_ssl") {
        ns.put(
            "verify_ssl",
            ns.value("ca_bundle").cloned().unwrap_or(Value::Null),
            false,
        );
    }
    for dest in ["read_timeout", "connect_timeout"] {
        let value = match ns.os_string(dest).map(unpack::parse_int_os) {
            Some(Ok(value)) => value,
            Some(Err(failure)) => {
                result.outcome = Outcome::Failure(failure);
                result.stopped_dest = Some(dest.into());
                return false;
            }
            None => 60,
        };
        ns.put(
            dest,
            if value == 0 {
                Value::Null
            } else {
                value.into()
            },
            false,
        );
    }
    if ns
        .entries
        .iter()
        .any(|entry| entry.value.contains_os_string() && !endpoint_url_with_scheme(entry))
    {
        result.outcome = Outcome::UnsupportedOsValue;
        return false;
    }
    true
}
fn endpoint_url_with_scheme(entry: &crate::namespace::Entry) -> bool {
    entry.dest == "endpoint_url"
        && entry
            .value
            .as_os_str()
            .is_some_and(|url| url_scheme(url.as_encoded_bytes()))
}
fn url_scheme(raw: &[u8]) -> bool {
    scheme_end(&url_text(raw)).is_some()
}
/// How globalargs._resolve_endpoint_url fails `url`: its `urlparse` raises
/// for the brackets of an authority ([`bracketed_authority`]), whether or
/// not a scheme leads to it, and a URL it reads must have a scheme. The
/// authority runs from a `//` to the first `/`, `?`, or `#`.
fn endpoint_url_failure(url: &std::ffi::OsStr) -> Option<FailureKind> {
    let text = url_text(url.as_encoded_bytes());
    let scheme = scheme_end(&text);
    let rest = scheme.map_or(&text[..], |end| &text[end + 1..]);
    if let Some(rest) = rest.strip_prefix(b"//") {
        let end = rest.iter().position(|byte| b"/?#".contains(byte));
        if let Err(raised) = bracketed_authority(&rest[..end.unwrap_or(rest.len())]) {
            return Some(FailureKind::EndpointUrlBrackets(raised));
        }
    }
    scheme
        .is_none()
        .then(|| FailureKind::EndpointUrlScheme(url.to_os_string()))
}
/// A URL as `urlsplit` starts on it: past the C0 controls and spaces it
/// leads with, and without a tab or line break.
fn url_text(raw: &[u8]) -> Vec<u8> {
    let start = raw.iter().position(|&byte| byte > b' ');
    raw[start.unwrap_or(raw.len())..]
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b'\t' | b'\r' | b'\n'))
        .collect()
}
/// Where the scheme `urlsplit` finds in [`url_text`]'s `url` ends: an ASCII
/// letter, then letters, digits, `+`, `-`, or `.`, up to the first colon.
fn scheme_end(url: &[u8]) -> Option<usize> {
    let end = url.iter().position(|&byte| byte == b':')?;
    let scheme = &url[..end];
    (scheme.first()?.is_ascii_alphabetic()
        && scheme
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')))
    .then_some(end)
}
/// What `urlsplit` raises for an authority with a bracket: a ValueError's
/// text. Brackets come as a pair and hold the whole host, after any userinfo
/// and ahead of any `:port`. With the pair in the userinfo alone, the host it
/// judges is the text up to the first colon.
fn bracketed_authority(authority: &[u8]) -> Result<(), String> {
    const INVALID: &str = "Invalid IPv6 URL";
    let (open, close) = (authority.contains(&b'['), authority.contains(&b']'));
    if !open && !close {
        return Ok(());
    }
    if open != close {
        return Err(INVALID.into());
    }
    let host_port = authority.rsplit(|&byte| byte == b'@').next();
    let host_port = host_port.unwrap_or(authority);
    let host = match host_port.iter().position(|&byte| byte == b'[') {
        Some(0) => {
            let mut parts = host_port[1..].splitn(2, |&byte| byte == b']');
            let host = parts.next().unwrap_or_default();
            let port = parts.next().unwrap_or_default();
            if !port.is_empty() && !port.starts_with(b":") {
                return Err(INVALID.into());
            }
            host
        }
        Some(_) => return Err(INVALID.into()),
        None => host_port
            .split(|&byte| byte == b':')
            .next()
            .unwrap_or_default(),
    };
    bracketed_host(host)
}
/// `urlsplit`'s check of the host brackets hold: an IPvFuture address, `v`,
/// hexadecimal digits, a dot, and more, or an IPv6 address as `ipaddress`
/// reads one. A `%zone` after the address is any text without a second `%`
/// (`ipaddress.IPv6Address._split_scope_id`), which need not be Unicode.
fn bracketed_host(host: &[u8]) -> Result<(), String> {
    if let [b'v' | b'V', future @ ..] = host {
        let digits = future.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        let named = digits > 0 && future.get(digits) == Some(&b'.') && future.len() > digits + 1;
        return if named {
            Ok(())
        } else {
            Err("IPvFuture address is invalid".into())
        };
    }
    let reads_as =
        |text: &[u8], address: fn(&str) -> bool| std::str::from_utf8(text).is_ok_and(address);
    if reads_as(host, |text| text.parse::<std::net::Ipv4Addr>().is_ok()) {
        return Err("An IPv4 address cannot be in brackets".into());
    }
    let mut parts = host.splitn(2, |&byte| byte == b'%');
    let address = parts.next().unwrap_or_default();
    let zoned = parts
        .next()
        .is_none_or(|zone| !zone.is_empty() && !zone.contains(&b'%'));
    if zoned && reads_as(address, |text| text.parse::<std::net::Ipv6Addr>().is_ok()) {
        return Ok(());
    }
    let mut repr = Vec::with_capacity(host.len() + 2);
    pyrepr::write(host, &mut repr);
    Err(format!(
        "{} does not appear to be an IPv4 or IPv6 address",
        String::from_utf8_lossy(&repr)
    ))
}
/// Complete a parsed result; true means a value was explicitly deferred to Python.
/// BasicCommand processes its values before it reports leftovers;
/// ServiceOperation.__call__ raises its UnknownArgumentError first and
/// builds its call parameters after (`_build_call_parameters`).
pub fn complete(result: &mut ParseResult<'_>, environment: &Environment<'_>) -> bool {
    assert_eq!(result.outcome, Outcome::Parsed);
    assert_eq!(result.stage, Stage::Table);
    assert!(!result.lineage.is_empty(), "parsed command has no lineage");
    let Some(kind) = result.lineage.last().map(|command| command.kind) else {
        return false;
    };
    if kind == CommandKind::Operation {
        return !leftover_failure(result) && unpack_values(result, environment);
    }
    if unpack_values(result, environment) {
        return true;
    }
    if result.outcome == Outcome::Parsed && !leftover_failure(result) && result.lineage.len() == 1 {
        // S3._run_main with no subcommand: the root's own usage failure.
        let lineage = result
            .lineage
            .iter()
            .map(|command| command.name.to_string())
            .collect();
        result.outcome = Outcome::Failure(Failure::new(FailureKind::TooFewArguments(lineage)));
    }
    false
}
/// The unpack stage; true when a value was deferred to Python. A value's
/// failure becomes the outcome.
fn unpack_values(result: &mut ParseResult<'_>, environment: &Environment<'_>) -> bool {
    let Some(command) = result.lineage.last() else {
        return false;
    };
    result.stage = Stage::Unpack;
    match unpack::unpack(&command.arguments, &result.args.namespace, environment) {
        Ok(values) => {
            result.values = values;
            false
        }
        Err(e) => {
            result.stopped_dest = Some(e.dest);
            if let Some(failure) = e.failure {
                result.outcome = Outcome::Failure(failure);
                return false;
            }
            true
        }
    }
}
/// The leftover stage: tokens no table consumed are the outcome's failure,
/// which the command's kind assigns its handler. True when there were any.
fn leftover_failure(result: &mut ParseResult<'_>) -> bool {
    let Some(command) = result.lineage.last() else {
        return false;
    };
    result.stage = Stage::Leftover;
    if result.args.remaining.is_empty() {
        return false;
    }
    let remaining: Vec<_> = result.args.remaining.iter().map(Token::as_os_str).collect();
    result.outcome = Outcome::Failure(Failure::new(FailureKind::UnknownOptions {
        suggestions: unknown_option_suggestions(&remaining, &command.arguments),
        tokens: remaining
            .into_iter()
            .map(std::ffi::OsStr::to_os_string)
            .collect(),
        kind: command.kind,
    }));
    true
}
/// argparser.format_unknown_options_error's suggestions: each `--` token's
/// name, before any `=`, against get_option_suggestion_candidates, which
/// lists the command's options and then cli.json's globals. Repeated lines
/// appear once.
/// Tokens that are not Unicode compare as their bytes.
fn unknown_option_suggestions(
    tokens: &[&std::ffi::OsStr],
    arguments: &[Argument],
) -> Vec<Suggestion> {
    let candidates: Vec<&str> = arguments
        .iter()
        .filter(|argument| !argument.positional && !argument.hidden)
        .chain(tables::GLOBALS)
        .map(|argument| argument.name)
        .collect();
    let mut suggestions = Vec::new();
    for token in tokens {
        let Some(option) = token.as_encoded_bytes().strip_prefix(b"--") else {
            continue;
        };
        let end = option.iter().position(|byte| *byte == b'=');
        let name = &option[..end.unwrap_or(option.len())];
        for matched in close_matches::close_matches(name, &candidates) {
            let suggestion = Suggestion {
                matched: matched.into(),
                name: name.to_vec(),
            };
            if !suggestions.contains(&suggestion) {
                suggestions.push(suggestion);
            }
        }
    }
    suggestions
}
/// `os.path.basename(sys.argv[0])`, argparse's default prog, as CPython's
/// stderr writes it, and its length in code points. surrogateescape decodes
/// each undecodable byte to one lone surrogate, which counts as one code
/// point and which the backslashreplace error handler writes as `\udcXX`.
/// The bytes are the program name's encoded ones. On Windows `os.path` is
/// `ntpath`, whose basename drops a drive letter's `X:` and splits at either
/// separator.
fn prog_name(prog: &std::ffi::OsStr) -> (String, usize) {
    let bytes = prog.as_encoded_bytes();
    let name = if cfg!(windows) {
        let path = match bytes {
            [_, b':', rest @ ..] => rest,
            _ => bytes,
        };
        path.rsplit(|byte| matches!(byte, b'/' | b'\\'))
            .next()
            .unwrap_or(path)
    } else {
        bytes.rsplit(|byte| *byte == b'/').next().unwrap_or(bytes)
    };
    (
        pyrepr::backslash_replaced(name),
        pyrepr::code_point_count(name),
    )
}
/// Generated first-pass usage keeps option groups intact at wrap boundaries.
pub fn first_pass_usage(options: &Options, color: bool) -> String {
    let (prog, prog_len) = prog_name(&options.prog);
    let width = options.formatter_width;
    let parts = ["[--profile PROFILE]", "[--debug]"];
    let colored = [
        "[\x1b[36m--profile \x1b[33mPROFILE\x1b[0m]",
        "[\x1b[36m--debug\x1b[0m]",
    ];
    let wraps = 7 + prog_len + 2 + parts[0].len() + parts[1].len() > width;
    let short = 7 + prog_len <= width - width.div_ceil(4);
    let indent = if short { 7 + prog_len + 1 } else { 7 };
    let mut output = if color {
        format!("\x1b[1;34musage: \x1b[0m\x1b[1;35m{prog}\x1b[0m")
    } else {
        prog
    };
    let mut line_len = 7 + prog_len;
    for (i, part) in parts.iter().enumerate() {
        let newline = wraps && ((i == 0 && !short) || line_len + 1 + part.len() > width);
        if newline {
            output.push('\n');
            output.push_str(&" ".repeat(indent));
            line_len = indent;
        } else {
            output.push(' ');
            line_len += 1;
        }
        output.push_str(if color { colored[i] } else { part });
        line_len += part.len();
    }
    if color {
        output.push('\n');
    }
    output
}
#[cfg(test)]
mod tests;
