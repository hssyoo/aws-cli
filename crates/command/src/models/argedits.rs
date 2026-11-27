//! The `building-argument-table` handlers of two registry initializers the
//! derivation reproduces, on an operation's own table event:
//!
//! - `awscli.customizations.argrename.register_arg_renames`, which
//!   registers one `rename_arg` per row of `ARGUMENT_RENAMES` and one
//!   `hidden_alias` per row of `HIDDEN_ALIASES` (argrename.py), each on
//!   `building-argument-table.<key without its last part>`. Both maps are
//!   static data; [`ARGUMENT_RENAMES`] and [`HIDDEN_ALIASES`] are copies,
//!   which a test holds to the fallback's source. Their scope is the
//!   commands their keys name, and nothing else.
//! - `awscli.customizations.toplevelbool.register_bool_params`, which
//!   registers `pull_up_bool` on `building-argument-table.ec2.*`.
//!
//! A handler runs only once its initializer was initialized by the time
//! the event is emitted (`Patterns::initialized`), and they run in the
//! order the emitter calls them: by node (`patterns::emission_order`),
//! then in the order they were registered, which is their initializers'
//! order and, within argrename, the maps' order.
use super::table::{ArgDefault, DerivedArgument, Origin, Table, Unpack};
use crate::patterns::{Patterns, emission_order, pattern_matches};
use awscli_argparse::{Action, Nargs};
use model::schema::{OperationSchema, ShapeType};

/// The initializer whose handlers rename arguments and add hidden aliases.
pub const ARGRENAME: &str = "awscli.customizations.argrename.register_arg_renames";
/// The initializer whose handler pulls `structure(Value: boolean)` members
/// up to `--flag` and `--no-flag`.
pub const TOPLEVELBOOL: &str = "awscli.customizations.toplevelbool.register_bool_params";
/// The event `register_bool_params` registers `pull_up_bool` on.
const TOPLEVELBOOL_EVENT: &str = "building-argument-table.ec2.*";
const TABLE_EVENT: &str = "building-argument-table";

/// `ARGUMENT_RENAMES`: `<command>.<operation or *>.<argument>` to the
/// argument's new name.
pub static ARGUMENT_RENAMES: &[(&str, &str)] = &[
    ("ec2.create-image.no-no-reboot", "reboot"),
    ("ec2.*.no-egress", "ingress"),
    ("ec2.*.no-disable-api-termination", "enable-api-termination"),
    ("swf.register-activity-type.version", "activity-version"),
    ("swf.register-workflow-type.version", "workflow-version"),
    ("datapipeline.*.query", "objects-query"),
    (
        "datapipeline.get-pipeline-definition.version",
        "pipeline-version",
    ),
    ("emr.*.job-flow-ids", "cluster-ids"),
    ("emr.*.job-flow-id", "cluster-id"),
    ("cloudsearchdomain.search.query", "search-query"),
    ("cloudsearchdomain.suggest.query", "suggest-query"),
    ("sns.subscribe.endpoint", "notification-endpoint"),
    ("deploy.*.s-3-location", "s3-location"),
    ("deploy.*.ec-2-tag-filters", "ec2-tag-filters"),
    ("codepipeline.get-pipeline.version", "pipeline-version"),
    (
        "codepipeline.create-custom-action-type.version",
        "action-version",
    ),
    (
        "codepipeline.delete-custom-action-type.version",
        "action-version",
    ),
    (
        "kinesisanalytics.add-application-output.output",
        "application-output",
    ),
    (
        "kinesisanalyticsv2.add-application-output.output",
        "application-output",
    ),
    (
        "route53.delete-traffic-policy.version",
        "traffic-policy-version",
    ),
    (
        "route53.get-traffic-policy.version",
        "traffic-policy-version",
    ),
    (
        "route53.update-traffic-policy-comment.version",
        "traffic-policy-version",
    ),
    ("gamelift.create-build.version", "build-version"),
    ("gamelift.update-build.version", "build-version"),
    ("gamelift.create-script.version", "script-version"),
    ("gamelift.update-script.version", "script-version"),
    ("route53domains.view-billing.start", "start-time"),
    ("route53domains.view-billing.end", "end-time"),
    ("apigateway.create-rest-api.version", "api-version"),
    ("apigatewayv2.create-api.version", "api-version"),
    ("apigatewayv2.update-api.version", "api-version"),
    ("pinpoint.get-campaign-version.version", "campaign-version"),
    ("pinpoint.get-segment-version.version", "segment-version"),
    ("pinpoint.delete-email-template.version", "template-version"),
    (
        "pinpoint.delete-in-app-template.version",
        "template-version",
    ),
    ("pinpoint.delete-push-template.version", "template-version"),
    ("pinpoint.delete-sms-template.version", "template-version"),
    ("pinpoint.delete-voice-template.version", "template-version"),
    ("pinpoint.get-email-template.version", "template-version"),
    ("pinpoint.get-in-app-template.version", "template-version"),
    ("pinpoint.get-push-template.version", "template-version"),
    ("pinpoint.get-sms-template.version", "template-version"),
    ("pinpoint.get-voice-template.version", "template-version"),
    ("pinpoint.update-email-template.version", "template-version"),
    (
        "pinpoint.update-in-app-template.version",
        "template-version",
    ),
    ("pinpoint.update-push-template.version", "template-version"),
    ("pinpoint.update-sms-template.version", "template-version"),
    ("pinpoint.update-voice-template.version", "template-version"),
    ("stepfunctions.send-task-success.output", "task-output"),
    ("clouddirectory.publish-schema.version", "schema-version"),
    ("mturk.list-qualification-types.query", "types-query"),
    (
        "workdocs.create-notification-subscription.endpoint",
        "notification-endpoint",
    ),
    ("workdocs.describe-users.query", "user-query"),
    ("lex-models.delete-bot.version", "bot-version"),
    ("lex-models.delete-intent.version", "intent-version"),
    ("lex-models.delete-slot-type.version", "slot-type-version"),
    ("lex-models.get-intent.version", "intent-version"),
    ("lex-models.get-slot-type.version", "slot-type-version"),
    ("lex-models.delete-bot-version.version", "bot-version"),
    ("lex-models.delete-intent-version.version", "intent-version"),
    (
        "lex-models.delete-slot-type-version.version",
        "slot-type-version",
    ),
    ("lex-models.get-export.version", "resource-version"),
    ("license-manager.get-grant.version", "grant-version"),
    ("license-manager.delete-grant.version", "grant-version"),
    ("license-manager.get-license.version", "license-version"),
    (
        "rekognition.create-stream-processor.output",
        "stream-processor-output",
    ),
    ("eks.create-cluster.version", "kubernetes-version"),
    ("eks.update-cluster-version.version", "kubernetes-version"),
    ("eks.create-nodegroup.version", "kubernetes-version"),
    ("eks.update-nodegroup-version.version", "kubernetes-version"),
    (
        "eks.update-cluster-components-version.version",
        "kubernetes-version",
    ),
    ("schemas.*.version", "schema-version"),
    ("sagemaker.delete-image-version.version", "version-number"),
    ("sagemaker.describe-image-version.version", "version-number"),
    ("sagemaker.list-aliases.version", "version-number"),
    ("sagemaker.update-image-version.version", "version-number"),
    ("iotwireless.*.lo-ra-wan", "lorawan"),
    ("codepipeline.get-action-type.version", "action-version"),
    ("ecs.*.no-enable-execute-command", "disable-execute-command"),
    ("ecs.execute-command.no-interactive", "non-interactive"),
    (
        "controltower.create-landing-zone.version",
        "landing-zone-version",
    ),
    (
        "controltower.update-landing-zone.version",
        "landing-zone-version",
    ),
    (
        "glue.get-unfiltered-partition-metadata.region",
        "resource-region",
    ),
    (
        "glue.get-unfiltered-partitions-metadata.region",
        "resource-region",
    ),
    (
        "glue.get-unfiltered-table-metadata.region",
        "resource-region",
    ),
    ("agent-toolkit.*.name", "skill-name"),
    ("agent-toolkit.search-skills.query", "search-query"),
];

/// `HIDDEN_ALIASES`: the same keys, to an undocumented alias's name.
pub static HIDDEN_ALIASES: &[(&str, &str)] = &[
    (
        "mgn.*.replication-servers-security-groups-ids",
        "replication-servers-security-groups-i-ds",
    ),
    ("mgn.*.source-server-ids", "source-server-i-ds"),
    (
        "mgn.*.replication-configuration-template-ids",
        "replication-configuration-template-i-ds",
    ),
    (
        "elasticache.create-replication-group.preferred-cache-cluster-azs",
        "preferred-cache-cluster-a-zs",
    ),
];

/// One reproduced handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handler {
    /// `rename_arg(from, to)`.
    Rename {
        from: &'static str,
        to: &'static str,
    },
    /// `hidden_alias(from, to)`.
    HiddenAlias {
        from: &'static str,
        to: &'static str,
    },
    /// `pull_up_bool`.
    PullUpBool,
}

/// The event portion and argument name of a map key (`rsplit('.', 1)`).
fn split(key: &'static str) -> (&'static str, &'static str) {
    key.rsplit_once('.').unwrap_or(("", key))
}

/// The patterns argrename registers its handlers on, without the
/// `building-argument-table.` prefix: the commands (and operations) its
/// maps cover.
pub fn rename_portions() -> impl Iterator<Item = &'static str> {
    ARGUMENT_RENAMES
        .iter()
        .chain(HIDDEN_ALIASES)
        .map(|(key, _)| split(key).0)
}

/// What the reproduced handlers on `building-argument-table.<command>.
/// <operation>` do, in the order they run: those of the initializers
/// `initialized` names, as they register them, sorted by node. `None`
/// when one of them would run outside the place the derivation gives
/// them: after `building-argument-table.*`'s handlers and before the
/// unscoped event's.
fn handlers(event: &str, initialized: &[String]) -> Option<Vec<Handler>> {
    let mut registered: Vec<(String, Handler)> = Vec::new();
    for entry in initialized {
        if entry == ARGRENAME {
            let renames = ARGUMENT_RENAMES.iter().map(|&(key, to)| {
                (
                    key,
                    Handler::Rename {
                        from: split(key).1,
                        to,
                    },
                )
            });
            let aliases = HIDDEN_ALIASES.iter().map(|&(key, to)| {
                (
                    key,
                    Handler::HiddenAlias {
                        from: split(key).1,
                        to,
                    },
                )
            });
            for (key, handler) in renames.chain(aliases) {
                registered.push((format!("{TABLE_EVENT}.{}", split(key).0), handler));
            }
        } else if entry == TOPLEVELBOOL {
            registered.push((TOPLEVELBOOL_EVENT.to_owned(), Handler::PullUpBool));
        }
    }
    registered.retain(|(pattern, _)| pattern_matches(pattern, event));
    // A stable sort keeps each node's handlers in registration order.
    registered.sort_by(|(a, _), (b, _)| emission_order(a, b));
    let placed = registered.iter().all(|(pattern, _)| {
        emission_order(&format!("{TABLE_EVENT}.*"), pattern).is_lt()
            && emission_order(pattern, TABLE_EVENT).is_lt()
    });
    placed.then(|| registered.into_iter().map(|(_, handler)| handler).collect())
}

/// Run the reproduced handlers of `[command, operation]`'s table event on
/// `table`. A table they cannot be reproduced on is customized.
pub fn apply(
    table: &mut Table,
    operation: OperationSchema<'_>,
    patterns: &Patterns,
    command: &str,
    cli_operation: &str,
) {
    let events = [
        format!("building-command-table.{command}_{cli_operation}"),
        format!("before-building-argument-table-parser.{command}.{cli_operation}"),
        format!("{TABLE_EVENT}.{command}.{cli_operation}"),
    ];
    let initialized = patterns.initialized(&events);
    let Some(handlers) = handlers(&events[2], &initialized) else {
        table.customized = true;
        return;
    };
    for handler in handlers {
        match handler {
            Handler::Rename { from, to } => rename(table, from, to),
            Handler::HiddenAlias { from, to } => hidden_alias(table, from, to),
            Handler::PullUpBool => pull_up_bool(table, operation),
        }
    }
}

/// The argument's dest as argparse names it.
fn dest_of(argument: &DerivedArgument) -> Box<str> {
    argument
        .dest
        .clone()
        .unwrap_or_else(|| argument.name.replace('-', "_").into())
}

/// `argument` named `name`: a boolean argument keeps the dest it was built
/// with (`BooleanArgument._destination`), any other takes its new name's.
fn named(argument: &DerivedArgument, name: &str) -> DerivedArgument {
    let mut copy = argument.clone();
    if matches!(argument.action, Action::StoreTrue | Action::StoreFalse) {
        copy.dest = Some(dest_of(argument));
    }
    copy.name = name.into();
    copy
}

/// `utils.rename_argument`, when the table has `from`: the argument is set
/// under `to` (where `to` stands, or last), then `from` is deleted. A
/// renamed half of a pulled-up pair would make `validate_boolean_mutex_
/// groups` read a name argparse never set, which the derivation does not
/// reproduce.
fn rename(table: &mut Table, from: &str, to: &str) {
    let Some(current) = table.arguments.iter().find(|held| &*held.name == from) else {
        return;
    };
    let renamed = named(current, to);
    if table
        .bool_pairs
        .iter()
        .any(|(positive, negative)| &**positive == from || &**negative == from)
    {
        table.customized = true;
    }
    table.insert(renamed);
    table.arguments.retain(|held| &*held.name != from);
}

/// `utils.make_hidden_alias`, when the table has `from`: an undocumented
/// copy set under `to`; a required original and its copy are both left
/// optional.
fn hidden_alias(table: &mut Table, from: &str, to: &str) {
    let Some(index) = table.arguments.iter().position(|held| &*held.name == from) else {
        return;
    };
    let mut alias = named(&table.arguments[index], to);
    alias.hidden = true;
    if alias.required {
        table.arguments[index].required = false;
        alias.required = false;
    }
    table.insert(alias);
}

/// `_qualifies_for_simplification`: a structure whose one member is a
/// boolean named `Value` (`detect_shape_structure` gives such a structure
/// `structure(scalar)`).
fn qualifies(shape: model::schema::Schema<'_>) -> bool {
    if shape.shape_type() != ShapeType::Structure {
        return false;
    }
    let mut members = shape.members();
    matches!(
        (members.next(), members.next()),
        (Some(only), None)
            if only.member_name() == Some("Value") && only.shape_type() == ShapeType::Boolean
    )
}

/// `pull_up_bool`: each model argument whose member qualifies becomes a
/// `PositiveBooleanArgument` where it stands (`--flag`, `--flag VALUE`;
/// `nargs='?'`, `default=_NOT_SPECIFIED`) and gains a
/// `NegativeBooleanParameter` `no-<flag>` (`store_true`,
/// `default=_NOT_SPECIFIED`), set last; each pair is one
/// `validate_boolean_mutex_groups` checks.
fn pull_up_bool(table: &mut Table, operation: OperationSchema<'_>) {
    let Some(input) = operation.input() else {
        return;
    };
    let snapshot = table.arguments.clone();
    for argument in snapshot {
        let Origin::Member { member, shape } = &argument.origin else {
            continue;
        };
        let Some(schema) = input
            .members()
            .find(|schema| schema.member_name() == Some(member))
        else {
            continue;
        };
        if !qualifies(schema) {
            continue;
        }
        let positive = DerivedArgument {
            name: argument.name.clone(),
            dest: None,
            positional: false,
            action: Action::Store,
            nargs: Some(Nargs::Optional),
            constant: None,
            choices: &[],
            default: ArgDefault::NotSpecified,
            required: false,
            value_type: None,
            hidden: false,
            cli_type_name: argument.cli_type_name.clone(),
            origin: Origin::BoolValue {
                member: member.clone(),
                shape: *shape,
                value: true,
            },
            // A value is unpacked as the structure it is.
            unpack: Unpack::Deferred,
        };
        let negative_name: Box<str> = format!("no-{}", argument.name).into();
        let negative = DerivedArgument {
            name: negative_name.clone(),
            action: Action::StoreTrue,
            nargs: None,
            origin: Origin::BoolValue {
                member: member.clone(),
                shape: *shape,
                value: false,
            },
            unpack: Unpack::AsIs,
            ..positive.clone()
        };
        table.insert(positive);
        table.insert(negative);
        table
            .bool_pairs
            .push((argument.name.clone(), negative_name));
    }
}

#[cfg(test)]
mod tests;
