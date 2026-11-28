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
//! client's events, and so does a row the client registers as it is
//! created (`ClientCreator.create_client`), which the pack observed by
//! creating every service's client ([`ClientHandlers`]).
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

/// The segment a generic client row's pattern holds where the client's
/// own hyphenized service id stands.
pub const SERVICE_ID: &str = "{service_id}";

/// The handlers a client registers on its own emitter as it is created,
/// beyond the session's builtins: what the pack saw creating each listed
/// service's client as the CLI does by default. botocore registers them in
/// code (`ClientCreator._register_retries`, `_register_s3_events`,
/// `_register_s3express_events`, `_register_s3_control_events`,
/// `_register_importexport_events`, `_register_endpoint_discovery`,
/// `BaseClient._register_handlers`), so only creating the clients shows
/// them. A row's section is its place in the event's list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientHandlers {
    /// The rows every service's client registers, with [`SERVICE_ID`] for
    /// the client's own service segment.
    pub generic: Vec<Builtin>,
    /// The other rows, by the service's loader name, with [`SERVICE_ID`]
    /// for the service's own segment.
    pub services: Vec<(Box<str>, Vec<Builtin>)>,
}

/// What a command-table handler does to its table: one name it sets
/// (`command_table[name] = ...`) or deletes (`del command_table[name]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEdit {
    Set(Box<str>),
    Delete(Box<str>),
}

/// One handler a table editor registers: the root command table event it
/// is registered on, and what it does to that table, in order. The pack
/// also names the handler, which nothing here reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableHandler {
    pub event: Box<str>,
    pub edits: Vec<TableEdit>,
}

/// A registry initializer whose handlers on root command tables do nothing
/// but edit them, with the handlers the pack saw it register there: by
/// event, and each event's in the order it registered them, which is the
/// order they run in (the order across events decides nothing, and is not
/// kept). The pack refuses one that reads its table, and one the registry
/// lists under any pattern but a root's table event
/// (`building-command-table.<command>`), or that registers anywhere else,
/// unless the pack names it as also registering handlers on the other
/// patterns it is listed under; those are not recorded, and nothing here
/// reproduces them (`ModelSource`'s customized tables).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEditor {
    pub entry: Entry,
    pub handlers: Vec<TableHandler>,
}

/// The whole entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patterns {
    /// `PLUGIN_REGISTRY`, pattern by pattern, in its order.
    pub registry: Vec<(Box<str>, Vec<Entry>)>,
    pub table_ops: Vec<TableOp>,
    pub builtins: Vec<Builtin>,
    pub client: ClientHandlers,
    pub table_editors: Vec<TableEditor>,
}

/// Where a match comes from, which says what can cover it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// An awscli registry initializer (`PLUGIN_REGISTRY`).
    Registry,
    /// A botocore session builtin (`BUILTIN_HANDLERS`).
    Builtin,
    /// A row a client registers as it is created ([`ClientHandlers`]).
    Client,
}

/// One pattern of the entry that matches an invocation's events, with the
/// initializer or handler it names. Client rows' patterns are spelled for
/// the subject's own service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub kind: MatchKind,
    pub pattern: String,
    pub name: String,
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
            builtins: rows(field("builtin_handlers")?, "builtin_handlers")?,
            client: client_handlers(field("client_handlers")?)?,
            table_editors: array(field("command_table_edits")?, "command_table_edits")?
                .iter()
                .map(table_editor)
                .collect::<Result<_, _>>()?,
        })
    }

    /// The edits the table editors make to the table of the root
    /// `command`, in the order Python makes them. Its event,
    /// `building-command-table.<command>`, initializes the editors listed
    /// under it, in the registry's order
    /// (`LazyInitEmitter._ensure_initialized`); no earlier event of the
    /// invocation reaches one, as the pack lists them under root table
    /// events alone. Their handlers on that event then run in the order
    /// they were registered, all at the event's own node
    /// (`HierarchicalEmitter._emit`), so before any registered on the bare
    /// `building-command-table`, as `add_waiters` is. An editor the
    /// registry does not list under the event is never initialized for it,
    /// so its handlers there never run.
    pub fn table_edits(&self, command: &str) -> Vec<&TableEdit> {
        let event = format!("building-command-table.{command}");
        let listed = self
            .registry
            .iter()
            .filter(|(pattern, _)| **pattern == *event)
            .flat_map(|(_, entries)| entries);
        let mut edits = Vec::new();
        for entry in listed {
            let editor = self
                .table_editors
                .iter()
                .find(|editor| editor.entry == *entry);
            let handlers = editor.into_iter().flat_map(|editor| &editor.handlers);
            for handler in handlers.filter(|handler| *handler.event == *event) {
                edits.extend(&handler.edits);
            }
        }
        edits
    }

    /// The registry initializers `events`, emitted in turn, initialize, in
    /// the order they are initialized: for each event, the entries of the
    /// patterns that match it in `prefix_search`'s order, each listed
    /// entry once (`LazyInitEmitter._ensure_initialized`).
    pub fn initialized(&self, events: &[String]) -> Vec<String> {
        let mut order: Vec<String> = Vec::new();
        for event in events {
            let mut matching: Vec<&(Box<str>, Vec<Entry>)> = self
                .registry
                .iter()
                .filter(|(pattern, _)| pattern_matches(pattern, event))
                .collect();
            matching.sort_by(|(a, _), (b, _)| emission_order(a, b));
            for entry in matching.into_iter().flat_map(|(_, entries)| entries) {
                let name = entry.name();
                if !order.contains(&name) {
                    order.push(name);
                }
            }
        }
        order
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

    /// The rows `subject`'s client registers as it is created whose
    /// patterns match one of `events`: the generic rows, and the service's
    /// own when the subject's model is the one the pack observed.
    ///
    /// A model a user supplies (`AWS_DATA_PATH`, `~/.aws/models`; not
    /// [`Subject::packed`]) meets the generic rows alone. They are the ones
    /// `ClientCreator.create_client` registers for any model: the signer
    /// and user-agent rebuild of `BaseClient._register_handlers`, and the
    /// standard retry handlers, which depend on configuration only. The
    /// others are registered on the model's loader name (`s3`, `s3control`,
    /// `importexport`) or on its having an endpoint-discovery operation
    /// (`_register_endpoint_discovery`), and the pack saw the packed
    /// model's, not the user's; so the generic rows are exactly a user
    /// model's only where its name is none of those and it has no
    /// `endpointoperation`. Admission must route any other user model to
    /// Python rather than read this as its full set.
    pub fn client_handlers(&self, subject: &Subject<'_>, events: &[String]) -> BTreeSet<String> {
        self.client_rows(subject)
            .filter(|row| {
                let pattern = scoped(&row.pattern, subject.service_id);
                events.iter().any(|event| pattern_matches(&pattern, event))
            })
            .map(|row| row.handler.to_string())
            .collect()
    }

    /// Every pattern that matches one of `subject`'s events, with what it
    /// names: the registry's over the CLI's and the client's events (the
    /// clients share the session's lazy emitter, so a registry entry runs
    /// for a client's event as for the CLI's, `after-call.<service>` say),
    /// then the builtins and the client rows over the client's, each in the
    /// entry's order.
    pub fn matches(&self, subject: &Subject<'_>) -> Vec<Match> {
        let client = subject.client_events();
        let mut events = subject.cli_events();
        events.extend(client.iter().cloned());
        let meets = |events: &[String], pattern: &str| {
            events.iter().any(|event| pattern_matches(pattern, event))
        };
        let mut found = Vec::new();
        for (pattern, entries) in &self.registry {
            if meets(&events, pattern) {
                found.extend(entries.iter().map(|entry| Match {
                    kind: MatchKind::Registry,
                    pattern: pattern.to_string(),
                    name: entry.name(),
                }));
            }
        }
        let builtins = self
            .builtins
            .iter()
            .map(|row| (MatchKind::Builtin, row, row.pattern.to_string()));
        let client_rows = self.client_rows(subject).map(|row| {
            (
                MatchKind::Client,
                row,
                scoped(&row.pattern, subject.service_id),
            )
        });
        for (kind, row, pattern) in builtins.chain(client_rows) {
            if meets(&client, &pattern) {
                found.push(Match {
                    kind,
                    pattern,
                    name: row.handler.to_string(),
                });
            }
        }
        found
    }

    /// The client rows that apply to `subject`'s client: the generic ones,
    /// and the service's own for its packed model.
    fn client_rows<'p>(&'p self, subject: &Subject<'_>) -> impl Iterator<Item = &'p Builtin> {
        let packed = subject.packed;
        let service = subject.service.to_owned();
        let own = self
            .client
            .services
            .iter()
            .filter(move |(name, _)| packed && **name == *service)
            .flat_map(|(_, rows)| rows);
        self.client.generic.iter().chain(own)
    }
}

/// A client row's pattern for the client of `service_id`.
fn scoped(pattern: &str, service_id: &str) -> String {
    pattern
        .split('.')
        .map(|chunk| {
            if chunk == SERVICE_ID {
                service_id
            } else {
                chunk
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn rows(value: &Json, at: &'static str) -> Result<Vec<Builtin>, PatternError> {
    array(value, at)?.iter().map(builtin).collect()
}

fn client_handlers(value: &Json) -> Result<ClientHandlers, PatternError> {
    let field = |name: &'static str| value.get(name).ok_or(PatternError::Shape(name));
    let services = array(field("services")?, "client services")?
        .iter()
        .map(|row| match array(row, "client service")? {
            [service, service_rows] => Ok((
                text(service, "client service name")?,
                rows(service_rows, "client service rows")?,
            )),
            _ => Err(PatternError::Shape("client service")),
        })
        .collect::<Result<_, _>>()?;
    Ok(ClientHandlers {
        generic: rows(field("generic")?, "client generic rows")?,
        services,
    })
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

fn table_editor(row: &Json) -> Result<TableEditor, PatternError> {
    let [entry, handlers] = array(row, "table editor")? else {
        return Err(PatternError::Shape("table editor"));
    };
    let [module, function] = array(entry, "table editor entry")? else {
        return Err(PatternError::Shape("table editor entry"));
    };
    let handlers = array(handlers, "table editor handlers")?
        .iter()
        .map(|handler| match array(handler, "table handler")? {
            [event, _name, edits] => Ok(TableHandler {
                event: text(event, "table handler event")?,
                edits: array(edits, "table edits")?
                    .iter()
                    .map(table_edit)
                    .collect::<Result<_, _>>()?,
            }),
            _ => Err(PatternError::Shape("table handler")),
        })
        .collect::<Result<_, _>>()?;
    Ok(TableEditor {
        entry: Entry {
            module: text(module, "table editor module")?,
            function: text(function, "table editor function")?,
        },
        handlers,
    })
}

fn table_edit(edit: &Json) -> Result<TableEdit, PatternError> {
    match array(edit, "table edit")? {
        [kind, name] if kind == "set" => Ok(TableEdit::Set(text(name, "set name")?)),
        [kind, name] if kind == "del" => Ok(TableEdit::Delete(text(name, "deleted name")?)),
        _ => Err(PatternError::Shape("table edit")),
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

const TABLE_EVENT: &str = "building-command-table.";

/// Whether `pattern` is one root command's table event: one command, not
/// the main table's (whose initializers the main table's ops replace) and
/// no subcommand's, whose lineage is `_`-joined
/// (`ServiceOperation._build_subcommand_table`, clidriver.py:891).
pub fn root_table_event(pattern: &str) -> bool {
    pattern.strip_prefix(TABLE_EVENT).is_some_and(|command| {
        !command.is_empty() && command != "main" && command != "*" && !command.contains(['.', '_'])
    })
}

/// The order `PrefixTrie.prefix_search` gives two patterns that both match
/// one event: its stack walk visits a node's literal child, all of its
/// subtree, before its `*` child, and a node before its children, and it
/// collects each node's values ahead of those visited before it
/// (`extendleft`). So the pattern visited later comes first: the deeper of
/// a pattern and its prefix, and at the first part they differ in, the
/// `*`.
pub fn emission_order(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut left, mut right) = (a.split('.'), b.split('.'));
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            // The shorter is the ancestor, visited first, collected last.
            (None, Some(_)) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Less,
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(_)) => {
                return if x == "*" {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
        }
    }
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
    /// Whether the operation's model is the payload's alone
    /// (`SearchSource::packed`), so what the pack observed of its client
    /// holds for it.
    pub packed: bool,
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

#[cfg(test)]
mod tests;
