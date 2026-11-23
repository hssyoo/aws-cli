//! Which Python customizations the native route stands in for, and on
//! what basis.
//!
//! The matcher (`command::patterns`) lists every registry pattern, builtin
//! row and client-creation row that an invocation's events meet. Each must
//! be covered, or the invocation is Python's (`UnportedCustomization`):
//!
//! - a botocore builtin or client-creation row by a `replaces` list of a
//!   client [`Customization`] in scope, which the `client` and
//!   `customizations` crates own;
//! - an awscli registry initializer, reached by one pattern, by a [`Row`] of
//!   [`CLI`], which this crate owns. Its [`Basis`] says why: the native
//!   route implements what the handlers it registers do, or they do
//!   nothing for an invocation that the native route admits, because the
//!   case where they act is refused before any request;
//! - a table editor, reached by any root command's table event, by an
//!   [`EditorRow`] of [`TABLE_EDITORS`].
//!
//! An initializer is keyed by the pattern it was reached through because
//! the lazy emitter imports it the first time any of its patterns matches
//! (`LazyInitEmitter._ensure_initialized`), and what it then registers acts
//! on its own events: `cloudfront`'s, reached through `top-level-args-parsed`
//! alone, registers handlers for that service's operations and one
//! `context.update` on the shared event. The derived table's shape
//! (`command::models::TABLE_EDITS`) is no basis for an initializer whose
//! handlers act as the command runs: it is what parses, not what runs. A
//! table editor's handlers do nothing but shape a table, so for it the
//! derived table is the basis.
use cli_core::cli_error::PythonReason;
use client::hooks::Customization;
use command::patterns::{Match, MatchKind};

/// Why a registry initializer, reached by one pattern, needs no Python.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The native route does what its handlers do, at `at`.
    Native { at: &'static str },
    /// Its handlers do nothing unless `acts_when`; the native route refuses
    /// that case as `refused_as`, at `by`, before any request. `None`: the
    /// case never reaches a model operation's native route at all.
    Inert {
        acts_when: &'static str,
        refused_as: Option<PythonReason>,
        by: &'static str,
    },
}

/// One awscli initializer reached by one registry pattern, and its basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub pattern: &'static str,
    pub entry: &'static str,
    pub basis: Basis,
}

const MAIN_TABLE: Basis = Basis::Native {
    at: "command::models::ModelSource::roots applies MAIN_COMMAND_TABLE_OPS, which the lazy \
         emitter applies in place of every building-command-table.main initializer and then \
         marks them initialized, so none is imported (lazy_emitter.py:73-113)",
};

const fn main_table(entry: &'static str) -> Row {
    Row {
        pattern: "building-command-table.main",
        entry,
        basis: MAIN_TABLE,
    }
}

const SESSION: &str = "session-initialized";

/// The awscli initializers every model operation reaches, with their
/// bases. One reached by any other pattern is uncovered.
pub static CLI: &[Row] = &[
    main_table("awscli.customizations.s3.s3.register_s3_main"),
    main_table("awscli.customizations.dynamodb.ddb.register_ddb"),
    main_table("awscli.customizations.configure.configure.register_configure_cmd"),
    main_table("awscli.customizations.codedeploy.codedeploy.register_rename_codedeploy"),
    main_table("awscli.customizations.configservice.rename_cmd.register_rename_config"),
    main_table("awscli.customizations.history.register_history_commands"),
    main_table("awscli.customizations.devcommands.register_dev_commands"),
    main_table("awscli.customizations.login.register_login_cmds"),
    main_table("awscli.customizations.update.register_update_command"),
    Row {
        pattern: "building-argument-table",
        entry: "awscli.customizations.cliinput.register_cli_input_args",
        basis: Basis::Inert {
            acts_when: "--cli-input-json or --cli-input-yaml is supplied: CliInputArgument \
                        merges a value into the call's parameters only when one is given \
                        (cliinput.py:59)",
            refused_as: Some(PythonReason::UnsupportedOption),
            by: "both options unpack as Unpack::Deferred (command::models::table), which the \
                 unpack loop stops at and the router names (cli-router generic::deferred_reason)",
        },
    },
    Row {
        pattern: "building-argument-table",
        entry: "awscli.customizations.paginate.register_pagination",
        basis: Basis::Inert {
            acts_when: "the operation has a paginator: unify_paging_params returns for one \
                        without (paginate.py:133-145)",
            refused_as: Some(PythonReason::UnsupportedTrait),
            by: "admit::traits refuses a paginated operation",
        },
    },
    Row {
        pattern: "building-argument-table",
        entry: "awscli.customizations.generatecliskeleton.register_generate_cli_skeleton",
        basis: Basis::Inert {
            acts_when: "--generate-cli-skeleton is supplied: generate_skeleton returns without \
                        a value (generatecliskeleton.py:94-97), and its required-argument \
                        override only lifts what the parse then checks, as the derived \
                        table's overrides do",
            refused_as: Some(PythonReason::UnsupportedOption),
            by: "the option unpacks as Unpack::Deferred (command::models::table)",
        },
    },
    Row {
        pattern: "building-argument-table.*",
        entry: "awscli.customizations.streamingoutputarg.register_streaming_output_arg",
        basis: Basis::Inert {
            acts_when: "the output is streaming: add_streaming_output_arg adds `outfile` only \
                        then (streamingoutputarg.py:25-36)",
            refused_as: Some(PythonReason::UnsupportedTrait),
            by: "admit::traits refuses a streaming output",
        },
    },
    Row {
        pattern: "building-command-table",
        entry: "awscli.customizations.waiters.register_add_waiters",
        basis: Basis::Inert {
            acts_when: "the service's command table is built: add_waiters adds `wait` there, \
                        and returns for an operation's own table, whose command object has no \
                        service_model (waiters.py:28-45); an operation's invocation never runs \
                        the `wait` command it adds",
            refused_as: None,
            by: "the `wait` command is no operation of the source (command::models::Service)",
        },
    },
    Row {
        pattern: "building-command-table",
        entry: "awscli.alias.register_alias_commands",
        basis: Basis::Inert {
            acts_when: "the alias file defines aliases under the command's tree \
                        (AliasSubCommandInjector.on_building_command_table, alias.py:166)",
            refused_as: Some(PythonReason::CommandAlias),
            by: "cli-router defines_alias, before binding",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.paramfile.register_init_uri_param_handler",
        basis: Basis::Native {
            at: "awscli_argparse::unpack reads file:// and fileb:// values as get_paramfile \
                 does (LOCAL_PREFIX_MAP, paramfile.py:34,103) and defers the reads it cannot \
                 match (UnsupportedValue)",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.binaryformat.register_init_binary_formatter",
        basis: Basis::Inert {
            acts_when: "a blob member is supplied (base64_decode_input_blobs on \
                        provide-client-params, binaryformat.py:38), or a response holds a blob, \
                        which the identity blob parser leaves as its base64 text, as the \
                        client's ParseOptions does (client::protocol::ParseOptions); an \
                        unknown cli_binary_format is a ClientSetting refusal",
            refused_as: Some(PythonReason::UnsupportedOption),
            by: "a blob member unpacks as Unpack::Deferred (command::models::table)",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.clidriver.register_no_pager_handler",
        basis: Basis::Native {
            at: "format::pager::Chain reads --no-cli-pager as no_pager_handler clears the \
                 pager setting (clidriver.py:233)",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.assumerole.register_assume_role_provider",
        basis: Basis::Inert {
            acts_when: "the credential chain reaches the assume-role provider, whose cache it \
                        sets; building the chain refuses instance-metadata settings, which the \
                        router renders natively (cli-router session_start) \
                        (assumerole.py:19)",
            refused_as: Some(PythonReason::CredentialForm),
            by: "the router's credential stage refuses assume-role and web-identity profiles",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.timestampformat.register_timestamp_format",
        basis: Basis::Native {
            at: "format::TimestampParser parses response timestamps as add_timestamp_parser \
                 selects (timestampformat.py:49)",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.history.register_history_mode",
        basis: Basis::Inert {
            acts_when: "cli_history is enabled (attach_history_handler, history/__init__.py:49)",
            refused_as: Some(PythonReason::HistoryEnabled),
            by: "the SETTINGS row for cli_history (cli-router effective_config)",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.sso.register_sso_commands",
        basis: Basis::Inert {
            acts_when: "the credential chain reaches the SSO provider, whose cache it sets \
                        (inject_json_file_cache, sso/__init__.py:38); its other handler builds \
                        the `sso` command's table only",
            refused_as: Some(PythonReason::CredentialForm),
            by: "the router's credential stage refuses SSO profiles",
        },
    },
    Row {
        pattern: "top-level-args-parsed",
        entry: "awscli.customizations.globalargs.register_parse_global_args",
        basis: Basis::Native {
            at: "cli-router's globals and client_config resolve the query, signing, SSL \
                 verification and timeouts as resolve_types, no_sign_request, \
                 resolve_verify_ssl and the timeout handlers do (globalargs.py:25)",
        },
    },
    Row {
        pattern: "top-level-args-parsed",
        entry: "awscli.customizations.cloudfront.register",
        basis: Basis::Inert {
            acts_when: "the operation is one of that service's three it customizes: its only \
                        handler on this event is `context.update`, recording the parsed \
                        globals for a later argument (cloudfront/__init__.py:63); its \
                        operations reach it through their own patterns, which no row covers",
            refused_as: None,
            by: "this row covers the shared event alone",
        },
    },
    Row {
        pattern: "process-cli-arg",
        entry: "awscli.argprocess.register_param_shorthand_parser",
        basis: Basis::Inert {
            acts_when: "the member is a structure, map or list of those: ParamShorthandParser \
                        returns None for any other (_is_complex_shape, argprocess.py:265)",
            refused_as: Some(PythonReason::UnsupportedOption),
            by: "such a member unpacks as Unpack::Deferred (command::models::table)",
        },
    },
];

/// An initializer covered through any root command's table event
/// (`building-command-table.<command>`), whichever command it is.
///
/// Keying such an initializer by each pattern it is listed under would add
/// nothing but a row per service upstream lists it for: the lazy emitter
/// calls an initializer once, with the emitter alone (`fn(self)`,
/// lazy_emitter.py:125-137), so what it registers does not depend on the
/// pattern that reached it. A table editor registers nothing but handlers
/// on root command tables, which the pack records by running it from the
/// source the fallback runs (`command::patterns::TableEditor`), and the
/// derived table applies each where the lazy emitter would run it
/// (`command::patterns::Patterns::table_edits`). The pack refuses an editor
/// listed under any other pattern or registering on any other event, and
/// the shape is checked here too: a pattern of this shape is met only by a
/// root's own table event, and an editor met through any other pattern is
/// still uncovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorRow {
    pub entry: &'static str,
    pub basis: Basis,
}

const TABLE_EVENT: &str = "building-command-table.";

/// Whether `pattern` is one root command's table event: one command, not
/// the main table's (whose initializers the main table's ops replace) and
/// no subcommand's, whose lineage is `_`-joined
/// (`ServiceOperation._build_subcommand_table`, clidriver.py:891).
fn root_table_event(pattern: &str) -> bool {
    pattern.strip_prefix(TABLE_EVENT).is_some_and(|command| {
        !command.is_empty() && command != "main" && command != "*" && !command.contains(['.', '_'])
    })
}

/// The table editors, covered through every root's table event. Their
/// handlers act on the building of a table, never on an invocation's
/// arguments or calls, and the derived table holds what they leave in it.
pub static TABLE_EDITORS: &[EditorRow] = &[
    EditorRow {
        entry: "awscli.customizations.removals.register_removals",
        basis: Basis::Native {
            at: "command::models::ModelSource::load deletes the operations each \
                 `_remove_commands` handler deletes from its service's table, so they parse \
                 as invalid choices and are no operation of the source; CommandRemover \
                 registers nothing else (removals.py:28-154)",
        },
    },
    EditorRow {
        entry: "awscli.customizations.wizard.commands.register_wizard_commands",
        basis: Basis::Native {
            at: "command::models::ModelSource::load sets `wizard` where `_add_wizard_command` \
                 does, after the operations and before `wait`, and `wizard-dev` on cli-dev, \
                 a hand-written table; neither is an operation of the source, so invoking \
                 one is Python's (wizard/commands.py:18-40, wizard/devcommands.py:18-21)",
        },
    },
];

/// The first match nothing covers: no [`CLI`] row for a registry
/// initializer and its pattern, nor an [`EditorRow`] for it through a root
/// table event, and no `replaces` of a customization in scope for
/// `service` and `operation` (model names) for a botocore row.
pub fn uncovered<'m>(
    matches: &'m [Match],
    customizations: &[&Customization],
    service: &str,
    operation: &str,
) -> Option<&'m Match> {
    matches.iter().find(|found| match found.kind {
        MatchKind::Registry => !covered_initializer(found),
        MatchKind::Builtin | MatchKind::Client => !customizations.iter().any(|customization| {
            customization.scope.matches(service, operation)
                && customization.replaces.contains(&found.name.as_str())
        }),
    })
}

/// Whether a [`CLI`] row covers the registry match, or an [`EditorRow`]
/// does through a root table event.
fn covered_initializer(found: &Match) -> bool {
    let row = CLI
        .iter()
        .any(|row| row.pattern == found.pattern && row.entry == found.name);
    let editor = || {
        root_table_event(&found.pattern) && TABLE_EDITORS.iter().any(|row| row.entry == found.name)
    };
    row || editor()
}

#[cfg(test)]
mod tests;
