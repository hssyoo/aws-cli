//! The commands the service models supply, derived for the invocation from
//! botocore's search path, with no list of anything in source: the root
//! table is the model directories with the main table's renames and
//! additions applied (`CLIDriver._build_builtin_commands`,
//! `LazyInitEmitter._apply_main_command_table_ops`), a service's table is
//! its operations through `xform_name` (`ServiceCommand._create_command_table`),
//! and an operation's argument table is its input members and the
//! cross-cutting arguments (`table`). A service's table then takes the
//! edits the pack recorded its table editors making
//! (`Patterns::table_edits`), and `wait`.
use crate::patterns::{CliCustomization, Match, Patterns, Subject, TableEdit, TableOp};
use crate::registry::OperationSource;
use crate::spec::{Admission, Invocation, Needs};
use awscli_argparse::tables::RequiredOverride;
use awscli_argparse::{Argument, CliType, DefaultValue, ParseResult};
use cli_core::cli_error::PythonReason;
use model::search::SearchSource;
use model::shapes::Model;
use model::source::{Kind, LoadError, Source, load_service_model};
use model::xform::{SpecialCases, xform_name};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;

pub mod dump;
mod table;
#[doc(inline)]
pub use table::{DerivedArgument, Origin, Paging, Table, Unpack, ValueType, kind_name};

type Json = serde_json::Value;

/// The registry entries whose table edits the derived tables reproduce:
/// `wait` for a service with waiters, the router's alias rule, the
/// cross-cutting arguments, and the table editors' removals and `wizard`
/// commands. They stand in for the table edits only, not for what the
/// handlers do when an argument is supplied, so admission does not read
/// them.
pub const TABLE_EDITS: &[CliCustomization] = &[
    CliCustomization {
        id: "tables.removals",
        replaces: &["awscli.customizations.removals.register_removals"],
    },
    CliCustomization {
        id: "tables.wizard",
        replaces: &["awscli.customizations.wizard.commands.register_wizard_commands"],
    },
    CliCustomization {
        id: "tables.wait",
        replaces: &["awscli.customizations.waiters.register_add_waiters"],
    },
    CliCustomization {
        id: "tables.alias",
        replaces: &["awscli.alias.register_alias_commands"],
    },
    CliCustomization {
        id: "tables.outfile",
        replaces: &["awscli.customizations.streamingoutputarg.register_streaming_output_arg"],
    },
    CliCustomization {
        id: "tables.cli-input",
        replaces: &["awscli.customizations.cliinput.register_cli_input_args"],
    },
    CliCustomization {
        id: "tables.paging",
        replaces: &["awscli.customizations.paginate.register_pagination"],
    },
    CliCustomization {
        id: "tables.skeleton",
        replaces: &["awscli.customizations.generatecliskeleton.register_generate_cli_skeleton"],
    },
];

/// A loaded service: its model, its command table and its operations'
/// tables, derived once each.
#[derive(Debug)]
pub struct Service {
    pub command: String,
    pub name: String,
    pub model: Model,
    /// The CLI names of the model operations the command table holds, in
    /// model order: those its table editors neither delete nor set.
    pub operations: Vec<String>,
    /// The command table's names: the model operations' names after the
    /// table editors' edits, then `wait` when the service has waiters
    /// (`add_waiters`). A name an editor sets is no model operation.
    pub choices: Vec<String>,
    /// A `building-command-table` handler the derivation does not reproduce
    /// edits the table.
    pub customized: bool,
    /// The model is the payload's alone (`SearchSource::packed`), the one
    /// whose client the pack observed.
    pub packed: bool,
    pages: Option<Json>,
    /// Each of `operations`' index in the model's operations.
    indices: Vec<usize>,
    tables: Vec<OnceCell<Table>>,
}

impl Service {
    /// The model operation the CLI name `operation` stands for, with its
    /// place in `operations`.
    fn position(&self, operation: &str) -> Option<(usize, &model::shapes::Operation)> {
        let index = self.operations.iter().position(|name| name == operation)?;
        let model_index = *self.indices.get(index)?;
        Some((index, self.model.operations().get(model_index)?))
    }
    /// The model operation the CLI name `operation` stands for.
    pub fn operation(&self, operation: &str) -> Option<&model::shapes::Operation> {
        self.position(operation).map(|(_, found)| found)
    }
    /// Whether the paginators file configures the model operation
    /// `operation` (`get_paginator_config`, which `client.can_paginate`
    /// also reads).
    pub fn paginated(&self, operation: &str) -> bool {
        self.paginator(operation).is_some()
    }
    /// The paginators file's entry for the model operation `operation`
    /// (`PaginatorModel.get_paginator`).
    pub fn paginator(&self, operation: &str) -> Option<&Json> {
        self.pages.as_ref()?.get(operation)
    }
}

/// An append-only list whose values never move, so a borrow of one lives
/// as long as the list.
#[derive(Debug)]
struct Cache<T> {
    value: OnceCell<T>,
    next: OnceCell<Box<Cache<T>>>,
}
impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            value: OnceCell::new(),
            next: OnceCell::new(),
        }
    }
}
impl<T> Cache<T> {
    fn find_or_insert(&self, wanted: impl Fn(&T) -> bool, make: impl FnOnce() -> T) -> &T {
        let mut node = self;
        loop {
            match node.value.get() {
                Some(value) if wanted(value) => return value,
                Some(_) => node = node.next.get_or_init(Box::default),
                None => return node.value.get_or_init(make),
            }
        }
    }
}

/// The commands of the models botocore would load for this invocation.
#[derive(Debug)]
pub struct ModelSource<'a> {
    models: SearchSource<'a>,
    patterns: Patterns,
    special: SpecialCases,
    roots: OnceCell<Result<BTreeMap<String, String>, LoadError>>,
    services: Cache<(String, Option<Service>)>,
}

/// Why a payload's models cannot be read natively.
#[derive(Debug)]
pub enum SourceError {
    /// The payload has no pattern entry, as a payload packed without the
    /// generator has none, or the entry cannot be read.
    Patterns(String),
    /// The search path cannot be built as Python builds it.
    Search(model::search::SearchError),
    /// botocore's `xform_name` cases cannot be read.
    Xform(model::xform::XformError),
}
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Patterns(error) => f.write_str(error),
            Self::Search(error) => error.fmt(f),
            Self::Xform(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SourceError {}

impl<'a> ModelSource<'a> {
    /// The source of the payload `view`'s models and patterns, over the
    /// search path the CLI builds from `aws_data_path`; `expand` is
    /// `os.path.expanduser(os.path.expandvars(path))` (`SearchSource::for_cli`).
    pub fn from_payload(
        view: pyload::View<'a>,
        aws_data_path: Option<&std::ffi::OsStr>,
        expand: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<Self, SourceError> {
        let entry = model::payload::load_resource(view, crate::patterns::ENTRY)
            .map_err(|error| SourceError::Patterns(error.to_string()))?;
        let patterns =
            Patterns::parse(&entry).map_err(|error| SourceError::Patterns(error.to_string()))?;
        let models =
            SearchSource::for_cli(view, aws_data_path, expand).map_err(SourceError::Search)?;
        let special = SpecialCases::botocore().map_err(SourceError::Xform)?;
        Ok(Self::new(models, patterns, special))
    }

    pub fn new(models: SearchSource<'a>, patterns: Patterns, special: SpecialCases) -> Self {
        Self {
            models,
            patterns,
            special,
            roots: OnceCell::new(),
            services: Cache::default(),
        }
    }

    /// The search path the models come from.
    pub const fn search(&self) -> &SearchSource<'a> {
        &self.models
    }

    pub const fn patterns(&self) -> &Patterns {
        &self.patterns
    }

    /// The model roots by CLI name, each naming its model directory: the
    /// services the search lists, renamed by the main table's renames, less
    /// the names it adds as hand-written commands. Nothing is loaded.
    pub fn roots(&self) -> Result<&BTreeMap<String, String>, &LoadError> {
        self.roots
            .get_or_init(|| {
                let services = self.models.services(Kind::Service)?;
                let mut roots: BTreeMap<String, String> = services
                    .into_iter()
                    .map(|name| (name.clone(), name))
                    .collect();
                for op in &self.patterns.table_ops {
                    match op {
                        TableOp::Rename { from, to } => {
                            if let Some(service) = roots.remove(&**from) {
                                roots.insert(to.to_string(), service);
                            }
                        }
                        TableOp::Add { name, .. } => {
                            roots.remove(&**name);
                        }
                    }
                }
                Ok(roots)
            })
            .as_ref()
    }

    /// The loaded service of the root `command`, once.
    pub fn service(&self, command: &str) -> Option<&Service> {
        let service = self.roots().ok()?.get(command)?;
        let (_, loaded) = self.services.find_or_insert(
            |(name, _)| name == command,
            || (command.to_owned(), self.load(command, service)),
        );
        loaded.as_ref()
    }

    fn load(&self, command: &str, service: &str) -> Option<Service> {
        let model = Model::load(&self.models, service).ok()?;
        let packed = self.models.packed(service).ok()?;
        let mut table: Vec<(String, Option<usize>)> = model
            .operations()
            .iter()
            .enumerate()
            .map(|(index, operation)| {
                (xform_name(&operation.name, "-", &self.special), Some(index))
            })
            .collect();
        for edit in self.patterns.table_edits(command) {
            edit_table(&mut table, edit);
        }
        if self.has_waiters(service) {
            edit_table(&mut table, &TableEdit::Set("wait".into()));
        }
        let choices = table.iter().map(|(name, _)| name.clone()).collect();
        let (operations, indices): (Vec<String>, Vec<usize>) = table
            .into_iter()
            .filter_map(|(name, index)| Some((name, index?)))
            .unzip();
        let pages = load_service_model(&self.models, service, Kind::Paginators)
            .ok()
            .and_then(|document| document.json.get("pagination").cloned());
        let tables = operations.iter().map(|_| OnceCell::new()).collect();
        Some(Service {
            customized: self.edits(&[format!("building-command-table.{command}")]),
            packed,
            command: command.to_owned(),
            name: service.to_owned(),
            model,
            operations,
            choices,
            pages,
            indices,
            tables,
        })
    }

    /// `get_waiter_model_from_service_model` and `waiter_names`: a waiter
    /// model the search finds, holding a waiter.
    fn has_waiters(&self, service: &str) -> bool {
        load_service_model(&self.models, service, Kind::Waiters).is_ok_and(|document| {
            document
                .json
                .get("waiters")
                .and_then(Json::as_object)
                .is_some_and(|waiters| !waiters.is_empty())
        })
    }

    /// Whether a registry entry outside [`TABLE_EDITS`] matches one of
    /// `events`.
    fn edits(&self, events: &[String]) -> bool {
        let reproduced = TABLE_EDITS
            .iter()
            .flat_map(|edit| edit.replaces.iter().copied());
        let reproduced: Vec<&str> = reproduced.collect();
        self.patterns
            .registry_entries(events)
            .iter()
            .any(|entry| !reproduced.contains(&entry.as_str()))
    }

    /// The table of `[command, operation]`, derived once.
    pub fn table(&self, command: &str, operation: &str) -> Option<&Table> {
        let service = self.service(command)?;
        let (index, model_operation) = service.position(operation)?;
        Some(service.tables.get(index)?.get_or_init(|| {
            let pages = service
                .pages
                .as_ref()
                .and_then(|pages| pages.get(&*model_operation.name));
            let mut table = table::derive(&service.model, model_operation, pages, &self.special);
            let events = [
                format!("building-command-table.{command}_{operation}"),
                format!("before-building-argument-table-parser.{command}.{operation}"),
                format!("building-argument-table.{command}.{operation}"),
            ];
            table.customized |= self.edits(&events);
            table
        }))
    }

    /// The awscli entries and botocore handlers that touch `[command,
    /// operation]`, by dotted name.
    pub fn touching(&self, command: &str, operation: &str) -> Option<Vec<String>> {
        self.with_subject(command, operation, |subject| {
            self.patterns.touching(subject).into_iter().collect()
        })
    }

    /// Every pattern that matches `[command, operation]`'s events, with
    /// what it names ([`Patterns::matches`]).
    pub fn matches(&self, command: &str, operation: &str) -> Option<Vec<Match>> {
        self.with_subject(command, operation, |subject| self.patterns.matches(subject))
    }

    /// The rows `[command, operation]`'s client registers as it is created
    /// that its events meet ([`Patterns::client_handlers`]), by handler.
    pub fn client_handlers(&self, command: &str, operation: &str) -> Option<Vec<String>> {
        self.with_subject(command, operation, |subject| {
            let events = subject.client_events();
            self.patterns
                .client_handlers(subject, &events)
                .into_iter()
                .collect()
        })
    }

    /// `judge` of the subject the pattern matcher reads for `[command,
    /// operation]`.
    fn with_subject<T>(
        &self,
        command: &str,
        operation: &str,
        judge: impl FnOnce(&Subject<'_>) -> T,
    ) -> Option<T> {
        let service = self.service(command)?;
        let table = self.table(command, operation)?;
        let model_operation = service.operation(operation)?;
        let metadata = service.model.metadata();
        let service_id = metadata.hyphenized_service_id().unwrap_or_default();
        let arguments: Vec<&str> = table
            .arguments
            .iter()
            .map(|argument| &*argument.name)
            .collect();
        let subject = Subject {
            service: &service.name,
            endpoint_prefix: metadata.endpoint_prefix.as_deref().unwrap_or_default(),
            service_id: &service_id,
            command,
            operation: &model_operation.name,
            cli_operation: operation,
            arguments: &arguments,
            packed: service.packed,
        };
        Some(judge(&subject))
    }
}

/// One edit of a command table: a set name keeps its place, or is added
/// last, and is no model operation from then on; a deleted name leaves
/// (`OrderedDict.__setitem__`, `__delitem__`). Deleting a name the table
/// lacks only logs (`_remove_commands`).
fn edit_table(table: &mut Vec<(String, Option<usize>)>, edit: &TableEdit) {
    match edit {
        TableEdit::Set(name) => match table.iter_mut().find(|(held, _)| **held == **name) {
            Some((_, index)) => *index = None,
            None => table.push((name.to_string(), None)),
        },
        TableEdit::Delete(name) => table.retain(|(held, _)| **held != **name),
    }
}

/// The argparse form of a derived argument.
fn argparse(argument: &DerivedArgument) -> Argument<'_> {
    Argument {
        name: &argument.name,
        dest: argument.dest.as_deref(),
        positional: argument.positional,
        action: argument.action,
        nargs: argument.nargs,
        constant: argument.constant.map(DefaultValue::String),
        default: if argument.default_null {
            DefaultValue::Null
        } else {
            DefaultValue::None
        },
        choices: argument.choices,
        required: argument.required,
        hidden: argument.hidden,
        value_type: argument.value_type,
        cli_type: match argument.unpack {
            Unpack::AsIs => None,
            Unpack::Integer => Some(CliType::UnboundedInteger),
            Unpack::Deferred => Some(CliType::Deferred),
        },
    }
}

impl OperationSource for ModelSource<'_> {
    fn is_root(&self, name: &str) -> bool {
        self.roots().is_ok_and(|roots| roots.contains_key(name))
    }
    fn operations(&self, root: &str) -> Option<Cow<'_, [&str]>> {
        let service = self.service(root)?;
        Some(Cow::Owned(
            service.choices.iter().map(String::as_str).collect(),
        ))
    }
    fn arguments(&self, root: &str, operation: &str) -> Option<Cow<'_, [Argument<'_>]>> {
        let table = self.table(root, operation)?;
        Some(Cow::Owned(table.arguments.iter().map(argparse).collect()))
    }
    fn overrides(&self, root: &str, operation: &str) -> Cow<'_, [RequiredOverride<'_>]> {
        let lifts = self
            .table(root, operation)
            .map_or(&[][..], |table| &table.overrides);
        let lifts = lifts.iter().map(|&(option, unless_next)| RequiredOverride {
            option,
            unless_next,
        });
        Cow::Owned(lifts.collect())
    }
    fn reproduces_failures(&self, root: &str, operation: Option<&str>) -> bool {
        let Some(service) = self.service(root) else {
            return false;
        };
        let Some(operation) = operation else {
            return !service.customized;
        };
        let table = self.table(root, operation);
        !service.customized && table.is_some_and(|table| !table.customized)
    }
    fn needs(&self, root: &str) -> Needs {
        let service = self.service(root);
        let metadata = service.map(|service| service.model.metadata());
        let id = metadata.and_then(model::shapes::Metadata::hyphenized_service_id);
        Needs::Client {
            service: id.unwrap_or_else(|| root.to_owned()).into(),
        }
    }
    /// No model operation binds natively yet: every one meets the
    /// cross-cutting entries (the session's and the argument table's), and
    /// no native piece replaces what they do when the command runs.
    fn bind(&self, _parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        Err(Admission::Python(PythonReason::UnportedCustomization))
    }
}

#[cfg(test)]
mod tests;
