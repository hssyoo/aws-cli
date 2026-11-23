"""`aws sts get-caller-identity`, the generic service command end to end:
the output, the failures, and the request, as the Python route gives
them. A run forcing the Python route checks these expectations against
Python itself; the default run checks the native route, which the
User-Agent's language token proves the request came from.
"""

import json
from typing import Final

import pytest

from apihelpers import ApiSession, Reply

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
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (AccessDenied) when calling the "
        "GetCallerIdentity operation (reached max retries: 0): User is not "
        "authorized\n"
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
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (AccessDenied) when calling the "
        "GetCallerIdentity operation (reached max retries: 0): denied\n"
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
