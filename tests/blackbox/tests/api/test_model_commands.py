"""The generic service command over the repository's fake models
(tests/models, through `AWS_DATA_PATH`): each feature the native route
claims, with the output, failures and request the Python route gives.
A run forcing the Python route checks every expectation against Python.
"""

import json
import re
import uuid
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Final

import pytest
import pytest_asyncio

from apihelpers import ApiSession, Reply, api_session, fake_model_protocol
from localstub import HTTPResponse

pytestmark = pytest.mark.asyncio

MODELS: Final = Path(__file__).resolve().parents[3] / "models"
SERVICES: Final = ("zjson", "zjson11", "zquery", "zglobal")
USAGE: Final = (
    "\nusage: aws [options] <command> <subcommand> [<subcommand> ...] "
    "[parameters]\nTo see help text, you can run:\n\n  aws help\n  "
    "aws <command> help\n  aws <command> <subcommand> help\n"
)


@pytest_asyncio.fixture
async def models(aws_cli: str, tmp_path: Path) -> AsyncIterator[ApiSession]:
    async with api_session(aws_cli, tmp_path) as session:
        session.env["AWS_DATA_PATH"] = str(MODELS)
        # A zone without daylight-saving time, which iso8601 output of an
        # epoch needs on the native route.
        session.env["TZ"] = "UTC"
        session.protocol = fake_model_protocol(MODELS, SERVICES)
        yield session


async def test_scalar_members_bind_in_model_order(models: ApiSession) -> None:
    run = await models.run(
        "zjson describe --on --total 99999999999999999999 --count 7 --text hello",
        replies={},
    )
    params = run.last_params or {}
    token = params.pop("Token", None)
    assert params == {
        "Text": "hello",
        "Count": 7,
        "Total": 99999999999999999999,
        "On": True,
    }, run.describe()
    # The idempotency token botocore fills in is a fresh uuid4.
    assert token is not None and uuid.UUID(token).version == 4
    body = json.loads(run.calls[0].request.body or b"{}")
    assert list(body) == ["Text", "Count", "Total", "On", "Token"]


async def test_a_boolean_s_twin_sets_it_false(models: ApiSession) -> None:
    run = await models.run("zjson describe --no-on", replies={})
    assert (run.last_params or {}).get("On") is False


async def test_a_missing_required_argument_fails_the_parse(models: ApiSession) -> None:
    run = await models.run("zjson strict --name n", expected_rc=252)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (ParamValidation): the following "
        f"arguments are required: --size\n{USAGE}"
    ), run.describe()


async def test_an_unknown_argument_is_named(models: ApiSession) -> None:
    run = await models.run("zjson ping --bogus", expected_rc=252)
    assert run.calls == ()
    assert run.stderr == (
        f"{USAGE}\n\naws: [ERROR]: Unknown options: --bogus\n"
    ), run.describe()


async def test_an_integer_member_fails_as_int_does(models: ApiSession) -> None:
    run = await models.run("zjson describe --count x", expected_rc=255)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: invalid literal for int() with base 10: 'x'\n"
    ), run.describe()


async def test_validation_reports_before_any_request(models: ApiSession) -> None:
    run = await models.run(["zjson", "strict", "--name", "", "--size", "0"], expected_rc=252)
    assert run.calls == ()
    assert run.stderr.startswith(
        "\naws: [ERROR]: An error occurred (ParamValidation): Parameter "
        "validation failed:\n"
    ), run.describe()


async def test_json_and_text_print_the_output(models: ApiSession) -> None:
    reply = {
        "Text": "hello",
        "Count": 3,
        "On": False,
        "Items": ["a", "b"],
        "Nested": {},
        "Pairs": {"k": "v"},
    }
    run = await models.run("zjson describe", replies=reply)
    assert json.loads(run.stdout) == reply, run.describe()
    assert run.stdout.startswith('{\n    "Text": "hello",\n')
    run = await models.run("zjson describe --output text", replies=reply)
    assert run.stdout == "3\tFalse\thello\nITEMS\ta\nITEMS\tb\nPAIRS\tv\n", run.describe()


CONFLICT: Final = Reply(
    {
        "Error": {
            "Code": "Conflict",
            "Message": "in use",
            "Resource": "thing-1",
            "Retry": 3,
        }
    },
    status=409,
)
CONFLICT_TEXT: Final = (
    "\naws: [ERROR]: An error occurred (Conflict) when calling the Describe "
    "operation (reached max retries: 0): in use\n"
)


async def test_a_modelled_error_names_its_code(models: ApiSession) -> None:
    run = await models.run("zjson describe", replies=CONFLICT, expected_rc=254)
    assert run.stderr == CONFLICT_TEXT, run.describe()


async def test_a_server_error_without_a_body_names_its_status(models: ApiSession) -> None:
    run = await models.run(
        "zjson ping",
        replies=HTTPResponse(status=503, headers={}, body=b""),
        expected_rc=254,
    )
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (503) when calling the Ping operation "
        "(reached max retries: 0): Service Unavailable\n"
    ), run.describe()


async def test_a_host_prefix_names_its_label(models: ApiSession) -> None:
    run = await models.run("zjson locate --zone z1", replies={})
    host = run.calls[0].request.headers.get("host") or ""
    assert host.startswith("z1.data."), host


async def test_the_query_protocol_sends_a_form(models: ApiSession) -> None:
    run = await models.run("zquery get-thing --thing-id t-1", replies={"ThingId": "t-1"})
    request = run.calls[0].request
    assert request.method == "POST"
    # GetThing's ClientToken is an idempotency token, filled with a uuid4.
    assert re.fullmatch(
        rb"Action=GetThing&Version=2020-01-01&ThingId=t-1&ClientToken=[0-9a-f-]{36}",
        request.body or b"",
    ), request.body
    assert json.loads(run.stdout) == {"ThingId": "t-1"}


async def test_a_global_service_still_needs_a_region(models: ApiSession) -> None:
    # endpoints.json names no partition endpoint for a user model, so
    # client creation raises NoRegionError before any request.
    del models.env["AWS_DEFAULT_REGION"]
    run = await models.run("zglobal whoami", expected_rc=253)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (NoRegion): You must specify a region. "
        'You can also configure your region by running "aws configure".\n'
    ), run.describe()


async def test_a_global_rule_set_names_one_host(models: ApiSession) -> None:
    hosts = set()
    for region in ("us-east-1", "eu-west-1"):
        run = await models.run(["zglobal", "whoami", "--region", region], replies={})
        hosts.add(run.calls[0].request.headers.get("host"))
    assert len(hosts) == 1, hosts
