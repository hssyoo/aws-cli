"""The command tables the AWS CLI's table editors shape: the operations
`register_removals` deletes from a service's table
(awscli/customizations/removals.py) and the `wizard` command
`register_wizard_commands` adds (awscli/customizations/wizard/commands.py).

Their handlers only edit a table as it is built, so the native route
derives the edited table and runs the operations they leave: a deleted
operation is an invalid choice, which a table nothing else edits renders
natively, and `wizard` is a command of the table that is no model
operation, which runs under Python as `wait` does. Every expectation is
Python's, so a run forcing the Python route checks them all against it;
the tests marked `native_only` force the native route to show where each
input runs.
"""

import json
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any, Final

import pytest
import pytest_asyncio

from apihelpers import ApiSession, CommandRun, Reply, api_session

pytestmark = pytest.mark.asyncio

ROUTER: Final = "AWS_CLI_NATIVE_ROUTER"
# MainArgParser's usage (awscli/argparser.py:25-28), which ends a parse
# failure's report.
MAIN_USAGE: Final = (
    "usage: aws [options] <command> <subcommand> [<subcommand> ...] "
    "[parameters]\n"
    "To see help text, you can run:\n"
    "\n"
    "  aws help\n"
    "  aws <command> help\n"
    "  aws <command> <subcommand> help\n"
)


def refusal(reason: str) -> str:
    """The line forced native routing prints for `reason`."""
    return (
        f"aws: {ROUTER}=native: native implementation unavailable "
        f"({reason}); Python fallback disabled\n"
    )


def invalid_choice(value: str, maybe: list[str]) -> str:
    """The report of an operation the table lacks, with its close matches
    (`ServiceArgParser._check_value`, awscli/argparser.py:117-125;
    errorhandler.py's ParamValidation)."""
    suggestions = "".join(f"\n  * {word}" for word in maybe)
    if suggestions:
        suggestions = "Maybe you meant:\n" + suggestions + "\n"
    return (
        "\naws: [ERROR]: An error occurred (ParamValidation): argument "
        f"operation: Found invalid choice '{value}'\n\n{suggestions}\n"
        + MAIN_USAGE
    )


@pytest_asyncio.fixture
async def api(aws_cli: str, tmp_path: Path) -> AsyncIterator[ApiSession]:
    async with api_session(aws_cli, tmp_path) as session:
        yield session


def failed(run: CommandRun) -> tuple[str, str, int]:
    """A run's output and request count, for a command that fails before
    any request."""
    return run.stdout, run.stderr, len(run.calls)


def printed(value: dict[str, Any]) -> str:
    """The JSON output of `value`, as the CLI indents it."""
    return json.dumps(value, indent=4) + "\n"


# Operations of services whose tables the editors alone touched, which
# meet no other uncovered customization: lambda (rest-json) and ses
# (query), one of them paginated.
FREED: Final = {
    "lambda-list-functions-no-paginate": (
        "lambda list-functions --no-paginate",
        {"Functions": [{"FunctionName": "f"}]},
    ),
    "lambda-list-functions": (
        "lambda list-functions",
        {"Functions": [{"FunctionName": "f"}]},
    ),
    "lambda-get-account-settings": (
        "lambda get-account-settings",
        {"AccountLimit": {"TotalCodeSize": 80530636800},
         "AccountUsage": {"FunctionCount": 3}},
    ),
    "ses-get-account-sending-enabled": (
        "ses get-account-sending-enabled",
        {"Enabled": True},
    ),
}


@pytest.mark.parametrize("case", FREED)
async def test_an_operation_the_editors_leave_runs(
    api: ApiSession, case: str
) -> None:
    command, reply = FREED[case]
    run = await api.run(command, replies=Reply(reply))
    assert (run.stdout, run.stderr) == (printed(reply), ""), run.describe()
    assert len(run.calls) == 1


@pytest.mark.native_only
@pytest.mark.parametrize("case", FREED)
async def test_an_operation_the_editors_leave_runs_natively(
    api: ApiSession, case: str
) -> None:
    command, reply = FREED[case]
    api.env[ROUTER] = "native"
    run = await api.run(command, replies=Reply(reply))
    assert (run.stdout, run.stderr) == (printed(reply), ""), run.describe()
    assert len(run.calls) == 1


# A deleted operation, and a misspelling of one whose close matches no
# longer name it; emr's table is edited by emr_initialize as well.
REMOVED: Final = {
    "ses-verify-email-address": (
        "ses verify-email-address --email-address a@example.com",
        invalid_choice("verify-email-address", []),
    ),
    "lambda-invoke-with-response-stream": (
        "lambda invoke-with-response-stream --function-name f",
        invalid_choice("invoke-with-response-stream", []),
    ),
    "ses-verify-email-addres": (
        "ses verify-email-addres",
        invalid_choice("verify-email-addres", []),
    ),
    "emr-run-job-flow": (
        "emr run-job-flow",
        invalid_choice("run-job-flow", []),
    ),
}


@pytest.mark.parametrize("case", REMOVED)
async def test_a_removed_operation_is_an_invalid_choice(
    api: ApiSession, case: str
) -> None:
    command, stderr = REMOVED[case]
    run = await api.run(command, expected_rc=252)
    assert failed(run) == ("", stderr, 0), run.describe()


# Where a removed operation's failure renders: natively for a table the
# editors alone touch, under Python for emr's.
@pytest.mark.native_only
@pytest.mark.parametrize("case", REMOVED)
async def test_a_removed_operation_s_failure_renders_natively(
    api: ApiSession, case: str
) -> None:
    command, stderr = REMOVED[case]
    api.env[ROUTER] = "native"
    if case.startswith("emr-"):
        run = await api.run(command, expected_rc=255)
        stderr = refusal("unsupported_subcommand")
    else:
        run = await api.run(command, expected_rc=252)
    assert failed(run) == ("", stderr, 0), run.describe()


# `wizard` is a choice of each table it is added to, last before `wait`.
@pytest.mark.parametrize("command", ["lambda", "dynamodb", "iam", "events"])
async def test_wizard_is_among_the_close_matches(
    api: ApiSession, command: str
) -> None:
    run = await api.run(f"{command} wizar", expected_rc=252)
    expected = invalid_choice("wizar", ["wizard"])
    assert failed(run) == ("", expected, 0), run.describe()


@pytest.mark.native_only
async def test_wizard_s_close_match_renders_natively(api: ApiSession) -> None:
    api.env[ROUTER] = "native"
    run = await api.run("lambda wizar", expected_rc=252)
    expected = invalid_choice("wizar", ["wizard"])
    assert failed(run) == ("", expected, 0), run.describe()


# The wizard command takes a wizard name; with none and no `_main` wizard
# it reports its usage (TopLevelWizardCommand._run_main).
@pytest.mark.parametrize("command", ["lambda", "events"])
async def test_wizard_runs_under_python(api: ApiSession, command: str) -> None:
    run = await api.run(f"{command} wizard", expected_rc=252)
    expected = (
        "\naws: [ERROR]: An error occurred (ParamValidation): usage: aws "
        f"[options] {command} wizard <subcommand> [parameters]\n"
        "aws: [ERROR]: too few arguments\n"
    )
    assert failed(run) == ("", expected, 0), run.describe()


@pytest.mark.native_only
async def test_wizard_is_no_native_command(api: ApiSession) -> None:
    api.env[ROUTER] = "native"
    run = await api.run("lambda wizard", expected_rc=255)
    expected = refusal("unsupported_command")
    assert failed(run) == ("", expected, 0), run.describe()
