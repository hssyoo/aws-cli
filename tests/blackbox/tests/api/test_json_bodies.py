"""JSON response bodies as `json.loads` reads them, on the json and rest-json
fake models (tests/models, through `AWS_DATA_PATH`): `NaN` and the
infinities print as Python prints them, and what the native client's
reader cannot hold, a lone surrogate or nesting past its limit, fails the
call naming that limit, since a request already sent cannot go to Python.

The tests after the first group are the known differences: where the
native route fails and the Python route exits 0, because the value it
cannot hold is never printed, or nests between the native limit and
Python's own printing limit.
"""

import json
from typing import Final

import pytest
from harness_plugin import PYTHON_ROUTE
from localstub import HTTPResponse

from apihelpers import ApiSession, fake_models_fixture

pytestmark = pytest.mark.asyncio

# The run forcing the Python route answers as Python; every other run
# sends these commands natively, which cannot hand a sent request to Python.
PYTHON: Final = PYTHON_ROUTE

# The deepest nesting the native reader holds (model::pyjson::MAX_DEPTH),
# counting the response's own object; the CLI's Python prints a response
# nested 984 deep and fails on 985.
NATIVE_DEPTH: Final = 900
PYTHON_DEPTH: Final = 984

SURROGATE_FAILURE: Final = "surrogates not allowed"
LIMIT: Final = "native client limit: the response holds "


models = fake_models_fixture("zjson", "zrestjson")


# Each service's command that returns a document member, the member order
# its output shape gives, and the content type of its bodies.
COMMANDS: Final = {
    "zjson": ("zjson describe", ("Text", "Doc"), "application/x-amz-json-1.0"),
    "zrestjson": ("zrestjson get-document --id d", ("Name", "Doc"), "application/json"),
}


def body(service: str, text: str, doc: str, status: int = 200) -> HTTPResponse:
    """A response whose first member is `text` and whose document is `doc`,
    both JSON as written."""
    _, (first, member), content_type = COMMANDS[service]
    return HTTPResponse(
        status=status,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"{first}": {text}, "{member}": {doc}}}'.encode(),
    )


def nested(depth: int) -> str:
    """Arrays nested `depth` deep."""
    return "[" * depth + "]" * depth


def json_output(service: str, first: object, doc: object) -> str:
    """What `JSONFormatter` prints for the response."""
    _, (name, member), _ = COMMANDS[service]
    return json.dumps({name: first, member: doc}, indent=4, ensure_ascii=False) + "\n"


async def run(models: ApiSession, service: str, reply: HTTPResponse, *args: str, rc: int | None):
    command, _, _ = COMMANDS[service]
    return await models.run([*command.split(), *args], replies=reply, expected_rc=rc)


@pytest.mark.parametrize("service", COMMANDS)
async def test_nan_and_the_infinities_print_as_python_prints_them(
    models: ApiSession, service: str
) -> None:
    reply = body(service, '"t"', "[NaN, Infinity, -Infinity, 1e400]")
    printed = await run(models, service, reply, rc=0)
    doc = [float("nan"), float("inf"), float("-inf"), float("inf")]
    assert printed.stdout == json_output(service, "t", doc), printed.describe()
    text = await run(models, service, reply, "--output", "text", rc=0)
    assert text.stdout.splitlines() == ["t", "DOC\tnan", "DOC\tinf", "DOC\t-inf", "DOC\tinf"], (
        text.describe()
    )


@pytest.mark.parametrize("service", COMMANDS)
async def test_a_printed_lone_surrogate_fails_the_call_on_either_route(
    models: ApiSession, service: str
) -> None:
    printed = await run(models, service, body(service, '"a\\ud800b"', "null"), rc=255)
    failure = SURROGATE_FAILURE if PYTHON else LIMIT + "a string with a lone surrogate"
    assert failure in printed.stderr, printed.describe()


@pytest.mark.parametrize("service", COMMANDS)
async def test_nesting_at_the_native_limit_prints_on_either_route(
    models: ApiSession, service: str
) -> None:
    # The response's object and its document nested to the limit.
    deep = NATIVE_DEPTH - 1
    printed = await run(models, service, body(service, '"t"', nested(deep)), rc=0)
    assert printed.stdout == json_output(service, "t", json.loads(nested(deep))), (
        printed.describe()
    )


@pytest.mark.parametrize("service", COMMANDS)
async def test_nesting_past_python_s_printing_limit_fails_on_either_route(
    models: ApiSession, service: str
) -> None:
    printed = await run(models, service, body(service, '"t"', nested(PYTHON_DEPTH)), rc=255)
    failure = "maximum recursion depth exceeded" if PYTHON else LIMIT + "arrays and objects"
    assert failure in printed.stderr, printed.describe()


# The known differences: the native route fails where Python exits 0.


@pytest.mark.parametrize("service", COMMANDS)
async def test_known_difference_nesting_between_the_limits(
    models: ApiSession, service: str
) -> None:
    printed = await run(models, service, body(service, '"t"', nested(NATIVE_DEPTH)), rc=None)
    if PYTHON:
        assert printed.rc == 0, printed.describe()
        assert printed.stdout == json_output(service, "t", json.loads(nested(NATIVE_DEPTH)))
    else:
        assert printed.rc == 255, printed.describe()
        assert LIMIT + "arrays and objects nested more than 900 deep" in printed.stderr


@pytest.mark.parametrize("service", COMMANDS)
@pytest.mark.parametrize(
    "case, doc, args",
    [
        # `--query` is the Python route's on either run (a Python-only
        # global), so a value it leaves out makes no difference.
        ("surrogate-output-off", '"\\ud800"', ("--output", "off")),
        ("deep-output-off", nested(PYTHON_DEPTH + 100), ("--output", "off")),
    ],
)
async def test_known_difference_a_value_never_printed(
    models: ApiSession, service: str, case: str, doc: str, args: tuple[str, ...]
) -> None:
    printed = await run(models, service, body(service, '"t"', doc), *args, rc=None)
    if PYTHON:
        assert printed.rc == 0, printed.describe()
    else:
        assert (printed.rc, printed.stdout) == (255, ""), printed.describe()
        assert LIMIT in printed.stderr, printed.describe()


@pytest.mark.parametrize("service", COMMANDS)
@pytest.mark.parametrize("doc", ['"\\ud800"', nested(PYTHON_DEPTH + 100)])
async def test_known_difference_a_member_the_output_shape_drops(
    models: ApiSession, service: str, doc: str
) -> None:
    _, (first, _), content_type = COMMANDS[service]
    reply = HTTPResponse(
        status=200,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"{first}": "t", "Unmodelled": {doc}}}'.encode(),
    )
    printed = await run(models, service, reply, rc=None)
    if PYTHON:
        assert (printed.rc, printed.stdout) == (0, json.dumps({first: "t"}, indent=4) + "\n"), (
            printed.describe()
        )
    else:
        assert (printed.rc, printed.stdout) == (255, ""), printed.describe()
        assert LIMIT in printed.stderr, printed.describe()


@pytest.mark.parametrize("service", COMMANDS)
@pytest.mark.parametrize("detail", ['"\\ud800"', nested(PYTHON_DEPTH + 100)])
async def test_known_difference_an_error_response_holding_the_value(
    models: ApiSession, service: str, detail: str
) -> None:
    _, _, content_type = COMMANDS[service]
    reply = HTTPResponse(
        status=400,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"__type": "Unknown", "message": "no", "Detail": {detail}}}'.encode(),
    )
    printed = await run(models, service, reply, rc=None)
    if PYTHON:
        assert printed.rc == 254, printed.describe()
    else:
        assert printed.rc == 255, printed.describe()
        assert LIMIT in printed.stderr, printed.describe()


@pytest.mark.parametrize("service", COMMANDS)
async def test_known_difference_invalid_json_nested_past_the_limit(
    models: ApiSession, service: str
) -> None:
    # Python's json.loads fails at the end, so botocore reads the body as
    # {'message': body} and prints nothing of it.
    _, (first, _), content_type = COMMANDS[service]
    reply = HTTPResponse(
        status=200,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"{first}": "t", "Doc": {nested(NATIVE_DEPTH + 10)},}}'.encode(),
    )
    printed = await run(models, service, reply, rc=None)
    if PYTHON:
        assert (printed.rc, printed.stdout) == (0, ""), printed.describe()
    else:
        assert printed.rc == 255, printed.describe()
        assert LIMIT in printed.stderr, printed.describe()
