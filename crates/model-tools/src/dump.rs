//! The derived tables in the form `cargo xtask check-argparse --derived`
//! compares with the AWS CLI's own (`tests/argparse/argtables.py`).
use awscli_argparse::{Action, Nargs, ValueType};
use command::models::{ArgDefault, DerivedArgument, ModelSource};
use serde_json::{Map, Value as Json, json};

fn action(action: Action) -> &'static str {
    match action {
        Action::Store => "store",
        Action::StoreTrue => "store_true",
        Action::StoreFalse => "store_false",
        Action::Append => "append",
        Action::AppendNamed => "append_named",
        Action::Command => "command",
        Action::Version => "version",
    }
}

fn nargs(nargs: Option<Nargs>) -> Json {
    match nargs {
        None => Json::Null,
        Some(Nargs::Optional) => "?".into(),
        Some(Nargs::ZeroOrMore) => "*".into(),
        Some(Nargs::OneOrMore) => "+".into(),
        Some(Nargs::Count(count)) => count.into(),
    }
}

/// One argument as argparse holds it, with the table object's facts.
pub fn argument(argument: &DerivedArgument) -> Json {
    let dest = argument
        .dest
        .as_deref()
        .map_or_else(|| argument.name.replace('-', "_"), str::to_owned);
    json!({
        "name": &*argument.name,
        "positional": argument.positional,
        "action": action(argument.action),
        "nargs": nargs(argument.nargs),
        "const": argument.constant,
        // `plain_json` of `toplevelbool`'s sentinel is its type's name.
        "default": match argument.default {
            ArgDefault::Unset | ArgDefault::Null => Json::Null,
            ArgDefault::NotSpecified => "<object>".into(),
        },
        "choices": argument.choices,
        "dest": dest,
        "required": argument.required || (argument.positional && argument.nargs.is_none()),
        "type": argument.value_type.map(|value_type| match value_type {
            ValueType::Int => "int",
            ValueType::Float => "float",
        }),
        "hidden": argument.hidden,
        "cli_type_name": &*argument.cli_type_name,
    })
}

/// The model roots, and for each of `commands` that is one, its table and
/// its operations' tables with what decides whether they render natively.
pub fn tables(source: &ModelSource<'_>, commands: &[String]) -> Json {
    let roots: Vec<&str> = source
        .roots()
        .map(|roots| roots.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let mut services = Map::new();
    for command in commands {
        let Some(service) = source.service(command) else {
            services.insert(command.clone(), Json::Null);
            continue;
        };
        let mut operations = Map::new();
        for name in &service.operations {
            let Some(table) = source.table(command, name) else {
                continue;
            };
            let arguments: Vec<Json> = table.arguments.iter().map(argument).collect();
            operations.insert(
                name.clone(),
                json!({"customized": table.customized, "typed": table.typed(),
                       "arguments": arguments}),
            );
        }
        services.insert(
            command.clone(),
            json!({"customized": service.customized, "choices": &service.choices,
                   "operations": operations}),
        );
    }
    json!({"roots": roots, "services": services})
}
