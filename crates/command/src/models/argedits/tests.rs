#![expect(
    clippy::panic,
    reason = "unexpected fixture outcomes must fail the test"
)]
use super::*;
use crate::models::table::derive_edited;
use model::shapes::Model;
use model::xform::SpecialCases;

/// The rows of `name = {...}` in argrename.py, the fallback's source, in
/// order.
fn python_map(name: &str) -> Vec<(String, String)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../aws-cli/awscli/customizations/argrename.py");
    let source = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}"));
    let start = source
        .find(&format!("{name} = {{"))
        .unwrap_or_else(|| panic!("no {name}"));
    let body = &source[start..];
    let body = &body[..body
        .find("\n}")
        .unwrap_or_else(|| panic!("{name} unclosed"))];
    body.lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let quoted: Vec<&str> = line.split('\'').collect();
            match quoted.as_slice() {
                [_, key, _, value, rest] if rest.trim() == "," || rest.is_empty() => {
                    ((*key).to_owned(), (*value).to_owned())
                }
                _ => panic!("{name}: unexpected line {line}"),
            }
        })
        .collect()
}

fn owned(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// The copies are the fallback's maps, row for row and in order: the
/// order is the handlers' registration order.
#[test]
fn the_maps_are_the_fallback_s() {
    assert_eq!(owned(ARGUMENT_RENAMES), python_map("ARGUMENT_RENAMES"));
    assert_eq!(owned(HIDDEN_ALIASES), python_map("HIDDEN_ALIASES"));
    assert!(ARGUMENT_RENAMES.len() > 80);
}

/// Every key's event portion names a command (never `*`) and an
/// operation or `*`, so each handler runs between `streamingoutputarg`'s
/// and the unscoped event's, where the derivation runs it.
#[test]
fn every_handler_runs_where_the_derivation_runs_it() {
    for portion in rename_portions().chain([&TOPLEVELBOOL_EVENT[TABLE_EVENT.len() + 1..]]) {
        let (command, operation) = portion
            .split_once('.')
            .unwrap_or_else(|| panic!("{portion}"));
        assert!(command != "*" && !operation.contains('.'), "{portion}");
        let event = format!("{TABLE_EVENT}.{command}.{}", operation.replace('*', "op"));
        let all = [ARGRENAME.to_owned(), TOPLEVELBOOL.to_owned()];
        let found = handlers(&event, &all).unwrap_or_else(|| panic!("{portion} misplaced"));
        assert!(!found.is_empty(), "{portion}");
    }
}

#[test]
fn the_emitter_calls_deeper_and_wildcard_nodes_first() {
    use std::cmp::Ordering::{Greater, Less};
    for (first, second) in [
        ("a.*", "a"),
        ("a.b.*", "a.b.c"),
        ("a.*", "a.b.*"),
        ("a.*.c", "a.b.c"),
        ("a.*.*", "a.*"),
        ("building-argument-table.*", "building-argument-table.ec2.*"),
        (
            "building-argument-table.ec2.*",
            "building-argument-table.ec2.op",
        ),
    ] {
        assert_eq!(emission_order(first, second), Less, "{first} {second}");
        assert_eq!(emission_order(second, first), Greater, "{second} {first}");
    }
    assert!(emission_order("a.b", "a.b").is_eq());
}

/// On ec2's create-image the ec2-wide renames run, then `pull_up_bool`
/// (argrename initializes first), then create-image's own rename; an
/// initializer not yet initialized registers nothing.
#[test]
fn handlers_run_by_node_then_registration() {
    let event = "building-argument-table.ec2.create-image";
    let both = [ARGRENAME.to_owned(), TOPLEVELBOOL.to_owned()];
    let found = handlers(event, &both).unwrap_or_else(|| panic!("placed"));
    let rename = |from, to| Handler::Rename { from, to };
    assert_eq!(
        found,
        [
            rename("no-egress", "ingress"),
            rename("no-disable-api-termination", "enable-api-termination"),
            Handler::PullUpBool,
            rename("no-no-reboot", "reboot"),
        ]
    );
    let flipped = [TOPLEVELBOOL.to_owned(), ARGRENAME.to_owned()];
    let found = handlers(event, &flipped).unwrap_or_else(|| panic!("placed"));
    assert_eq!(found.first(), Some(&Handler::PullUpBool));
    assert_eq!(handlers(event, &[]), Some(Vec::new()));
    let other = handlers(
        "building-argument-table.sns.subscribe",
        &[TOPLEVELBOOL.to_owned()],
    );
    assert_eq!(other, Some(Vec::new()));
}

const MODEL: &str = r#"{
  "metadata": {"protocol": "ec2", "apiVersion": "2020-01-01",
               "endpointPrefix": "zsvc", "serviceId": "Zsvc"},
  "operations": {
    "Modify": {"name": "Modify", "http": {"method": "POST", "requestUri": "/"},
               "input": {"shape": "ModifyRequest"}}
  },
  "shapes": {
    "Boolean": {"type": "boolean"}, "String": {"type": "string"},
    "Flag": {"type": "structure", "members": {"Value": {"shape": "Boolean"}}},
    "Named": {"type": "structure", "members": {"Other": {"shape": "Boolean"}}},
    "Pair": {"type": "structure", "members": {"Value": {"shape": "Boolean"},
                                              "More": {"shape": "String"}}},
    "ModifyRequest": {"type": "structure", "required": ["Id", "Ids"], "members": {
      "Optimized": {"shape": "Flag"}, "Id": {"shape": "String"},
      "Force": {"shape": "Boolean"}, "Named": {"shape": "Named"},
      "Pair": {"shape": "Pair"}, "Ids": {"shape": "String"}}}
  }
}"#;

fn edited(edit: impl FnOnce(&mut Table, OperationSchema<'_>)) -> Table {
    let model = Model::from_json(MODEL.as_bytes()).unwrap_or_else(|error| panic!("{error}"));
    let operation = model
        .schema()
        .service()
        .operation("Modify")
        .unwrap_or_else(|| panic!("no Modify"));
    derive_edited(operation, None, &SpecialCases::default(), |table| {
        edit(table, operation);
    })
}

fn names(table: &Table) -> Vec<&str> {
    table
        .arguments
        .iter()
        .map(|argument| &*argument.name)
        .collect()
}

fn find<'t>(table: &'t Table, name: &str) -> &'t DerivedArgument {
    table
        .arguments
        .iter()
        .find(|argument| &*argument.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

const MEMBERS: [&str; 7] = [
    "optimized",
    "id",
    "force",
    "no-force",
    "named",
    "pair",
    "ids",
];
const CROSS: [&str; 3] = ["cli-input-json", "cli-input-yaml", "generate-cli-skeleton"];

/// A rename moves the argument last, under its new name; a boolean keeps
/// its dest, anything else takes the new name's; a name the table lacks
/// changes nothing. The cross-cutting arguments still follow.
#[test]
fn a_rename_sets_the_new_name_last_and_deletes_the_old() {
    let table = edited(|table, _| {
        rename(table, "id", "identifier");
        rename(table, "no-force", "keep");
        rename(table, "absent", "nothing");
    });
    assert_eq!(
        names(&table),
        [
            &[
                "optimized",
                "force",
                "named",
                "pair",
                "ids",
                "identifier",
                "keep"
            ][..],
            &CROSS[..]
        ]
        .concat()
    );
    assert_eq!(find(&table, "identifier").dest, None);
    assert!(find(&table, "identifier").required);
    let keep = find(&table, "keep");
    assert_eq!(
        (keep.dest.as_deref(), keep.action),
        (Some("force"), Action::StoreFalse)
    );
    let table = edited(|table, _| rename(table, "id", "ids"));
    assert_eq!(
        names(&table),
        [
            &["optimized", "force", "no-force", "named", "pair", "ids"][..],
            &CROSS[..]
        ]
        .concat()
    );
    let member = |argument: &DerivedArgument| match &argument.origin {
        Origin::Member { member, .. } => member.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(&*member(find(&table, "ids")), "Id");
}

/// A hidden alias is an undocumented copy set last; a required original
/// and its copy are both left optional.
#[test]
fn a_hidden_alias_copies_the_argument_undocumented() {
    let table = edited(|table, _| hidden_alias(table, "ids", "i-ds"));
    assert_eq!(
        names(&table),
        [&MEMBERS[..], &["i-ds"], &CROSS[..]].concat()
    );
    let (original, alias) = (find(&table, "ids"), find(&table, "i-ds"));
    assert!(!original.required && !alias.required);
    assert!(alias.hidden && !original.hidden);
    assert_eq!(
        (alias.dest.as_deref(), &alias.origin),
        (None, &original.origin)
    );
}

/// Only a structure whose one member is the boolean `Value` is pulled up:
/// the positive argument where the member stood, the negative last.
#[test]
fn pull_up_bool_pairs_a_value_structure() {
    let table = edited(pull_up_bool);
    assert_eq!(
        names(&table),
        [&MEMBERS[..], &["no-optimized"], &CROSS[..]].concat()
    );
    assert_eq!(
        table.bool_pairs,
        [("optimized".into(), "no-optimized".into())]
    );
    for name in ["named", "pair", "force"] {
        assert!(
            matches!(find(&table, name).origin, Origin::Member { .. }),
            "{name}"
        );
    }
    assert!(!table.customized);
}

/// The positive: `store`, `nargs='?'`, the sentinel default, a value
/// unpacked as the structure.
#[test]
fn the_positive_takes_an_optional_value() {
    let table = edited(pull_up_bool);
    let positive = find(&table, "optimized");
    let shape = (
        positive.action,
        positive.nargs,
        positive.default,
        positive.unpack,
    );
    assert_eq!(
        shape,
        (
            Action::Store,
            Some(Nargs::Optional),
            ArgDefault::NotSpecified,
            Unpack::Deferred
        )
    );
    assert!(matches!(
        positive.origin,
        Origin::BoolValue { value: true, .. }
    ));
}

/// The negative: `store_true`, its own dest, the sentinel default.
#[test]
fn the_negative_stores_true() {
    let table = edited(pull_up_bool);
    let negative = find(&table, "no-optimized");
    let shape = (negative.action, negative.nargs, negative.dest.as_deref());
    assert_eq!(shape, (Action::StoreTrue, None, None));
    assert_eq!(
        (negative.default, negative.unpack),
        (ArgDefault::NotSpecified, Unpack::AsIs)
    );
    assert_eq!(&*negative.cli_type_name, "structure");
    assert!(matches!(
        negative.origin,
        Origin::BoolValue { value: false, .. }
    ));
}

/// A rename of half a pulled-up pair is not reproduced.
#[test]
fn renaming_a_pulled_up_argument_customizes_the_table() {
    let table = edited(|table, operation| {
        pull_up_bool(table, operation);
        rename(table, "no-optimized", "plain");
    });
    assert!(table.customized);
}
