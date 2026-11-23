//! The Python customizations' event patterns, read from the payload entry
//! the pack step derives from the fallback's own source
//! (`deps/scripts/gen_customization_patterns.py`), and the events a model
//! operation's invocation emits, which decide the entries that touch it.
//!
//! An entry of the awscli registry is an initializer: the lazy emitter runs
//! it the first time an emitted event matches its pattern
//! (`LazyInitEmitter._ensure_initialized`), and the handlers it registers
//! then run for their own events. An entry whose pattern matches any event
//! the invocation emits is therefore one that can touch it. A botocore
//! builtin row touches the invocation when its pattern matches one of the
//! client's events.
use std::collections::BTreeSet;

/// The payload entry that holds the patterns.
pub const ENTRY: &[u8] = b"native/customization-patterns.json";

/// One awscli initializer: `module.function`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub module: Box<str>,
    pub function: Box<str>,
}
impl Entry {
    /// The dotted name `replaces` lists.
    pub fn name(&self) -> String {
        format!("{}.{}", self.module, self.function)
    }
}

/// A row of `MAIN_COMMAND_TABLE_OPS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableOp {
    /// A model root shown under another name.
    Rename { from: Box<str>, to: Box<str> },
    /// A hand-written command, `module.class`.
    Add {
        name: Box<str>,
        module: Box<str>,
        class: Box<str>,
    },
}

/// Where a botocore builtin handler is registered in its event's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    First,
    Default,
    Last,
}

/// A row of botocore's `BUILTIN_HANDLERS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builtin {
    pub pattern: Box<str>,
    pub handler: Box<str>,
    pub section: Section,
}

/// The whole entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patterns {
    /// `PLUGIN_REGISTRY`, pattern by pattern, in its order.
    pub registry: Vec<(Box<str>, Vec<Entry>)>,
    pub table_ops: Vec<TableOp>,
    pub builtins: Vec<Builtin>,
}

/// Why the entry could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    Json(String),
    /// The document is not the shape the generator writes; the text says
    /// where.
    Shape(&'static str),
}
impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "customization patterns: {error}"),
            Self::Shape(at) => write!(f, "customization patterns: unexpected {at}"),
        }
    }
}
impl std::error::Error for PatternError {}

type Json = serde_json::Value;

fn text(value: &Json, at: &'static str) -> Result<Box<str>, PatternError> {
    value.as_str().map(Box::from).ok_or(PatternError::Shape(at))
}
fn array<'j>(value: &'j Json, at: &'static str) -> Result<&'j [Json], PatternError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(PatternError::Shape(at))
}

impl Patterns {
    /// The entry's bytes, as the generator writes them.
    pub fn parse(bytes: &[u8]) -> Result<Self, PatternError> {
        let json: Json =
            serde_json::from_slice(bytes).map_err(|error| PatternError::Json(error.to_string()))?;
        let field = |name: &'static str| json.get(name).ok_or(PatternError::Shape(name));
        Ok(Self {
            registry: array(field("plugin_registry")?, "plugin_registry")?
                .iter()
                .map(registry_row)
                .collect::<Result<_, _>>()?,
            table_ops: array(field("main_command_table_ops")?, "main_command_table_ops")?
                .iter()
                .map(table_op)
                .collect::<Result<_, _>>()?,
            builtins: array(field("builtin_handlers")?, "builtin_handlers")?
                .iter()
                .map(builtin)
                .collect::<Result<_, _>>()?,
        })
    }

    /// The awscli entries whose patterns match one of `events`, by
    /// `module.function`.
    pub fn registry_entries(&self, events: &[String]) -> BTreeSet<String> {
        let mut touched = BTreeSet::new();
        for (pattern, entries) in &self.registry {
            if events.iter().any(|event| pattern_matches(pattern, event)) {
                touched.extend(entries.iter().map(Entry::name));
            }
        }
        touched
    }

    /// The botocore builtin handlers whose patterns match one of `events`.
    pub fn builtin_handlers(&self, events: &[String]) -> BTreeSet<String> {
        self.builtins
            .iter()
            .filter(|row| {
                events
                    .iter()
                    .any(|event| pattern_matches(&row.pattern, event))
            })
            .map(|row| row.handler.to_string())
            .collect()
    }

    /// Every awscli entry and botocore handler that touches `subject`. The
    /// clients share the session's lazy emitter, so a registry entry runs
    /// for a client's event as for the CLI's (`after-call.<service>`, say).
    pub fn touching(&self, subject: &Subject<'_>) -> BTreeSet<String> {
        let client = subject.client_events();
        let mut events = subject.cli_events();
        events.extend(client.iter().cloned());
        let mut touched = self.registry_entries(&events);
        touched.extend(self.builtin_handlers(&client));
        touched
    }
}

fn registry_row(row: &Json) -> Result<(Box<str>, Vec<Entry>), PatternError> {
    let [pattern, entries] = array(row, "registry row")? else {
        return Err(PatternError::Shape("registry row"));
    };
    let entries = array(entries, "registry entries")?
        .iter()
        .map(|entry| match array(entry, "registry entry")? {
            [module, function] => Ok(Entry {
                module: text(module, "entry module")?,
                function: text(function, "entry function")?,
            }),
            _ => Err(PatternError::Shape("registry entry")),
        })
        .collect::<Result<_, _>>()?;
    Ok((text(pattern, "registry pattern")?, entries))
}

fn table_op(row: &Json) -> Result<TableOp, PatternError> {
    let fields = array(row, "table op")?;
    match fields {
        [kind, from, to] if kind == "rename" => Ok(TableOp::Rename {
            from: text(from, "rename source")?,
            to: text(to, "rename target")?,
        }),
        [kind, name, module, class] if kind == "add" => Ok(TableOp::Add {
            name: text(name, "added name")?,
            module: text(module, "added module")?,
            class: text(class, "added class")?,
        }),
        _ => Err(PatternError::Shape("table op")),
    }
}

fn builtin(row: &Json) -> Result<Builtin, PatternError> {
    let [pattern, handler, section] = array(row, "builtin row")? else {
        return Err(PatternError::Shape("builtin row"));
    };
    let section = match section.as_str() {
        Some("first") => Section::First,
        Some("default") => Section::Default,
        Some("last") => Section::Last,
        _ => return Err(PatternError::Shape("builtin section")),
    };
    Ok(Builtin {
        pattern: text(pattern, "builtin pattern")?,
        handler: text(handler, "builtin handler")?,
        section,
    })
}

/// botocore's hierarchical match (`PrefixTrie.prefix_search`): the
/// pattern's dotted parts are a prefix of the event's, and a `*` part
/// matches any one part.
pub fn pattern_matches(pattern: &str, event: &str) -> bool {
    let mut parts = event.split('.');
    pattern
        .split('.')
        .all(|want| parts.next().is_some_and(|part| want == "*" || want == part))
}

/// What names one model operation's invocation: the service's identities,
/// all computed from its model, and the operation's names.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    /// The model's directory name, botocore's `ServiceModel.service_name`.
    pub service: &'a str,
    /// `metadata.endpointPrefix`.
    pub endpoint_prefix: &'a str,
    /// `ServiceId.hyphenize()` of `metadata.serviceId`.
    pub service_id: &'a str,
    /// The CLI's root command, after the main table's renames.
    pub command: &'a str,
    /// The operation's model name.
    pub operation: &'a str,
    /// The operation's CLI name (`xform_name(name, '-')`).
    pub cli_operation: &'a str,
    /// The argument table's names, each of which a `load-cli-arg` event
    /// names.
    pub arguments: &'a [&'a str],
}

impl Subject<'_> {
    /// The CLI's events for an invocation of the operation that runs it,
    /// in the order they are emitted. `CLIDriver.main` emits
    /// `building-top-level-params`, `building-command-table.main`,
    /// `top-level-args-parsed` and `session-initialized`;
    /// `ServiceCommand._create_command_table` emits the service's
    /// `building-command-table`; `ServiceOperation` emits its subcommand
    /// table's (`building-command-table.<lineage joined by _>`), then
    /// `before-building-argument-table-parser`, `building-argument-table`,
    /// `operation-args-parsed`, one `load-cli-arg` per argument
    /// (`_unpack_arg`, keyed by the endpoint prefix), `process-cli-arg`
    /// for a supplied argument (`CLIArgument._unpack_argument`, keyed by
    /// the model's service name) and `calling-command`. The `doc-*` events
    /// are emitted only by help, which runs under Python whatever the
    /// patterns say, so no subject emits them.
    pub fn cli_events(&self) -> Vec<String> {
        let (command, operation) = (self.command, self.cli_operation);
        let mut events = vec![
            "building-top-level-params".to_owned(),
            "building-command-table.main".to_owned(),
            "top-level-args-parsed".to_owned(),
            "session-initialized".to_owned(),
            format!("building-command-table.{command}"),
            format!("building-command-table.{command}_{operation}"),
            format!("before-building-argument-table-parser.{command}.{operation}"),
            format!("building-argument-table.{command}.{operation}"),
            format!("operation-args-parsed.{command}.{operation}"),
        ];
        for argument in self.arguments {
            events.push(format!(
                "load-cli-arg.{}.{operation}.{argument}",
                self.endpoint_prefix
            ));
        }
        events.push(format!("process-cli-arg.{}.{operation}", self.service));
        events.push(format!("calling-command.{command}.{operation}"));
        events
    }

    /// botocore's events for creating the operation's client and making the
    /// call (`ClientCreator.create_client`, `BaseClient._make_api_call`,
    /// `Endpoint._send_request`, `RequestSigner.sign`,
    /// `EndpointRulesetResolver.construct_endpoint`). `getattr.*` is not
    /// emitted: the CLI calls the operation's method, which the client
    /// class defines (`CLIOperationCaller._make_client_call`).
    pub fn client_events(&self) -> Vec<String> {
        let (id, operation) = (self.service_id, self.operation);
        let mut events = vec![
            "before-create-client".to_owned(),
            "choose-service-name".to_owned(),
            format!("creating-client-class.{id}"),
            format!("before-endpoint-resolution.{id}"),
            "after-create-client".to_owned(),
        ];
        for stage in [
            "provide-client-params",
            "before-parameter-build",
            "before-call",
            "request-created",
            "choose-signer",
            "before-sign",
            "before-send",
            "response-received",
            "needs-retry",
            "before-parse",
            "after-call",
            "after-call-error",
        ] {
            events.push(format!("{stage}.{id}.{operation}"));
        }
        events
    }
}

/// A native piece of the CLI side and the Python registry entries it
/// stands in for (`module.function`), so admission can tell an entry that
/// touches an invocation from one whose effect native reproduces. The
/// client side's counterpart is the `client` crate's `Customization`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliCustomization {
    pub id: &'static str,
    pub replaces: &'static [&'static str],
}

#[cfg(test)]
mod tests;
