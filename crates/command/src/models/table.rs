//! One operation's argument table, as `ServiceOperation._create_argument_table`
//! and the cross-cutting `building-argument-table` handlers build it.
use super::*;
use awscli_argparse::{Action, Nargs};
use model::schema::{Flag, OperationSchema, Schema, SchemaId, ShapeType};

/// How argparse converts an option's value (`add_argument(type=...)`):
/// `CLIArgument.TYPE_MAP` gives `long` members `int` and `float`/`double`
/// members `float`, and `PageArgument.type_map` gives the integer page
/// options `int`; the parse applies it (`awscli_argparse::convert`).
pub use awscli_argparse::ValueType;

/// What the unpack loop does with a supplied value (`_build_call_parameters`
/// → `unpack_argument`, then `CLIArgument._unpack_argument` →
/// `unpack_cli_arg`), as far as native binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpack {
    /// `unpack_scalar_cli_arg` returns it as it stands: a string member
    /// that is no JSON-value header, a long (which argparse's `int` already
    /// converted, and `int()` leaves), or a boolean.
    AsIs,
    /// An integer member, which argparse stores as text:
    /// `unpack_scalar_cli_arg`'s `int()`.
    Integer,
    /// A `streaming` blob member, which argparse stores as text:
    /// `unpack_scalar_cli_arg` expands it as a path (`expandvars`, then
    /// `expanduser`) and opens the file it names, or raises that blob
    /// values must be a path to a file.
    File,
    /// Only Python unpacks or binds it: a list, map, structure, blob,
    /// timestamp, float or double member, a JSON-value header or document,
    /// or a customization's option.
    Deferred,
}

/// What a derived argument stands for, which says how it binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// An input member: its serialized name and its schema. A boolean
    /// member's `--no-` twin has the same origin.
    Member {
        member: Box<str>,
        shape: SchemaId,
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
    /// `toplevelbool`'s pair for a `structure(Value: boolean)` member:
    /// `PositiveBooleanArgument` (`value` true), which binds
    /// `{"Value": true}` when given without a value, and
    /// `NegativeBooleanParameter` (`value` false), which binds
    /// `{"Value": false}`.
    BoolValue {
        member: Box<str>,
        shape: SchemaId,
        value: bool,
    },
}

/// The `default` an argument gives argparse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgDefault {
    /// None given: argparse's own.
    Unset,
    /// `default=None` given explicitly (`BooleanArgument`).
    Null,
    /// `toplevelbool`'s `_NOT_SPECIFIED` sentinel, which tells an argument
    /// left out from one given without a value.
    NotSpecified,
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
    pub default: ArgDefault,
    pub required: bool,
    pub value_type: Option<ValueType>,
    /// `_UNDOCUMENTED`: kept out of help and of the unknown-option
    /// suggestions (`get_option_suggestion_candidates`).
    pub hidden: bool,
    /// `cli_type_name`: the member's shape type, or the customization's.
    pub cli_type_name: Box<str>,
    pub origin: Origin,
    pub unpack: Unpack,
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
    /// What the paging customization registered for the parse, when it
    /// built the table without raising.
    pub paging: Option<Paging>,
    /// `toplevelbool`'s pairs, positive then negative name, in the order
    /// `validate_boolean_mutex_groups` checks them.
    pub bool_pairs: Vec<(Box<str>, Box<str>)>,
}

/// What `unify_paging_params` hands the handlers it registers,
/// `check_should_enable_pagination` on `operation-args-parsed` and
/// `check_should_enable_pagination_call_parameters` on `calling-command`:
/// the paginator's input tokens and limit key, and the arguments its page
/// options shadowed, which the first puts back when paging is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paging {
    /// `_get_all_input_tokens` and `_get_all_cli_input_tokens`: the input
    /// tokens, then the limit key.
    pub tokens: Vec<PagingToken>,
    /// `shadowed_args`: each argument a page option replaced
    /// (`add_paging_argument`), named as the option is, in the order they
    /// were replaced.
    pub shadowed: Vec<DerivedArgument>,
}

/// One of the paginator's input tokens or its limit key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagingToken {
    /// As the model names it.
    pub model: Box<str>,
    /// Through `xform_name`, as its argument is named.
    pub cli: Box<str>,
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
    pub(super) fn insert(&mut self, argument: DerivedArgument) {
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
        default: ArgDefault::Unset,
        required: false,
        value_type: None,
        hidden: false,
        cli_type_name: cli_type_name.into(),
        origin,
        unpack: Unpack::Deferred,
    }
}

/// The CLI arguments of one input member: `ServiceOperation.ARG_TYPES` picks
/// `ListArgument` for a list, `BooleanArgument` (and its `--no-` twin) for a
/// boolean, and `CLIArgument` for the rest.
fn member_arguments(
    schema: Schema<'_>,
    required: bool,
    special: &SpecialCases,
) -> Vec<DerivedArgument> {
    let member = schema.member_name().unwrap_or_default();
    let name = xform_name(member, "-", special);
    let origin = Origin::Member {
        member: member.into(),
        shape: schema.index(),
    };
    let mut argument = plain(&name, schema.type_name(), origin);
    argument.unpack = unpack(schema);
    match schema.shape_type() {
        ShapeType::Boolean => {
            argument.action = Action::StoreTrue;
            argument.default = ArgDefault::Null;
            let mut negative = argument.clone();
            negative.name = format!("no-{name}").into();
            negative.action = Action::StoreFalse;
            negative.dest = Some(name.replace('-', "_").into());
            return vec![argument, negative];
        }
        ShapeType::List => argument.nargs = Some(Nargs::ZeroOrMore),
        ShapeType::Long => argument.value_type = Some(ValueType::Int),
        ShapeType::Float | ShapeType::Double => argument.value_type = Some(ValueType::Float),
        _ => {}
    }
    argument.required = required;
    vec![argument]
}

/// How `_unpack_cli_arg` treats a member of `shape`: a JSON-value header
/// (`is_json_value_header`) goes through `json.loads`, the scalars through
/// `unpack_scalar_cli_arg`, a streaming blob among them.
fn unpack(shape: Schema<'_>) -> Unpack {
    let json_header = shape.flag(Flag::JsonValue) && shape.location() == Some("header");
    match shape.shape_type() {
        ShapeType::String if !json_header => Unpack::AsIs,
        ShapeType::Long | ShapeType::Boolean => Unpack::AsIs,
        ShapeType::Integer => Unpack::Integer,
        ShapeType::Blob if shape.flag(Flag::Streaming) => Unpack::File,
        _ => Unpack::Deferred,
    }
}

/// The members' arguments in model order. A member is required when the
/// input shape lists it and it is no idempotency token.
fn members_table(operation: OperationSchema<'_>, special: &SpecialCases) -> Table {
    let mut table = Table {
        arguments: Vec::new(),
        overrides: Vec::new(),
        customized: false,
        paging: None,
        bool_pairs: Vec::new(),
    };
    let Some(input) = operation.input() else {
        return table;
    };
    for schema in input.members() {
        let token = schema.flag(Flag::IdempotencyToken);
        let listed = input
            .required()
            .any(|name| Some(name) == schema.member_name());
        let is_required = listed && !token;
        for argument in member_arguments(schema, is_required, special) {
            table.insert(argument);
        }
    }
    table
}

/// `OperationModel.has_streaming_output`: the output's payload member is a
/// blob (`_get_streaming_body`).
fn streaming_output(operation: OperationSchema<'_>) -> Option<Box<str>> {
    let output = operation.output()?;
    let payload = output.payload()?;
    let member = output
        .members()
        .find(|member| member.member_name() == Some(payload))?;
    (member.shape_type() == ShapeType::Blob).then(|| payload.into())
}

/// `streamingoutputarg`, the `building-argument-table.*` node's handler,
/// which the emitter calls before any command's own.
fn streaming(table: &mut Table, operation: OperationSchema<'_>) {
    if let Some(response_key) = streaming_output(operation) {
        let mut outfile = plain("outfile", "string", Origin::Outfile { response_key });
        outfile.positional = true;
        // `StreamingOutputArgument.add_to_params` keeps the text, after
        // `load-cli-arg`, as the path to write.
        outfile.unpack = Unpack::AsIs;
        table.insert(outfile);
    }
}

/// `cliinput`, `paginate` and `generatecliskeleton`, the unscoped node's,
/// which the emitter calls last, in the order their initializers
/// registered them.
fn cross_cutting(
    table: &mut Table,
    operation: OperationSchema<'_>,
    entry: Option<&Entry>,
    special: &SpecialCases,
) {
    if !table.has("outfile") {
        for (name, origin) in [
            ("cli-input-json", Origin::CliInputJson),
            ("cli-input-yaml", Origin::CliInputYaml),
        ] {
            table.insert(plain(name, "string", origin));
            table.overrides.push((override_option(name), None));
        }
    }
    if let Some(entry) = entry {
        paging(table, operation, entry, special);
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
/// `--max-items` are set, each replacing an argument of its name in place
/// and keeping it as shadowed. A limit key of a type `PageArgument` cannot
/// parse, or a token with no argument to hide, makes Python raise as it
/// builds the table. The page options' values are strings or, through
/// argparse's `int`, integers, which binding reads as they stand.
fn paging(
    table: &mut Table,
    operation: OperationSchema<'_>,
    entry: &Entry,
    special: &SpecialCases,
) {
    let mut tokens: Vec<&str> = entry.input_token.iter().map(String::as_str).collect();
    let limit_key = entry.limit_key.as_deref();
    tokens.extend(limit_key);
    let mut paging = Paging {
        tokens: Vec::new(),
        shadowed: Vec::new(),
    };
    for name in tokens {
        let cli_name = xform_name(name, "-", special);
        match table
            .arguments
            .iter_mut()
            .find(|argument| *argument.name == *cli_name)
        {
            Some(argument) => argument.hidden = true,
            None => table.customized = true,
        }
        paging.tokens.push(PagingToken {
            model: name.into(),
            cli: cli_name.into(),
        });
    }
    let mut starting_token = plain("starting-token", "string", Origin::StartingToken);
    starting_token.unpack = Unpack::AsIs;
    shadow(table, &mut paging, starting_token);
    let limit_type = match limit_key {
        None => Some("integer"),
        Some(key) => limit_type(operation, key),
    };
    let Some(type_name) = limit_type else {
        table.customized = true;
        return;
    };
    let value_type = (type_name != "string").then_some(ValueType::Int);
    let page_option = |name: &str, origin: Origin| DerivedArgument {
        value_type,
        unpack: Unpack::AsIs,
        ..plain(name, type_name, origin)
    };
    if limit_key.is_some() {
        shadow(
            table,
            &mut paging,
            page_option("page-size", Origin::PageSize),
        );
    }
    shadow(
        table,
        &mut paging,
        page_option("max-items", Origin::MaxItems),
    );
    if !table.customized {
        table.paging = Some(paging);
    }
}

/// `add_paging_argument`: `option` replaces the argument of its name,
/// which is kept as shadowed.
fn shadow(table: &mut Table, paging: &mut Paging, option: DerivedArgument) {
    if let Some(held) = table.arguments.iter().find(|held| held.name == option.name) {
        paging.shadowed.push(held.clone());
    }
    table.insert(option);
}

/// The limit key member's type, when `PageArgument.type_map` has it.
fn limit_type<'s>(operation: OperationSchema<'s>, key: &str) -> Option<&'s str> {
    let input = operation.input()?;
    let member = input
        .members()
        .find(|member| member.member_name() == Some(key))?;
    let name = member.type_name();
    matches!(name, "string" | "integer" | "long").then_some(name)
}

/// The operation's table: its members, then the cross-cutting handlers'.
#[cfg(test)]
pub fn derive(
    operation: OperationSchema<'_>,
    entry: Option<&Entry>,
    special: &SpecialCases,
) -> Table {
    derive_edited(operation, entry, special, |_| {})
}

/// [`derive`], with `edit` run where a command's own
/// `building-argument-table` handlers run: after `streamingoutputarg`'s,
/// before the unscoped event's.
pub fn derive_edited(
    operation: OperationSchema<'_>,
    entry: Option<&Entry>,
    special: &SpecialCases,
    edit: impl FnOnce(&mut Table),
) -> Table {
    let mut table = members_table(operation, special);
    streaming(&mut table, operation);
    edit(&mut table);
    cross_cutting(&mut table, operation, entry, special);
    table
}

#[cfg(test)]
mod tests;
