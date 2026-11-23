//! The commands the service models supply, derived for the invocation from
//! botocore's search path, with no list of anything in source: the root
//! table is the model directories with the main table's renames and
//! additions applied (`CLIDriver._build_builtin_commands`,
//! `LazyInitEmitter._apply_main_command_table_ops`), a service's table is
//! its operations through `xform_name` (`ServiceCommand._create_command_table`),
//! and an operation's argument table is its input members and the
//! cross-cutting arguments (`table`).
use crate::patterns::{Match, Patterns, Subject, TableOp};
use crate::registry::OperationSource;
use crate::spec::Needs;
use awscli_argparse::tables::RequiredOverride;
use awscli_argparse::{Argument, CliType, DefaultValue};
use model::search::SearchSource;
use model::shapes::Model;
use model::source::{Kind, LoadError, Source, load_service_model};
use model::xform::{SpecialCases, xform_name};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;

mod table;
#[doc(inline)]
pub use table::{DerivedArgument, Origin, Table, Unpack, ValueType, kind_name};

type Json = serde_json::Value;

/// A loaded service: its model, its command table and its operations'
/// tables, derived once each.
#[derive(Debug)]
pub struct Service {
    pub command: String,
    pub name: String,
    pub model: Model,
    /// The operations' CLI names, in model order.
    pub operations: Vec<String>,
    /// The command table's names: the operations, then `wait` when the
    /// service has waiters (`add_waiters`).
    pub choices: Vec<String>,
    /// A `building-command-table` handler the derivation does not reproduce
    /// edits the table.
    pub customized: bool,
    /// The model is the payload's alone (`SearchSource::packed`), the one
    /// whose client the pack observed.
    pub packed: bool,
    pages: Option<Json>,
    tables: Vec<OnceCell<Table>>,
}

impl Service {
    /// The model operation the CLI name `operation` stands for.
    pub fn operation(&self, operation: &str) -> Option<&model::shapes::Operation> {
        let index = self.operations.iter().position(|name| name == operation)?;
        self.model.operations().get(index)
    }
    /// Whether the paginators file configures the model operation
    /// `operation` (`get_paginator_config`, which `client.can_paginate`
    /// also reads).
    pub fn paginated(&self, operation: &str) -> bool {
        self.pages
            .as_ref()
            .is_some_and(|pages| pages.get(operation).is_some())
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
    /// The registry initializers whose edits to a table the derivation
    /// reproduces; any other that a table's events meet customizes it.
    reproduced: Vec<&'static str>,
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
    /// `reproduced` names the registry initializers whose table edits the
    /// derivation reproduces (the CLI's coverage table says which).
    pub fn from_payload(
        view: pyload::View<'a>,
        aws_data_path: Option<&std::ffi::OsStr>,
        expand: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
        reproduced: &[&'static str],
    ) -> Result<Self, SourceError> {
        let entry = model::payload::load_resource(view, crate::patterns::ENTRY)
            .map_err(|error| SourceError::Patterns(error.to_string()))?;
        let patterns =
            Patterns::parse(&entry).map_err(|error| SourceError::Patterns(error.to_string()))?;
        let models =
            SearchSource::for_cli(view, aws_data_path, expand).map_err(SourceError::Search)?;
        let special = SpecialCases::botocore().map_err(SourceError::Xform)?;
        Ok(Self::new(models, patterns, special, reproduced))
    }

    pub fn new(
        models: SearchSource<'a>,
        patterns: Patterns,
        special: SpecialCases,
        reproduced: &[&'static str],
    ) -> Self {
        Self {
            models,
            patterns,
            special,
            reproduced: reproduced.to_vec(),
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
        let operations: Vec<String> = model
            .operations()
            .iter()
            .map(|operation| xform_name(&operation.name, "-", &self.special))
            .collect();
        let mut choices = operations.clone();
        if self.has_waiters(service) {
            choices.push("wait".to_owned());
        }
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

    /// Whether a registry entry the derivation does not reproduce matches
    /// one of `events`.
    fn edits(&self, events: &[String]) -> bool {
        self.patterns
            .registry_entries(events)
            .iter()
            .any(|entry| !self.reproduced.contains(&entry.as_str()))
    }

    /// The table of `[command, operation]`, derived once.
    pub fn table(&self, command: &str, operation: &str) -> Option<&Table> {
        let service = self.service(command)?;
        let index = service
            .operations
            .iter()
            .position(|name| name == operation)?;
        let model_operation = &service.model.operations()[index];
        Some(service.tables[index].get_or_init(|| {
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
        let index = service
            .operations
            .iter()
            .position(|name| name == operation)?;
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
            operation: &service.model.operations()[index].name,
            cli_operation: operation,
            arguments: &arguments,
            packed: service.packed,
        };
        Some(judge(&subject))
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
    fn paginated(&self, root: &str, operation: &str) -> bool {
        self.service(root).is_some_and(|service| {
            service
                .operation(operation)
                .is_some_and(|found| service.paginated(&found.name))
        })
    }
    fn needs(&self, root: &str) -> Needs {
        let service = self.service(root);
        let metadata = service.map(|service| service.model.metadata());
        let id = metadata.and_then(model::shapes::Metadata::hyphenized_service_id);
        let builtins = service
            .and_then(|service| {
                load_service_model(&self.models, &service.name, Kind::EndpointRuleSet).ok()
            })
            .map(|document| document.json);
        let reads_s3_settings = builtins
            .as_ref()
            .and_then(|rules| rules.get("parameters")?.as_object())
            .is_some_and(|parameters| {
                parameters.values().any(|parameter| {
                    parameter
                        .get("builtIn")
                        .and_then(Json::as_str)
                        .is_some_and(|builtin| builtin.starts_with("AWS::S3"))
                })
            });
        Needs {
            service: id.unwrap_or_else(|| root.to_owned()).into(),
            reads_s3_settings,
        }
    }
}

#[cfg(test)]
mod tests;
