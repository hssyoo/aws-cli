"""The generic service command over the repository's fake models
(tests/models, through `AWS_DATA_PATH`): each feature the native route
claims, with the output, failures and request the Python route gives.
A run forcing the Python route checks every expectation against Python.
"""

import base64
import json
import os
import re
import uuid
from pathlib import Path
from typing import Final

import pytest

from apihelpers import (
    ApiSession,
    Reply,
    attempt_limit,
    fake_models_fixture,
    retries_clause,
)
from harness_plugin import FORCED_NATIVE, MAIN_USAGE
from localstub import HTTPResponse

pytestmark = pytest.mark.asyncio

USAGE: Final = "\n" + MAIN_USAGE



models = fake_models_fixture("zjson", "zjson11", "zquery", "zglobal", "zrestjson")


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
async def test_a_code_naming_no_error_shape_has_no_details(models: ApiSession) -> None:
    # `Conflict` is no error code of the model (its shape's is
    # ConflictError), so only Code and Message are modelled, and the stub
    # serializes only those (awswire's json error body).
    run = await models.run("zjson describe", replies=CONFLICT, expected_rc=254)
    retries = retries_clause(models.env, retryable=False)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (Conflict) when calling the Describe "
        f"operation{retries}: in use\n"
    ), run.describe()


def json_error(body: dict) -> HTTPResponse:
    """An error as a json service writes it: the code in `__type`, the
    modelled members beside the message (`_do_modeled_error_parse`)."""
    return HTTPResponse(
        status=400,
        headers={"Content-Type": "application/x-amz-json-1.0"},
        body=json.dumps(body).encode(),
    )


async def test_an_error_s_modelled_members_follow_as_details(models: ApiSession) -> None:
    # The members of the shape the error code names, ConflictError's here:
    # a simple value as itself.
    reply = json_error(
        {"__type": "ConflictError", "message": "in use", "Resource": "thing-1", "Retry": 3}
    )
    run = await models.run("zjson describe", replies=reply, expected_rc=254)
    retries = retries_clause(models.env, retryable=False)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (ConflictError) when calling the "
        f"Describe operation{retries}: in use\n\n"
        "Additional error details:\nResource: thing-1\nRetry: 3\n"
    ), run.describe()


async def test_details_inline_a_small_collection_and_hide_a_complex_one(
    models: ApiSession,
) -> None:
    reply = json_error(
        {
            "__type": "RejectedError",
            "message": "no",
            "Reason": "quota",
            "Codes": ["a", "b"],
            "Detail": {"Label": "x", "Inner": {"Value": "y"}},
            "Unmodelled": "hidden",
        }
    )
    run = await models.run("zjson describe", replies=reply, expected_rc=254)
    retries = retries_clause(models.env, retryable=False)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (RejectedError) when calling the "
        f"Describe operation{retries}: no\n\n"
        "Additional error details:\nReason: quota\nCodes: [a, b]\n"
        "Detail: <complex value>\n"
        'Use "--cli-error-format json" or another error format to see the full '
        "details.\n"
    ), run.describe()


async def test_a_server_error_without_a_body_names_its_status(models: ApiSession) -> None:
    # A 503 is retried: one reply for each attempt.
    unavailable = HTTPResponse(status=503, headers={}, body=b"")
    run = await models.run(
        "zjson ping",
        replies=[unavailable] * attempt_limit(models.env),
        expected_rc=254,
    )
    retries = retries_clause(models.env, retryable=True)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (503) when calling the Ping "
        f"operation{retries}: Service Unavailable\n"
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


# Ping has no output: botocore's query parser still reads the body as
# XML, so an empty one fails on either route; GetThing has output.
@pytest.mark.native_only
@pytest.mark.parametrize("service", ["zjson", "zjson11", "zquery", "zrestjson"])
async def test_the_native_route_sends_each_protocol_s_request(
    models: ApiSession, service: str
) -> None:
    run = await models.run([service, "get-thing", "--thing-id", "t"], replies={})
    tokens = (run.calls[0].request.headers.get("user-agent") or "").split()
    assert any(token.startswith("lang/rust#") for token in tokens), tokens
    assert not any(token.startswith("md/fallback#") for token in tokens), tokens


# A zone with daylight-saving time, whose offset dateutil's tzlocal picks
# per instant: only Python prints an epoch in it.
DST_ZONE: Final = "America/New_York"
STAMPS: Final = {
    "WhenIso": "2020-06-01T12:00:00Z",
    "WhenHttp": "Mon, 01 Jun 2020 12:00:00 GMT",
}
NOON_UTC: Final = "2020-06-01T12:00:00+00:00"


@pytest.mark.parametrize("service", ["zjson", "zquery"])
async def test_text_timestamps_print_in_any_zone(models: ApiSession, service: str) -> None:
    # ISO 8601 and RFC 822 text carries its own offset, so a zone with
    # daylight-saving time changes nothing, and the native route runs it.
    models.env["TZ"] = DST_ZONE
    run = await models.run(f"{service} stamp", replies=STAMPS)
    printed = json.loads(run.stdout)
    assert {key: printed[key] for key in STAMPS} == {
        "WhenIso": NOON_UTC,
        "WhenHttp": NOON_UTC,
    }, run.describe()


async def test_an_epoch_prints_in_a_fixed_zone(models: ApiSession) -> None:
    run = await models.run("zjson describe", replies={"When": 1591012800})
    assert json.loads(run.stdout) == {"When": NOON_UTC}, run.describe()


@pytest.mark.skipif(FORCED_NATIVE, reason="forced native refuses it (local_zone), as checked below")
async def test_an_epoch_prints_in_a_zone_with_daylight_saving(models: ApiSession) -> None:
    models.env["TZ"] = DST_ZONE
    run = await models.run("zjson describe", replies={"When": 1591012800})
    assert json.loads(run.stdout) == {"When": "2020-06-01T08:00:00-04:00"}, run.describe()


@pytest.mark.native_only
async def test_an_epoch_in_a_zone_with_daylight_saving_is_python_s(
    models: ApiSession,
) -> None:
    models.env.update(TZ=DST_ZONE, AWS_CLI_NATIVE_ROUTER="native")
    run = await models.run("zjson describe", replies=[], expected_rc=255)
    assert run.calls == ()
    assert "(local_zone)" in run.stderr, run.describe()


async def test_a_nested_timestamp_format_is_rejected(models: ApiSession) -> None:
    # add_timestamp_parser gets the block's dict and rejects it for every
    # command, before any request.
    models.config_path.write_text(
        models.config_path.read_text(encoding="utf-8")
        + "cli_timestamp_format =\n  nested = x\n",
        encoding="utf-8",
    )
    expected_rc = 255 if FORCED_NATIVE else 253
    run = await models.run("zjson ping", replies=[], expected_rc=expected_rc)
    assert run.calls == ()
    if FORCED_NATIVE:
        assert "(output_format)" in run.stderr, run.describe()
    else:
        assert run.stderr == (
            "\naws: [ERROR]: An error occurred (Configuration): Unknown "
            "cli_timestamp_format value: {'nested': 'x'}, "
            'valid values are "wire" or "iso8601"\n'
        ), run.describe()


@pytest.mark.parametrize(
    "variables,profile,host",
    [
        ({}, "", "zquery.us-east-1.amazonaws.com"),
        ({"AWS_USE_FIPS_ENDPOINT": "true"}, "", "zquery-fips.us-east-1.amazonaws.com"),
        ({"AWS_USE_DUALSTACK_ENDPOINT": "true"}, "", "zquery.us-east-1.api.aws"),
        (
            {"AWS_USE_FIPS_ENDPOINT": "true", "AWS_USE_DUALSTACK_ENDPOINT": "true"},
            "",
            "zquery-fips.us-east-1.api.aws",
        ),
        ({}, "use_fips_endpoint = true\n", "zquery-fips.us-east-1.amazonaws.com"),
        ({}, "use_dualstack_endpoint = true\n", "zquery.us-east-1.api.aws"),
    ],
    ids=["regional", "fips", "dual-stack", "fips-dual-stack", "fips-profile", "dual-stack-profile"],
)
async def test_the_rule_set_s_variants_name_their_host(
    models: ApiSession, variables: dict[str, str], profile: str, host: str
) -> None:
    models.env.update(variables)
    models.config_path.write_text(
        models.config_path.read_text(encoding="utf-8") + profile, encoding="utf-8"
    )
    run = await models.run("zquery get-thing --thing-id t", replies={"ThingId": "t"})
    assert run.calls[0].request.headers.get("host") == host, run.describe()


async def test_a_malformed_xml_body_is_a_parser_error(models: ApiSession) -> None:
    # ResponseParserError, which is not retried.
    reply = HTTPResponse(
        status=200,
        headers={"Content-Type": "text/xml"},
        body=b"<GetThingResponse><GetThingResult>",
    )
    run = await models.run("zquery get-thing --thing-id t", replies=reply, expected_rc=255)
    assert run.stdout == ""
    assert run.stderr == (
        "\naws: [ERROR]: Unable to parse response (no element found: line 1, "
        "column 34), invalid XML received. Further retries may succeed:\n"
        "b'<GetThingResponse><GetThingResult>'\n"
    ), run.describe()


async def test_a_malformed_json_body_parses_as_empty(models: ApiSession) -> None:
    # botocore's json parser reads a success body it cannot decode as no
    # members, so nothing is printed and the command succeeds.
    reply = HTTPResponse(
        status=200,
        headers={"Content-Type": "application/x-amz-json-1.0"},
        body=b'{"ThingId": ',
    )
    run = await models.run("zjson get-thing --thing-id t", replies=reply)
    assert (run.stdout, run.stderr) == ("", ""), run.describe()


async def test_a_redirect_is_a_client_error(models: ApiSession) -> None:
    run = await models.run(
        "zjson ping",
        replies=HTTPResponse(status=301, headers={}, body=b""),
        expected_rc=254,
    )
    retries = retries_clause(models.env, retryable=False)
    assert run.stdout == ""
    assert run.stderr == (
        f"\naws: [ERROR]: An error occurred (301) when calling the Ping operation{retries}:\n"
    ), run.describe()


def counting_process(models: ApiSession, *, fails: bool = False) -> Path:
    """A credential_process that appends a line to a counter file per run,
    configured as the session's only credentials; one that fails writes
    `boom` to stderr and exits 1."""
    for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"):
        models.env.pop(name, None)
    runs = models.cwd / "runs"
    script = models.cwd / "credentials.sh"
    script.write_text(
        "#!/bin/sh\n"
        f"echo run >> '{runs}'\n"
        + (
            "printf boom >&2\nexit 1\n"
            if fails
            else 'printf \'{"Version": 1, "AccessKeyId": "process_key", '
            '"SecretAccessKey": "process_secret"}\'\n'
        ),
        encoding="utf-8",
    )
    script.chmod(0o755)
    models.config_path.write_text(
        models.config_path.read_text(encoding="utf-8")
        + f"credential_process = {script}\n",
        encoding="utf-8",
    )
    return runs


def runs_of(runs: Path) -> int:
    return len(runs.read_text(encoding="utf-8").splitlines()) if runs.exists() else 0


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_refused_operation_runs_the_credential_process_once(
    models: ApiSession,
) -> None:
    # Every refusal comes before any credential provider runs, so the
    # Python route that takes the call over is the only one to run it; a
    # forced-native run refuses it and runs nothing.
    runs = counting_process(models)
    expected_rc = 255 if FORCED_NATIVE else 0
    run = await models.run("zjson ping-sigv4a", replies={}, expected_rc=expected_rc)
    if FORCED_NATIVE:
        assert "(unsupported_auth)" in run.stderr, run.describe()
        assert runs_of(runs) == 0
    else:
        assert runs_of(runs) == 1, run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_native_operation_runs_the_credential_process_once(
    models: ApiSession,
) -> None:
    runs = counting_process(models)
    run = await models.run("zjson ping", replies={})
    assert runs_of(runs) == 1, run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_paginated_operation_runs_the_credential_process_once(
    models: ApiSession,
) -> None:
    # Every page is signed with the client's credentials, which its chain
    # loaded once.
    runs = counting_process(models)
    pages = [{"Things": ["a"], "NextToken": "t1"}, {"Things": ["b"]}]
    run = await models.run("zjson list-things", replies=pages)
    assert len(run.calls) == 2, run.describe()
    assert runs_of(runs) == 1, run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_starting_token_only_python_reads_runs_the_process_once(
    models: ApiSession,
) -> None:
    # A token whose JSON nests past what the native reader follows is
    # refused before any provider runs, so only Python's chain runs the
    # process; a forced-native run refuses it and runs nothing.
    runs = counting_process(models)
    deep = '{"NextToken": ' + "[" * 1000 + "]" * 1000 + "}"
    token = base64.b64encode(deep.encode()).decode()
    command = ["zjson", "list-things", "--starting-token", token]
    if FORCED_NATIVE:
        run = await models.run(command, expected_rc=255)
        assert "(starting_token)" in run.stderr, run.describe()
        assert runs_of(runs) == 0, run.describe()
        return
    run = await models.run(command, expected_rc=None)
    assert runs_of(runs) == 1, run.describe()


async def test_a_starting_token_holding_a_lone_surrogate_is_sent_as_python_sends_it(
    models: ApiSession,
) -> None:
    # The token's JSON holds a lone surrogate, which the request body's
    # json.dumps writes as its escape.
    token = base64.b64encode(json.dumps({"NextToken": "a\ud800"}).encode()).decode()
    run = await models.run(
        ["zjson", "list-things", "--starting-token", token], replies={"Things": ["x"]}
    )
    assert [call.params for call in run.calls] == [{"NextToken": "a\ud800"}], run.describe()
    assert json.loads(run.stdout) == {"Things": ["x"]}, run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_failing_credential_process_is_reported_before_a_bad_starting_token(
    models: ApiSession,
) -> None:
    # botocore creates the client, walking the chain, before the paginator
    # reads the starting token.
    runs = counting_process(models, fails=True)
    command = ["zjson", "list-things", "--starting-token", "does___not___work"]
    run = await models.run(command, expected_rc=255)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: Error when retrieving credentials from custom-process: boom\n"
    ), run.describe()
    assert runs_of(runs) == 1, run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_failing_credential_process_is_reported_before_invalid_input(
    models: ApiSession,
) -> None:
    # botocore walks the chain as it creates the client, before it
    # validates the input.
    runs = counting_process(models, fails=True)
    run = await models.run(["zjson", "strict", "--name", "", "--size", "0"], expected_rc=255)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: Error when retrieving credentials from custom-process: boom\n"
    ), run.describe()
    assert runs_of(runs) == 1, run.describe()
