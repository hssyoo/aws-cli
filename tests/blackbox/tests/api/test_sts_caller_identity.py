"""`aws sts get-caller-identity`, the generic service command end to end:
the output, the failures, and the request, as the Python route gives
them. A run forcing the Python route checks these expectations against
Python itself; the default run checks the native route, which the
User-Agent's language token proves the request came from.
"""

import json
import os
from pathlib import Path
from typing import Final

import pytest

from apihelpers import ApiSession, Reply, retries_clause

pytestmark = pytest.mark.asyncio

COMMAND: Final = ["sts", "get-caller-identity"]
IDENTITY: Final = {
    "UserId": "AIDACKCEVSQ6C2EXAMPLE",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/tester",
}
BODY: Final = b"Action=GetCallerIdentity&Version=2011-06-15"


def signed_headers(authorization: str) -> set[str]:
    """The SignedHeaders list of a SigV4 Authorization header."""
    part = next(
        piece for piece in authorization.split(", ") if piece.startswith("SignedHeaders=")
    )
    return set(part.removeprefix("SignedHeaders=").split(";"))


async def test_json_output_is_the_response_indented(api: ApiSession) -> None:
    run = await api.run(COMMAND, replies=IDENTITY)
    assert run.stdout == json.dumps(IDENTITY, indent=4) + "\n", run.describe()
    assert run.stderr == ""
    assert run.operations == ["GetCallerIdentity"]


async def test_text_output_is_the_values_in_key_order(api: ApiSession) -> None:
    run = await api.run([*COMMAND, "--output", "text"], replies=IDENTITY)
    assert run.stdout == (
        "123456789012\tarn:aws:iam::123456789012:user/tester\tAIDACKCEVSQ6C2EXAMPLE\n"
    ), run.describe()


async def test_the_request_is_a_signed_query_post(api: ApiSession) -> None:
    run = await api.run(COMMAND, replies=IDENTITY)
    (call,) = run.calls
    request = call.request
    assert (request.method, request.target) == ("POST", "/")
    assert request.body == BODY
    assert request.headers.get("content-type") == (
        "application/x-www-form-urlencoded; charset=utf-8"
    )
    authorization = request.headers.get("authorization") or ""
    assert authorization.startswith(
        "AWS4-HMAC-SHA256 Credential=access_key/"
    ), authorization
    assert "/us-east-1/sts/aws4_request" in authorization
    assert signed_headers(authorization) == {"content-type", "host", "x-amz-date"}


async def test_a_session_token_is_sent_and_signed(api: ApiSession) -> None:
    api.env["AWS_SESSION_TOKEN"] = "session-token"
    run = await api.run(COMMAND, replies=IDENTITY)
    (call,) = run.calls
    assert call.request.headers.get("x-amz-security-token") == "session-token"
    authorization = call.request.headers.get("authorization") or ""
    assert "x-amz-security-token" in signed_headers(authorization)


async def test_no_sign_request_sends_no_signature(api: ApiSession) -> None:
    run = await api.run([*COMMAND, "--no-sign-request"], replies=IDENTITY)
    (call,) = run.calls
    assert call.request.headers.get("authorization") is None
    assert json.loads(run.stdout) == IDENTITY


@pytest.mark.native_only
async def test_the_native_route_sends_the_request(api: ApiSession) -> None:
    run = await api.run(COMMAND, replies=IDENTITY)
    (call,) = run.calls
    user_agent = call.request.headers.get("user-agent") or ""
    tokens = user_agent.split()
    assert any(token.startswith("lang/rust#") for token in tokens), user_agent
    assert not any(token.startswith("md/fallback#") for token in tokens)


async def test_a_service_error_names_its_code_and_retries(api: ApiSession) -> None:
    error = Reply(
        {
            "Error": {
                "Type": "Sender",
                "Code": "AccessDenied",
                "Message": "User is not authorized",
            }
        },
        status=403,
    )
    run = await api.run(COMMAND, replies=error, expected_rc=254)
    assert run.stdout == ""
    # `Type` is part of every query error, so no details block follows.
    retries = retries_clause(api.env, retryable=False)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (AccessDenied) when calling the "
        f"GetCallerIdentity operation{retries}: User is not authorized\n"
    ), run.describe()


async def test_an_unmodelled_error_field_is_left_out(api: ApiSession) -> None:
    # ClientErrorHandler's details name the error shape's modelled members
    # only, and sts's errors model none beyond the message.
    error = Reply(
        {
            "Error": {
                "Type": "Sender",
                "Code": "AccessDenied",
                "Message": "denied",
                "Reason": "policy",
            }
        },
        status=403,
    )
    run = await api.run(COMMAND, replies=error, expected_rc=254)
    retries = retries_clause(api.env, retryable=False)
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (AccessDenied) when calling the "
        f"GetCallerIdentity operation{retries}: denied\n"
    ), run.describe()


async def test_without_credentials_nothing_is_sent(api: ApiSession) -> None:
    for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"):
        del api.env[name]
    run = await api.run(COMMAND, replies=[], expected_rc=253)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (NoCredentials): Unable to locate "
        'credentials. You can configure credentials by running "aws login".\n'
    ), run.describe()


async def test_retries_reuse_the_invocation_id(api: ApiSession) -> None:
    del api.env["AWS_MAX_ATTEMPTS"]
    failure = Reply({"Error": {"Code": "InternalError", "Message": "x"}}, status=500)
    run = await api.run(COMMAND, replies=[failure, IDENTITY])
    first, second = run.calls
    ids = [call.request.headers.get("amz-sdk-invocation-id") for call in (first, second)]
    assert ids[0] is not None and ids[0] == ids[1]
    # The limit is the default's, or what the run's --env imposes.
    attempts = [call.request.headers.get("amz-sdk-request") or "" for call in (first, second)]
    limits = {attempt.partition("; ")[2] for attempt in attempts}
    assert [attempt.partition("; ")[0] for attempt in attempts] == ["attempt=1", "attempt=2"]
    assert len(limits) == 1 and limits <= {"max=3", "max=2"}, attempts
    assert json.loads(run.stdout) == IDENTITY


def host_of(run) -> str:
    (call,) = run.calls
    return call.request.headers.get("host") or ""


@pytest.mark.parametrize(
    "variables,arguments,host",
    [
        ({}, [], "sts.us-east-1.amazonaws.com"),
        ({}, ["--region", "eu-west-1"], "sts.eu-west-1.amazonaws.com"),
        ({"AWS_USE_FIPS_ENDPOINT": "true"}, [], "sts-fips.us-east-1.amazonaws.com"),
        ({}, ["--region", "fips-us-east-1"], "sts-fips.us-east-1.amazonaws.com"),
        ({"AWS_USE_DUALSTACK_ENDPOINT": "true"}, [], "sts.us-east-1.api.aws"),
        ({}, ["--endpoint-url", "https://sts.example.test"], "sts.example.test"),
        ({"AWS_ENDPOINT_URL_STS": "https://sts.configured.test"}, [], "sts.configured.test"),
    ],
    ids=[
        "default-region",
        "region-flag",
        "fips-setting",
        "fips-pseudo-region",
        "dual-stack",
        "endpoint-url",
        "service-endpoint-variable",
    ],
)
async def test_the_endpoint_follows_the_settings(
    api: ApiSession,
    variables: dict[str, str],
    arguments: list[str],
    host: str,
) -> None:
    api.env.update(variables)
    run = await api.run([*COMMAND, *arguments], replies=IDENTITY)
    assert host_of(run) == host, run.describe()


async def test_a_profile_names_the_region(api: ApiSession) -> None:
    del api.env["AWS_DEFAULT_REGION"]
    api.config_path.write_text(
        api.config_path.read_text(encoding="utf-8")
        # An explicit profile skips the environment's keys
        # (`ProfileProviderBuilder`), so it carries its own.
        + "[profile west]\nregion = us-west-2\n"
        "aws_access_key_id = profile_key\naws_secret_access_key = profile_secret\n",
        encoding="utf-8",
    )
    run = await api.run([*COMMAND, "--profile", "west"], replies=IDENTITY)
    assert host_of(run) == "sts.us-west-2.amazonaws.com", run.describe()


async def test_the_timeout_options_run_the_command(api: ApiSession) -> None:
    run = await api.run(
        [*COMMAND, "--cli-read-timeout", "30", "--cli-connect-timeout", "5"],
        replies=IDENTITY,
    )
    assert json.loads(run.stdout) == IDENTITY


async def test_a_refused_connection_names_the_endpoint(api: ApiSession) -> None:
    # A closed loopback port, with the proxy bypassed for it.
    api.env["NO_PROXY"] = "127.0.0.1"
    run = await api.run(
        [*COMMAND, "--endpoint-url", "http://127.0.0.1:9"],
        replies=[],
        expected_rc=255,
    )
    assert run.calls == ()
    assert run.stderr == (
        '\naws: [ERROR]: Could not connect to the endpoint URL: "http://127.0.0.1:9/"\n'
    ), run.describe()


def signing_key(run) -> str:
    """The access key id the request's SigV4 credential scope names."""
    (call,) = run.calls
    authorization = call.request.headers.get("authorization") or ""
    return authorization.split("Credential=", 1)[1].split("/", 1)[0]


def without_environment_keys(api: ApiSession) -> None:
    for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"):
        del api.env[name]


async def test_environment_keys_sign(api: ApiSession) -> None:
    run = await api.run(COMMAND, replies=IDENTITY)
    assert signing_key(run) == "access_key"


async def test_the_shared_credentials_file_signs(api: ApiSession) -> None:
    without_environment_keys(api)
    Path(api.env["AWS_SHARED_CREDENTIALS_FILE"]).write_text(
        "[default]\naws_access_key_id = file_key\naws_secret_access_key = file_secret\n",
        encoding="utf-8",
    )
    run = await api.run(COMMAND, replies=IDENTITY)
    assert signing_key(run) == "file_key", run.describe()


async def test_the_config_file_s_keys_sign(api: ApiSession) -> None:
    without_environment_keys(api)
    api.config_path.write_text(
        api.config_path.read_text(encoding="utf-8")
        + "aws_access_key_id = config_key\naws_secret_access_key = config_secret\n",
        encoding="utf-8",
    )
    run = await api.run(COMMAND, replies=IDENTITY)
    assert signing_key(run) == "config_key", run.describe()


@pytest.mark.skipif(os.name == "nt", reason="the process is a POSIX shell command")
async def test_a_credential_process_signs(api: ApiSession) -> None:
    without_environment_keys(api)
    script = api.cwd / "credentials.sh"
    script.write_text(
        "#!/bin/sh\n"
        'printf \'{"Version": 1, "AccessKeyId": "process_key", '
        '"SecretAccessKey": "process_secret", "SessionToken": "process_token"}\'\n',
        encoding="utf-8",
    )
    script.chmod(0o755)
    api.config_path.write_text(
        api.config_path.read_text(encoding="utf-8")
        + f"credential_process = {script}\n",
        encoding="utf-8",
    )
    run = await api.run(COMMAND, replies=IDENTITY)
    assert signing_key(run) == "process_key", run.describe()
    (call,) = run.calls
    assert call.request.headers.get("x-amz-security-token") == "process_token"
