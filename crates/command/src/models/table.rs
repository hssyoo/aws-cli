//! One operation's argument table, as `ServiceOperation._create_argument_table`
//! and the cross-cutting `building-argument-table` handlers build it.
use super::*;
use awscli_argparse::{Action, Nargs};
use model::shapes::{Flag, Member, Operation, ShapeId, ShapeKind};

/// How argparse converts an option's value (`add_argument(type=...)`):
/// `CLIArgument.TYPE_MAP` gives `long` members `int` and `float`/`double`
/// members `float`, and `PageArgument.type_map` gives the integer page
/// options `int`; the parse applies it (`awscli_argparse::convert`).
pub use awscli_argparse::ValueType;

/// What a derived argument stands for, which says how it binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// An input member: its serialized name and its shape. A boolean
    /// member's `--no-` twin has the same origin.
    Member {
        member: Box<str>,
        shape: ShapeId,
    },
    /// `cliinput`'s `--cli-input-json` and `--cli-input-yaml`.
    CliInputJson,
    CliInputYaml,
    /// `generatecliskeleton`'s `--generate-cli-skeleton`.
    GenerateCliSkeleton,
    /// `paginate`'s `--starting-token`, `--page-size` and `--max-items`.
    StartingToken,
    PageSize,
    MaxItems,
    /// `streamingoutputarg`'s `outfile`, naming the response's streamed
    /// member.
    Outfile {
        response_key: Box<str>,
    },
}

/// One argument of a derived table: what the argparse table needs, and
/// what binding needs to read its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedArgument {
    /// The table's key and the option's spelling without `--`.
    pub name: Box<str>,
    /// The namespace destination, when it is not argparse's default.
    pub dest: Option<Box<str>>,
    pub positional: bool,
    pub action: Action,
    pub nargs: Option<Nargs>,
    pub constant: Option<&'static str>,
    pub choices: &'static [&'static str],
    /// `default=None` given explicitly (`BooleanArgument`).
    pub default_null: bool,
    pub required: bool,
    pub value_type: Option<ValueType>,
    /// `_UNDOCUMENTED`: kept out of help and of the unknown-option
    /// suggestions (`get_option_suggestion_candidates`).
    pub hidden: bool,
    /// `cli_type_name`: the member's shape type, or the customization's.
    pub cli_type_name: Box<str>,
    pub origin: Origin,
}

/// A derived table and what decides whether native may judge its parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub arguments: Vec<DerivedArgument>,
    /// The options that lift the required arguments.
    pub overrides: Vec<(&'static str, Option<&'static str>)>,
    /// A `building-argument-table` handler, or another table event's, that
    /// the derivation does not reproduce touches the table, or the paging
    /// customization would raise while it builds it.
    pub customized: bool,
}

impl Table {
    /// Whether an argument's value goes through an argparse type.
    pub fn typed(&self) -> bool {
        self.arguments
            .iter()
            .any(|argument| argument.value_type.is_some())
    }
    /// Insert as an `OrderedDict` assignment does: a new key at the end, an
    /// existing key replaced where it stands.
    fn insert(&mut self, argument: DerivedArgument) {
        match self
            .arguments
            .iter_mut()
            .find(|held| held.name == argument.name)
        {
            Some(held) => *held = argument,
            None => self.arguments.push(argument),
        }
    }
    fn has(&self, name: &str) -> bool {
        self.arguments
            .iter()
            .any(|argument| &*argument.name == name)
    }
}

fn plain(name: &str, cli_type_name: &str, origin: Origin) -> DerivedArgument {
    DerivedArgument {
        name: name.into(),
        dest: None,
        positional: false,
        action: Action::Store,
        nargs: None,
        constant: None,
        choices: &[],
        default_null: false,
        required: false,
        value_type: None,
        hidden: false,
        cli_type_name: cli_type_name.into(),
        origin,
    }
}

/// The CLI arguments of one input member: `ServiceOperation.ARG_TYPES` picks
/// `ListArgument` for a list, `BooleanArgument` (and its `--no-` twin) for a
/// boolean, and `CLIArgument` for the rest.
fn member_arguments(
    model: &Model,
    member: &Member,
    required: bool,
    special: &SpecialCases,
) -> Vec<DerivedArgument> {
    let shape = model.shape(member.shape);
    let name = xform_name(&member.name, "-", special);
    let type_name = kind_name(&shape.kind);
    let origin = Origin::Member {
        member: member.name.clone(),
        shape: member.shape,
    };
    let mut argument = plain(&name, type_name, origin);
    match shape.kind {
        ShapeKind::Boolean => {
            argument.action = Action::StoreTrue;
            argument.default_null = true;
            let mut negative = argument.clone();
            negative.name = format!("no-{name}").into();
            negative.action = Action::StoreFalse;
            negative.dest = Some(name.replace('-', "_").into());
            return vec![argument, negative];
        }
        ShapeKind::List { .. } => argument.nargs = Some(Nargs::ZeroOrMore),
        ShapeKind::Long => argument.value_type = Some(ValueType::Int),
        ShapeKind::Float | ShapeKind::Double => argument.value_type = Some(ValueType::Float),
        _ => {}
    }
    argument.required = required;
    vec![argument]
}

/// botocore's `type_name` for a shape kind.
pub fn kind_name(kind: &ShapeKind) -> &str {
    match kind {
        ShapeKind::Structure { .. } => "structure",
        ShapeKind::List { .. } => "list",
        ShapeKind::Map { .. } => "map",
        ShapeKind::String => "string",
        ShapeKind::Integer => "integer",
        ShapeKind::Long => "long",
        ShapeKind::Boolean => "boolean",
        ShapeKind::Float => "float",
        ShapeKind::Double => "double",
        ShapeKind::Timestamp => "timestamp",
        ShapeKind::Blob => "blob",
        ShapeKind::Other(name) => name,
    }
}

/// The members' arguments in model order. A member is required when the
/// input shape lists it and it is no idempotency token.
fn members_table(model: &Model, operation: &Operation, special: &SpecialCases) -> Table {
    let mut table = Table {
        arguments: Vec::new(),
        overrides: Vec::new(),
        customized: false,
    };
    let Some(input) = operation.input else {
        return table;
    };
    let required = &model.shape(input).traits.required;
    for member in model.members(input) {
        let token = model.shape(member.shape).traits.is(Flag::IdempotencyToken);
        let is_required = required.contains(&member.name) && !token;
        for argument in member_arguments(model, member, is_required, special) {
            table.insert(argument);
        }
    }
    table
}

/// `OperationModel.has_streaming_output`: the output's payload member is a
/// blob (`_get_streaming_body`).
fn streaming_output(model: &Model, operation: &Operation) -> Option<Box<str>> {
    let output = operation.output?;
    let payload = model.shape(output).traits.payload.as_deref()?;
    let member = model
        .members(output)
        .iter()
        .find(|member| &*member.name == payload)?;
    matches!(model.shape(member.shape).kind, ShapeKind::Blob).then(|| payload.into())
}

/// `streamingoutputarg`, then `cliinput`, `paginate` and
/// `generatecliskeleton`, in the order the emitter calls their handlers:
/// the `building-argument-table.*` node's before the unscoped node's, and
/// these in the order their initializers registered them.
fn cross_cutting(
    table: &mut Table,
    model: &Model,
    operation: &Operation,
    pages: Option<&Json>,
    special: &SpecialCases,
) {
    if let Some(response_key) = streaming_output(model, operation) {
        let mut outfile = plain("outfile", "string", Origin::Outfile { response_key });
        outfile.positional = true;
        table.insert(outfile);
    }
    if !table.has("outfile") {
        for (name, origin) in [
            ("cli-input-json", Origin::CliInputJson),
            ("cli-input-yaml", Origin::CliInputYaml),
        ] {
            table.insert(plain(name, "string", origin));
            table.overrides.push((override_option(name), None));
        }
    }
    if let Some(config) = pages {
        paging(table, model, operation, config, special);
    }
    if !table.has("outfile") {
        let mut skeleton = plain(
            "generate-cli-skeleton",
            "string",
            Origin::GenerateCliSkeleton,
        );
        skeleton.nargs = Some(Nargs::Optional);
        skeleton.constant = Some("input");
        skeleton.choices = &["input", "output", "yaml-input"];
        table.insert(skeleton);
        table
            .overrides
            .push(("--generate-cli-skeleton", Some("output")));
    }
}

const CLI_INPUT_OVERRIDES: [&str; 2] = ["--cli-input-json", "--cli-input-yaml"];
fn override_option(name: &str) -> &'static str {
    if name == "cli-input-yaml" {
        CLI_INPUT_OVERRIDES[1]
    } else {
        CLI_INPUT_OVERRIDES[0]
    }
}

/// `unify_paging_params`: the paginator's input tokens and limit key are
/// hidden, then `--starting-token`, `--page-size` (with a limit key) and
/// `--max-items` are set, each replacing an argument of its name in place.
/// A limit key of a type `PageArgument` cannot parse, or a token with no
/// argument to hide, makes Python raise as it builds the table.
fn paging(
    table: &mut Table,
    model: &Model,
    operation: &Operation,
    config: &Json,
    special: &SpecialCases,
) {
    let tokens: Vec<&str> = match config.get("input_token") {
        Some(Json::String(token)) => vec![token.as_str()],
        Some(Json::Array(tokens)) => tokens.iter().filter_map(Json::as_str).collect(),
        _ => Vec::new(),
    };
    let limit_key = config.get("limit_key").and_then(Json::as_str);
    for name in tokens.iter().copied().chain(limit_key) {
        let cli_name = xform_name(name, "-", special);
        match table
            .arguments
            .iter_mut()
            .find(|argument| *argument.name == *cli_name)
        {
            Some(argument) => argument.hidden = true,
            None => table.customized = true,
        }
    }
    table.insert(plain("starting-token", "string", Origin::StartingToken));
    let limit_type = match limit_key {
        None => Some("integer"),
        Some(key) => limit_type(model, operation, key),
    };
    let Some(type_name) = limit_type else {
        table.customized = true;
        return;
    };
    let value_type = (type_name != "string").then_some(ValueType::Int);
    if limit_key.is_some() {
        let mut page_size = plain("page-size", type_name, Origin::PageSize);
        page_size.value_type = value_type;
        table.insert(page_size);
    }
    let mut max_items = plain("max-items", type_name, Origin::MaxItems);
    max_items.value_type = value_type;
    table.insert(max_items);
}

/// The limit key member's type, when `PageArgument.type_map` has it.
fn limit_type<'m>(model: &'m Model, operation: &Operation, key: &str) -> Option<&'m str> {
    let input = operation.input?;
    let member = model
        .members(input)
        .iter()
        .find(|member| &*member.name == key)?;
    let name = kind_name(&model.shape(member.shape).kind);
    matches!(name, "string" | "integer" | "long").then_some(name)
}

/// The operation's table: its members, then the cross-cutting handlers'.
pub fn derive(
    model: &Model,
    operation: &Operation,
    pages: Option<&Json>,
    special: &SpecialCases,
) -> Table {
    let mut table = members_table(model, operation, special);
    cross_cutting(&mut table, model, operation, pages, special);
    table
}

#[cfg(test)]
mod tests;
