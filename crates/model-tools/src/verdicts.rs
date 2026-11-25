//! Each shipped operation's verdict, the rows of the golden
//! `tests/verdicts/verdicts.tsv`: the client's static refusal by its
//! reason and what it names, the operation's eligibility [`Standing`] and
//! its paginator's verdict, each rendered by a stable name
//! (`PythonReason::name()`, model keys, `Display` texts), never `Debug`.
//! The signatures here are fixed; their bodies follow the product API.
use crate::eligibility::{PythonReasonName, Standing, standing};
use cli_core::cli_error::PythonReason;
use client::admission::{Unsupported, static_refusal};
use client::endpoint::EndpointData;
use command::models::ModelSource;
use service_command::admit;

/// The columns of a row, the TSV's first line.
pub const HEADER: &str = "command\toperation\trefusal\trefused\tstanding\tpaginator";

/// One operation's verdict. `-` stands for none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The root command, as the CLI names it.
    pub command: String,
    /// The operation, as the CLI names it.
    pub operation: String,
    /// The reason of the client's static refusal (`PythonReason::name()`).
    pub refusal: String,
    /// What that refusal names: a protocol, a model key, an auth type, a
    /// built-in, a parameter, or the paginator's refusal text.
    pub refused: String,
    /// The eligibility standing: `no_arguments`, `scalar_arguments`,
    /// `required_not_scalar` or `refused:<reason>`.
    pub standing: String,
    /// `native` (the native paginator runs it), `unpaged` (an entry whose
    /// table has no paging options), `refused:<why>`, or `-` (no entry).
    pub paginator: String,
}

impl Row {
    /// The row as a TSV line, without its newline.
    pub fn line(&self) -> String {
        [
            &self.command,
            &self.operation,
            &self.refusal,
            &self.refused,
            &self.standing,
            &self.paginator,
        ]
        .map(|field| cell(field))
        .join("\t")
    }
}

/// Every root's operations' rows, roots by name ([`roots`]) and each
/// root's operations in its command table's order ([`root_rows`]): the
/// operations the eligibility census counts, with the CLI's customization
/// table.
pub fn rows(source: &ModelSource<'_>) -> Vec<Row> {
    roots(source)
        .iter()
        .flat_map(|root| root_rows(source, root))
        .collect()
}

/// The roots, by name.
pub fn roots(source: &ModelSource<'_>) -> Vec<String> {
    source
        .roots()
        .map(|roots| roots.keys().cloned().collect())
        .unwrap_or_default()
}

/// The rows of the root `root`'s operations, in its command table's
/// order; none for a root whose service does not load.
pub fn root_rows(source: &ModelSource<'_>, root: &str) -> Vec<Row> {
    let table = &customizations::TABLE[..];
    let Some(service) = source.service(root) else {
        return Vec::new();
    };
    let model = &service.model;
    let endpoints = EndpointData::load(source.search(), &service.name, model.metadata());
    let mut rows = Vec::new();
    for operation in &service.operations {
        let found = service.operation(operation);
        let entry = found.and_then(|found| service.paginator(&found.name));
        let refusal = match (&endpoints, found) {
            (Err(_), _) => Some((PythonReason::EndpointData.name(), "-".to_owned())),
            (Ok(_), None) => Some((PythonReason::UnsupportedSubcommand.name(), "-".to_owned())),
            (Ok(endpoints), Some(found)) => static_refusal(
                model,
                found,
                endpoints,
                &service.name,
                table,
                entry.map(|entry| &entry.config),
            )
            .map(|unsupported| (admit::reason(&unsupported).name(), named(&unsupported))),
        };
        let (refusal, refused) = refusal.unwrap_or(("-", "-".to_owned()));
        let paginator = match entry.map(|entry| entry.config.as_ref()) {
            None => "-".to_owned(),
            Some(Err(refused)) => format!("refused:{refused}"),
            Some(Ok(_)) if paged(source, root, operation) => "native".to_owned(),
            Some(Ok(_)) => "unpaged".to_owned(),
        };
        rows.push(Row {
            command: root.to_owned(),
            operation: operation.clone(),
            refusal: refusal.to_owned(),
            refused,
            standing: standing_name(standing(source, table, root, operation)),
            paginator,
        });
    }
    rows
}

/// A standing's stable name.
pub fn standing_name(standing: Standing) -> String {
    match standing {
        Standing::NoArguments => "no_arguments".to_owned(),
        Standing::ScalarArguments => "scalar_arguments".to_owned(),
        Standing::RequiredNotScalar => "required_not_scalar".to_owned(),
        Standing::Refused(PythonReasonName(reason)) => format!("refused:{reason}"),
    }
}

/// What a refusal names, `-` for a refusal that names nothing.
fn named(unsupported: &Unsupported) -> String {
    match unsupported {
        Unsupported::Protocol(name)
        | Unsupported::Trait(name)
        | Unsupported::Builtin(name)
        | Unsupported::ClientContextParam(name)
        | Unsupported::Auth(name)
        | Unsupported::Hook(name) => name.clone(),
        Unsupported::StartingToken(why) => (*why).to_owned(),
        Unsupported::Paginator(refused) => refused.to_string(),
        Unsupported::OperationContextParams
        | Unsupported::UnsignedPayload
        | Unsupported::EndpointHeaders
        | Unsupported::RegionPattern
        | Unsupported::EndpointData
        | Unsupported::Validation(_) => "-".to_owned(),
    }
}

/// `unify_paging_params` gave the operation's table its paging options.
fn paged(source: &ModelSource<'_>, command: &str, operation: &str) -> bool {
    source
        .table(command, operation)
        .is_some_and(|table| table.paging.is_some())
}

/// A field as one TSV cell: backslash, tab, carriage return and newline
/// escaped, and `-` for an empty field.
fn cell(field: &str) -> String {
    if field.is_empty() {
        return "-".to_owned();
    }
    let mut escaped = String::with_capacity(field.len());
    for character in field.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}
