//! Which Python customizations the native route stands in for, and on
//! what basis: the one CLI-side table.
//!
//! The matcher (`command::patterns`) lists every registry pattern, builtin
//! row and client-creation row that an invocation's events meet. Each must
//! be covered, or the invocation is Python's (`UnportedCustomization`):
//!
//! - a botocore builtin or client-creation row by a `replaces` list of a
//!   client [`Customization`] in scope, which the `client` and
//!   `customizations` crates own;
//! - an awscli registry initializer, reached by one pattern, by a [`Row`] of
//!   [`CLI`], which this crate owns;
//! - a table editor, reached by any root command's table event, by an
//!   [`EditorRow`] of [`TABLE_EDITORS`].
//!
//! A row says two things. [`TableEdit`]: whether the derived tables
//! reproduce what the initializer does to them, which the derivation reads
//! ([`table_edits`], handed to `command::models::ModelSource`); an edit
//! they do not reproduce marks the table customized, so its parse failures
//! are Python's. [`Basis`]: why the invocation needs no Python for it when
//! it runs, which tests check: an `Implemented` path names an item that
//! exists, and an `Inert` row's refusal is the verdict of a case that would
//! make the initializer act (`coverage::tests`'s
//! `every_inert_refusal_refuses_where_the_initializer_acts`, and
//! `every_core_router_refusal_refuses_where_the_customization_acts` for the
//! client customizations' router refusals).
//!
//! An initializer is keyed by the pattern it was reached through because
//! the lazy emitter imports it the first time any of its patterns matches
//! (`LazyInitEmitter._ensure_initialized`), and what it then registers acts
//! on its own events: `cloudfront`'s, reached through `top-level-args-parsed`
//! alone, registers handlers for that service's operations and one
//! `context.update` on the shared event.
use cli_core::cli_error::PythonReason;
use client::hooks::{Basis, Customization, Refusal};
use command::patterns::{Match, MatchKind};

/// Whether the derived tables reproduce what an initializer does to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableEdit {
    /// It edits no table an operation's parse reads.
    None,
    /// It edits one, as the derivation reproduces: `how`.
    Reproduced { how: &'static str },
}

/// One awscli initializer reached by one registry pattern: what it does to
/// the tables, and the basis for running without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub pattern: &'static str,
    pub entry: &'static str,
    pub table: TableEdit,
    pub basis: Basis,
}

/// The lazy emitter applies `MAIN_COMMAND_TABLE_OPS` in place of every
/// `building-command-table.main` initializer and marks them initialized, so
/// none is imported (lazy_emitter.py:73-113); the root table applies the
/// same list.
const fn main_table(entry: &'static str) -> Row {
    Row {
        pattern: "building-command-table.main",
        entry,
        table: TableEdit::None,
        basis: Basis::Implemented {
            at: "command::models::ModelSource::roots",
        },
    }
}

const SESSION: &str = "session-initialized";
const ARGUMENT_TABLE: &str = "building-argument-table";
const COMMAND_TABLE: &str = "building-command-table";
const OPTION: Refusal = Refusal::Router(PythonReason::UnsupportedOption);

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
        pattern: ARGUMENT_TABLE,
        entry: "awscli.customizations.cliinput.register_cli_input_args",
        table: TableEdit::Reproduced {
            how: "the cross-cutting --cli-input-json and --cli-input-yaml",
        },
        // CliInputArgument merges a value into the call only when one is
        // given (cliinput.py:59); both options unpack as Unpack::Deferred.
        basis: Basis::Inert {
            acts_when: "--cli-input-json or --cli-input-yaml is supplied",
            refused_as: OPTION,
        },
    },
    Row {
        pattern: ARGUMENT_TABLE,
        entry: "awscli.customizations.paginate.register_pagination",
        table: TableEdit::Reproduced {
            how: "the paging options of a paginated operation",
        },
        // unify_paging_params returns for an operation without a paginator
        // (paginate.py:133-145). For one with, bind runs the handlers it
        // registers (check_should_enable_pagination,
        // ensure_paging_params_not_set,
        // check_should_enable_pagination_call_parameters) and
        // PageArgument.add_to_params, and the invocation pages through
        // client::paginate::PageIterator as _make_client_call does
        // (paginate.py:133-432, clidriver.py:1140); admission refuses a
        // paginator entry the native paginator does not run. Its
        // doc-description handler writes help, which is Python's.
        basis: Basis::Implemented {
            at: "service_command::invoke::paged::run",
        },
    },
    Row {
        pattern: ARGUMENT_TABLE,
        entry: "awscli.customizations.generatecliskeleton.register_generate_cli_skeleton",
        table: TableEdit::Reproduced {
            how: "--generate-cli-skeleton and its required-argument override",
        },
        // generate_skeleton returns without a value
        // (generatecliskeleton.py:94-97); the option unpacks as Deferred.
        basis: Basis::Inert {
            acts_when: "--generate-cli-skeleton is supplied",
            refused_as: OPTION,
        },
    },
    Row {
        pattern: "building-argument-table.*",
        entry: "awscli.customizations.streamingoutputarg.register_streaming_output_arg",
        table: TableEdit::Reproduced {
            how: "outfile for a streaming output",
        },
        // add_streaming_output_arg adds `outfile` only for a streaming
        // output (streamingoutputarg.py:25-36).
        basis: Basis::Inert {
            acts_when: "the output is streaming",
            refused_as: Refusal::Trait("streaming"),
        },
    },
    Row {
        pattern: COMMAND_TABLE,
        entry: "awscli.customizations.waiters.register_add_waiters",
        table: TableEdit::Reproduced {
            how: "`wait` for a service with waiters",
        },
        // add_waiters adds `wait` to the service's table and returns for an
        // operation's own, whose command object has no service_model
        // (waiters.py:28-45); the derived table adds the same `wait`.
        basis: Basis::Implemented {
            at: "command::models::ModelSource::has_waiters",
        },
    },
    Row {
        pattern: COMMAND_TABLE,
        entry: "awscli.alias.register_alias_commands",
        table: TableEdit::Reproduced {
            how: "nothing without an alias under the command",
        },
        // AliasSubCommandInjector.on_building_command_table (alias.py:166).
        basis: Basis::Inert {
            acts_when: "the alias file defines aliases under the command's tree",
            refused_as: Refusal::Router(PythonReason::CommandAlias),
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.paramfile.register_init_uri_param_handler",
        table: TableEdit::None,
        // file:// and fileb:// read as get_paramfile reads them
        // (LOCAL_PREFIX_MAP, paramfile.py:34,103); a read native cannot
        // match is deferred (UnsupportedValue).
        basis: Basis::Implemented {
            at: "awscli_argparse::unpack::unpack",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.binaryformat.register_init_binary_formatter",
        table: TableEdit::None,
        // Its input decoder acts on a supplied blob member
        // (base64_decode_input_blobs, binaryformat.py:38), which unpacks as
        // Deferred; its output parser is the client's identity blob hook
        // (invoke::cli_parse_hooks); an unknown cli_binary_format is a
        // ClientSetting refusal.
        basis: Basis::Inert {
            acts_when: "a blob member is supplied",
            refused_as: OPTION,
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.clidriver.register_no_pager_handler",
        table: TableEdit::None,
        // no_pager_handler clears the pager setting (clidriver.py:233).
        basis: Basis::Implemented {
            at: "format::pager::destination",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.assumerole.register_assume_role_provider",
        table: TableEdit::None,
        // It sets the assume-role provider's cache (assumerole.py:19).
        basis: Basis::Inert {
            acts_when: "the credential chain reaches the assume-role provider",
            refused_as: Refusal::Router(PythonReason::CredentialForm),
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.timestampformat.register_timestamp_format",
        table: TableEdit::None,
        // add_timestamp_parser (timestampformat.py:49).
        basis: Basis::Implemented {
            at: "format::timestamps::TimestampParser",
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.history.register_history_mode",
        table: TableEdit::None,
        // attach_history_handler (history/__init__.py:49).
        basis: Basis::Inert {
            acts_when: "cli_history is enabled",
            refused_as: Refusal::Router(PythonReason::HistoryEnabled),
        },
    },
    Row {
        pattern: SESSION,
        entry: "awscli.customizations.sso.register_sso_commands",
        table: TableEdit::None,
        // inject_json_file_cache sets the SSO provider's cache
        // (sso/__init__.py:38); its other handler builds `sso`'s table.
        basis: Basis::Inert {
            acts_when: "the credential chain reaches the SSO provider",
            refused_as: Refusal::Router(PythonReason::CredentialForm),
        },
    },
    Row {
        pattern: "top-level-args-parsed",
        entry: "awscli.customizations.globalargs.register_parse_global_args",
        table: TableEdit::None,
        // resolve_types, no_sign_request, resolve_verify_ssl and the timeout
        // handlers (globalargs.py:25).
        basis: Basis::Implemented {
            at: "cli_core::client_config::resolve_settings",
        },
    },
    Row {
        pattern: "top-level-args-parsed",
        entry: "awscli.customizations.cloudfront.register",
        table: TableEdit::None,
        // Its only handler on this event is `context.update`, recording the
        // parsed globals for one of that service's arguments
        // (cloudfront/__init__.py:63); the service's operations, which its
        // other handlers act on, speak rest-xml. Once native speaks it,
        // they reach the initializer through their own patterns, which no
        // row covers, and this row's refusal becomes UnportedCustomization.
        basis: Basis::Inert {
            acts_when: "the operation is one of that service's",
            refused_as: Refusal::Router(PythonReason::UnsupportedProtocol),
        },
    },
    Row {
        pattern: "process-cli-arg",
        entry: "awscli.argprocess.register_param_shorthand_parser",
        table: TableEdit::None,
        // ParamShorthandParser returns None for any member that is not a
        // structure, map or list of those (_is_complex_shape,
        // argprocess.py:265), which unpacks as Deferred.
        basis: Basis::Inert {
            acts_when: "a structure, map or list of them is supplied",
            refused_as: OPTION,
        },
    },
];

/// The initializers whose table edits the derived tables reproduce, which
/// the derivation does not count as customizing a table.
pub fn table_edits() -> Vec<&'static str> {
    let rows = CLI.iter().map(|row| (row.entry, row.table));
    let editors = TABLE_EDITORS.iter().map(|row| (row.entry, row.table));
    rows.chain(editors)
        .filter(|(_, table)| matches!(table, TableEdit::Reproduced { .. }))
        .map(|(entry, _)| entry)
        .collect()
}

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
    pub table: TableEdit,
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
        table: TableEdit::Reproduced {
            how: "the operations each `_remove_commands` handler deletes",
        },
        // The derivation deletes the operations each `_remove_commands`
        // handler deletes from its service's table, so they parse as invalid
        // choices and are no operation of the source; CommandRemover
        // registers nothing else (removals.py:28-154).
        basis: Basis::Implemented {
            at: "command::models::edit_table",
        },
    },
    EditorRow {
        entry: "awscli.customizations.wizard.commands.register_wizard_commands",
        table: TableEdit::Reproduced {
            how: "`wizard`, after the operations and before `wait`",
        },
        // The derivation sets `wizard` where `_add_wizard_command` does, and
        // `wizard-dev` on cli-dev, a hand-written table; neither is an
        // operation of the source, so invoking one is Python's
        // (wizard/commands.py:18-40, wizard/devcommands.py:18-21).
        basis: Basis::Implemented {
            at: "command::models::edit_table",
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
                && customization
                    .replaces
                    .iter()
                    .any(|(name, _)| *name == found.name.as_str())
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
