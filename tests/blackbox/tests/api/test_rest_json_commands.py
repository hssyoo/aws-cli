"""The generic service command over the repository's fake rest-json model
(tests/models/zrestjson, through `AWS_DATA_PATH`): the request each
operation sends (method, path, query string, headers, body bytes), the
output it prints and the errors it reports, as the Python route gives them.

A run forcing the Python route checks every expectation against Python.
The default run sends a command whose arguments all bind natively (strings,
integers, longs and booleans) down the native route once rest-json is
among the protocols it implements; a list, map or structure argument is
Python's on either run (`Unpack::Deferred`). `test_each_command_s_route`
names the route of every command here, so a native run shows which of
these tests the native route answered. A run forcing the native route
refuses a command with such an argument, which that test checks; the
other tests sending one skip that run.
"""

import json
from collections.abc import AsyncIterator, Mapping
from pathlib import Path
from typing import Any, Final

import pytest
import pytest_asyncio
from harness_plugin import COMMAND_ENVIRONMENT
from localstub import HTTPResponse

from apihelpers import (
    ApiSession,
    Call,
    CommandRun,
    api_session,
    attempt_limit,
    fake_model_protocol,
    retries_clause,
)

pytestmark = pytest.mark.asyncio

# The run forces the native route, which refuses what it cannot run.
FORCED_NATIVE: Final = COMMAND_ENVIRONMENT.get("AWS_CLI_NATIVE_ROUTER") == "native"
COLLECTIONS_REFUSED: Final = (
    "forced native refuses a collection argument (unsupported_option), "
    "as test_each_command_s_route checks"
)

MODELS: Final = Path(__file__).resolve().parents[3] / "models"
HOST: Final = "zrestjson.us-east-1.amazonaws.com"
# The headers whose values change with each request; the Authorization
# header's SignedHeaders are compared on their own.
VOLATILE: Final = frozenset(
    {"user-agent", "x-amz-date", "authorization", "amz-sdk-invocation-id"}
)


@pytest_asyncio.fixture
async def restjson(aws_cli: str, tmp_path: Path) -> AsyncIterator[ApiSession]:
    async with api_session(aws_cli, tmp_path) as session:
        session.env["AWS_DATA_PATH"] = str(MODELS)
        # A zone without daylight-saving time, which iso8601 output of an
        # epoch needs on the native route.
        session.env["TZ"] = "UTC"
        session.protocol = fake_model_protocol(MODELS, ("zrestjson",))
        yield session


def headers(session: ApiSession, **modelled: str) -> dict[str, str]:
    """The headers every request sends, with `modelled` added (`_` in a
    keyword spells `-`)."""
    return {
        "host": HOST,
        "accept-encoding": "identity",
        "amz-sdk-request": f"attempt=1; max={attempt_limit(session.env)}",
        **{name.replace("_", "-"): value for name, value in modelled.items()},
    }


def sent(call: Call) -> tuple[str, str, dict[str, str], bytes]:
    """The request as the wire carried it: method, path with query string,
    the headers but the volatile ones (names lowercased), and the body."""
    request = call.request
    kept = {
        name.lower(): value
        for name, value in request.headers.items()
        if name.lower() not in VOLATILE
    }
    return request.method, request.effective_path, kept, request.body or b""


def signed_headers(call: Call) -> list[str]:
    """The SignedHeaders list of the request's SigV4 Authorization header."""
    authorization = call.request.headers.get("authorization") or ""
    part = next(
        piece
        for piece in authorization.split(", ")
        if piece.startswith("SignedHeaders=")
    )
    return part.removeprefix("SignedHeaders=").split(";")


def only_call(run: CommandRun) -> Call:
    (call,) = run.calls
    return call


def printed(value: Mapping[str, Any]) -> str:
    """The JSON output of `value`, as the CLI indents it."""
    return json.dumps(value, indent=4) + "\n"


def reply(
    status: int, body: bytes = b"", **header_values: str
) -> HTTPResponse:
    """A response as given (`_` in a header keyword spells `-`)."""
    return HTTPResponse(
        status=status,
        headers={
            name.replace("_", "-"): value
            for name, value in header_values.items()
        },
        body=body,
    )


# Labels, query string and headers.


async def test_labels_query_and_headers_are_placed_by_location(
    restjson: ApiSession,
) -> None:
    # A plain label encodes its `/` and a greedy one keeps it
    # (`_render_uri_template`); the query string follows the model's
    # member order, percent-encoded (`percent_encode_sequence`), with a
    # boolean as `true`; header members are text.
    run = await restjson.run(
        [
            "zrestjson", "get-item",
            "--trace", "t1",
            "--verbose",
            "--limit", "5",
            "--filter", "a b&c=d/é",
            "--key", "x/y z~.txt",
            "--id", "a b/c",
            "--depth", "3",
        ],
        replies=reply(200, b"{}"),
    )
    call = only_call(run)
    assert sent(call) == (
        "GET",
        "/items/a%20b%2Fc/x/y%20z~.txt"
        "?filter=a%20b%26c%3Dd%2F%C3%A9&limit=5&verbose=true",
        headers(restjson, x_zrj_trace="t1", x_zrj_depth="3"),
        b"",
    ), run.describe()
    assert signed_headers(call) == [
        "host", "x-amz-date", "x-zrj-depth", "x-zrj-trace",
    ]
    assert call.params == {
        "Id": "a b/c",
        "Key": "x/y z~.txt",
        "Filter": "a b&c=d/é",
        "Limit": 5,
        "Verbose": True,
        "Trace": "t1",
        "Depth": 3,
    }


async def test_a_false_query_boolean_is_sent(restjson: ApiSession) -> None:
    run = await restjson.run(
        "zrestjson get-item --id i --key k --no-verbose",
        replies=reply(200, b"{}"),
    )
    method, path, *_ = sent(only_call(run))
    assert (method, path) == ("GET", "/items/i/k?verbose=false"), run.describe()


@pytest.mark.skipif(FORCED_NATIVE, reason=COLLECTIONS_REFUSED)
async def test_lists_and_a_header_map_are_spread(restjson: ApiSession) -> None:
    # A query list repeats its key; a header list joins its items with
    # commas, quoting one that holds a comma or a quote
    # (`_escape_header_list_string`); a `headers` map prefixes each key,
    # whose case it keeps, and every one is signed. Python binds these
    # arguments on either route.
    run = await restjson.run(
        [
            "zrestjson", "get-item", "--id", "i", "--key", "k",
            "--tags", "b", "a",
            "--accepts", "x", 'y,"z"',
            "--meta", "Color=red,size=L",
        ],
        replies=reply(200, b"{}"),
    )
    call = only_call(run)
    assert sent(call) == (
        "GET",
        "/items/i/k?tag=b&tag=a",
        {
            **headers(restjson, x_zrj_accept='x,"y,\\"z\\""'),
            "x-zrj-meta-color": "red",
            "x-zrj-meta-size": "L",
        },
        b"",
    ), run.describe()
    meta = [
        name for name, _ in call.request.headers.items()
        if name.lower().startswith("x-zrj-meta-")
    ]
    assert meta == ["x-zrj-meta-Color", "x-zrj-meta-size"]
    assert signed_headers(call) == [
        "host", "x-amz-date", "x-zrj-accept", "x-zrj-meta-color",
        "x-zrj-meta-size",
    ]


async def test_a_literal_query_in_the_uri_comes_first(
    restjson: ApiSession,
) -> None:
    # The members' query string joins the URI's own with `&`
    # (`prepare_request_dict`).
    run = await restjson.run(
        "zrestjson list-versions --id i1 --limit 2",
        replies=reply(200, b'{"Versions": ["v1", "v2"]}'),
    )
    assert sent(only_call(run)) == (
        "GET", "/items/i1?versions&limit=2", headers(restjson), b"",
    ), run.describe()
    assert run.stdout == printed({"Versions": ["v1", "v2"]})


# Bodies.


async def test_a_json_body_holds_the_members_with_no_location(
    restjson: ApiSession,
) -> None:
    # `json.dumps` with its default separators, in the model's member
    # order, under each member's locationName.
    run = await restjson.run(
        "zrestjson put-item --enabled --count 3 --name n --mode fast --id i1",
        replies=reply(201, b'{"Id": "i1"}', Location="/items/i1"),
    )
    call = only_call(run)
    body = b'{"name": "n", "Count": 3, "Enabled": true}'
    assert sent(call) == (
        "POST",
        "/items/i1",
        headers(
            restjson,
            x_zrj_mode="fast",
            content_type="application/json",
            content_length=str(len(body)),
        ),
        body,
    ), run.describe()
    assert signed_headers(call) == [
        "content-type", "host", "x-amz-date", "x-zrj-mode",
    ]
    assert run.stdout == printed(
        {"Location": "/items/i1", "Status": 201, "Id": "i1"}
    ), run.describe()


async def test_a_body_with_no_member_given_is_an_empty_object(
    restjson: ApiSession,
) -> None:
    # The input has members with no location, so its body is `{}`
    # (`_requires_empty_body`), which takes the JSON Content-Type.
    run = await restjson.run(
        "zrestjson put-item --id i1", replies=reply(201)
    )
    assert sent(only_call(run)) == (
        "POST",
        "/items/i1",
        headers(
            restjson, content_type="application/json", content_length="2"
        ),
        b"{}",
    ), run.describe()
    assert run.stdout == printed({"Status": 201}), run.describe()


@pytest.mark.skipif(FORCED_NATIVE, reason=COLLECTIONS_REFUSED)
async def test_collections_in_the_body_keep_their_order(
    restjson: ApiSession,
) -> None:
    # Python binds these arguments on either route.
    run = await restjson.run(
        [
            "zrestjson", "put-item", "--id", "i1",
            "--config", "Label=l,Depth=2",
            "--attributes", "b=2,a=1",
            "--labels", "x", "y",
            "--no-enabled",
        ],
        replies=reply(201),
    )
    _, _, _, body = sent(only_call(run))
    assert body == (
        b'{"Enabled": false, "Labels": ["x", "y"], '
        b'"Attributes": {"b": "2", "a": "1"}, '
        b'"Config": {"Label": "l", "Depth": 2}}'
    ), run.describe()


async def test_an_operation_without_input_sends_no_body(
    restjson: ApiSession,
) -> None:
    run = await restjson.run(
        "zrestjson ping",
        replies=reply(200, b'{"Healthy": true}', x_zrj_region="r"),
    )
    call = only_call(run)
    assert sent(call) == ("GET", "/ping", headers(restjson), b""), run.describe()
    assert signed_headers(call) == ["host", "x-amz-date"]
    assert run.stdout == printed({"Region": "r", "Healthy": True})


@pytest.mark.parametrize("force", ["true", "false"])
async def test_a_delete_sends_an_empty_body_and_prints_nothing(
    restjson: ApiSession, force: str
) -> None:
    # A method that may carry a body states its empty one; GET does not.
    flag = "--force" if force == "true" else "--no-force"
    run = await restjson.run(
        ["zrestjson", "delete-item", "--id", "i1", flag], replies=reply(204)
    )
    assert sent(only_call(run)) == (
        "DELETE",
        f"/items/i1?force={force}",
        headers(restjson, content_length="0"),
        b"",
    ), run.describe()
    assert (run.stdout, run.stderr) == ("", "")


# Output.

ITEM_HEADERS: Final = {
    "ETag": '"e1"',
    "x-zrj-revision": "7",
    "x-zrj-fresh": "True",
    "Last-Modified": "Mon, 01 Jun 2020 12:00:00 GMT",
    "x-zrj-meta-Color": "red",
    "X-Zrj-Meta-size": "L",
    "Content-Type": "application/json",
}
ITEM_BODY: Final = {
    "Name": "n",
    "Size": 12345678901,
    "Labels": ["a"],
    "Created": 1591012800,
}
NOON_UTC: Final = "2020-06-01T12:00:00+00:00"


async def test_output_members_come_from_headers_status_and_body(
    restjson: ApiSession,
) -> None:
    # The located members first, in model order (`_parse_non_payload_attrs`),
    # then the body's: a header integer through `int`, a header boolean
    # through `ensure_boolean`, a header timestamp as RFC 822, the status
    # code, and a `headers` map keyed by what follows its prefix, as the
    # response spells it.
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=HTTPResponse(
            status=200,
            headers=ITEM_HEADERS,
            body=json.dumps(ITEM_BODY).encode(),
        ),
    )
    assert run.stdout == printed(
        {
            "ETag": '"e1"',
            "Revision": 7,
            "Fresh": True,
            "LastModified": NOON_UTC,
            "Status": 200,
            "Meta": {"Color": "red", "size": "L"},
            "Name": "n",
            "Size": 12345678901,
            "Labels": ["a"],
            "Created": NOON_UTC,
        }
    ), run.describe()
    run = await restjson.run(
        "zrestjson get-item --id i --key k --output text",
        replies=HTTPResponse(
            status=200,
            headers=ITEM_HEADERS,
            body=json.dumps(ITEM_BODY).encode(),
        ),
    )
    assert run.stdout == (
        f'{NOON_UTC}\t"e1"\tTrue\t{NOON_UTC}\tn\t7\t12345678901\t200\n'
        "LABELS\ta\nMETA\tred\tL\n"
    ), run.describe()


async def test_a_body_key_overrides_the_located_member_of_its_name(
    restjson: ApiSession,
) -> None:
    # The body's structure is parsed with every member, located or not
    # (`BaseJSONParser._handle_structure`), and updates the located ones in
    # place.
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=reply(
            200, b'{"Name": "n", "ETag": "body", "Status": 299}', ETag="header"
        ),
    )
    assert run.stdout == printed(
        {"ETag": "body", "Status": 299, "Meta": {}, "Name": "n"}
    ), run.describe()


@pytest.mark.parametrize(
    "body", [b"", b'{"Name": '], ids=["empty", "malformed"]
)
async def test_a_body_without_members_leaves_the_located_ones(
    restjson: ApiSession, body: bytes
) -> None:
    # A body that is no JSON parses as `{"message": ...}`
    # (`_parse_body_as_json`), which names no output member; the `headers`
    # map is there even when no header has its prefix.
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=reply(200, body, ETag="e"),
    )
    assert (run.stdout, run.stderr) == (
        printed({"ETag": "e", "Status": 200, "Meta": {}}),
        "",
    ), run.describe()


# Errors.


def failed(session: ApiSession, code: str, message: str) -> str:
    """The error line of GetItem's `code`; an empty `message` leaves the
    line ending at its colon."""
    retries = retries_clause(session.env, retryable=False)
    return (
        f"\naws: [ERROR]: An error occurred ({code}) when calling the GetItem "
        f"operation{retries}:{f' {message}' if message else ''}\n"
    )


async def test_the_error_type_header_names_the_code(
    restjson: ApiSession,
) -> None:
    # `x-amzn-errortype` wins over the body's code and loses what follows
    # its `:` (`_inject_error_code`); the body's `message` is the message,
    # and the error shape's members follow as details.
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=reply(
            404,
            b'{"code": "ItemLocked", "message": "gone", "Id": "i"}',
            x_amzn_errortype="ItemNotFound:http://internal.example/errors/",
        ),
        expected_rc=254,
    )
    assert run.stdout == ""
    assert run.stderr == failed(restjson, "ItemNotFound", "gone") + (
        "\nAdditional error details:\nId: i\n"
    ), run.describe()


async def test_an_error_s_header_member_is_a_detail(
    restjson: ApiSession,
) -> None:
    # Without the header, the body's `code` names the error, over its
    # `__type`; the error shape's header member is parsed from the
    # response's headers (`_do_modeled_error_parse`).
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=reply(
            409,
            b'{"__type": "ItemNotFound", "code": "ItemLocked", '
            b'"Message": "locked", "Attempts": 2}',
            x_zrj_owner="alice",
        ),
        expected_rc=254,
    )
    assert run.stderr == failed(restjson, "ItemLocked", "locked") + (
        "\nAdditional error details:\nOwner: alice\nAttempts: 2\n"
    ), run.describe()


@pytest.mark.parametrize(
    "body,code,message",
    [
        # A `__type` loses its namespace (`BaseJSONParser._do_error_parse`).
        (
            b'{"__type": "com.example#ItemNotFound", "message": "ns"}',
            "ItemNotFound",
            "ns",
        ),
        # With no code at all the status names the error.
        (b"", "400", ""),
        # A body that is no JSON is the message.
        (b"not json", "400", "not json"),
    ],
    ids=["namespaced-type", "empty", "not-json"],
)
async def test_an_error_without_a_type_header(
    restjson: ApiSession, body: bytes, code: str, message: str
) -> None:
    run = await restjson.run(
        "zrestjson get-item --id i --key k",
        replies=reply(400, body),
        expected_rc=254,
    )
    assert run.stderr == failed(restjson, code, message), run.describe()


# Routes.


# Each command above, and the route the default run gives it once rest-json
# is implemented: native for one whose arguments are scalars, and Python's,
# naming the deferred option, for one with a list, map or structure.
ROUTES: Final = {
    "get-item-scalars": (
        "get-item --id a --key b/c --filter f --limit 1 --verbose --trace t",
        None,
    ),
    "get-item-no-verbose": ("get-item --id i --key k --no-verbose", None),
    "get-item-collections": (
        "get-item --id i --key k --tags a --meta k=v",
        "unsupported_option",
    ),
    "list-versions": ("list-versions --id i --limit 2", None),
    "put-item-scalars": ("put-item --id i --name n --count 3 --enabled", None),
    "put-item-empty": ("put-item --id i", None),
    "put-item-collections": (
        "put-item --id i --labels x y",
        "unsupported_option",
    ),
    "ping": ("ping", None),
    "delete-item": ("delete-item --id i --force", None),
}


@pytest.mark.native_only
@pytest.mark.parametrize("case", ROUTES)
async def test_each_command_s_route(restjson: ApiSession, case: str) -> None:
    command, fallback = ROUTES[case]
    if FORCED_NATIVE and fallback is not None:
        run = await restjson.run(f"zrestjson {command}", expected_rc=None)
        refusal = (
            "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable "
            f"({fallback}); Python fallback disabled\n"
        )
        assert (run.rc, run.stderr, run.calls) == (255, refusal, ()), run.describe()
        return
    run = await restjson.run(f"zrestjson {command}", replies=reply(200, b"{}"))
    user_agent = only_call(run).request.headers.get("user-agent") or ""
    tokens = user_agent.split()
    # Whether the native route sent it, and the reason the Python route
    # names.
    route = (
        any(token.startswith("lang/rust#") for token in tokens),
        [token for token in tokens if token.startswith("md/fallback#")],
    )
    if fallback is None:
        assert route == (True, []), user_agent
    else:
        assert route == (False, [f"md/fallback#{fallback}"]), user_agent
