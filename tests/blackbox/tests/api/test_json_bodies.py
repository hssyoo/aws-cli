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


class _Chunks:
    """A stream that keeps each `write`, as `json.dump` makes them."""

    def __init__(self) -> None:
        self.chunks: list[str] = []

    def write(self, chunk: str) -> None:
        self.chunks.append(chunk)


def printed_by_python(chunks: list[str]) -> tuple[str, str | None]:
    """What a `surrogateescape` stdout holds after `chunks` were written
    one by one, read back as the harness reads it, and the text of the
    `UnicodeEncodeError` that stopped them: each write is encoded on its
    own, so the bytes before the failing one stay written."""
    written = b""
    for chunk in chunks:
        try:
            written += chunk.encode("utf-8", "surrogateescape")
        except UnicodeEncodeError as error:
            return written.decode("utf-8", "surrogateescape"), str(error)
    return written.decode("utf-8", "surrogateescape"), None


def json_chunks(value: object) -> list[str]:
    """`JSONFormatter`'s writes: `json.dump`'s chunks, then the newline."""
    stream = _Chunks()
    json.dump(value, stream, indent=4, ensure_ascii=False)
    return [*stream.chunks, "\n"]


def general_failure(message: str) -> str:
    """What `GeneralExceptionHandler` prints for an exception's text."""
    return f"\naws: [ERROR]: {message}\n"


def text_json(text: str) -> str:
    """`text` as a JSON string, a lone surrogate written as its escape."""
    return json.dumps(text)


# A lone surrogate in a string a service sends is kept. Printed to stdout,
# which writes with surrogateescape, U+DC80-U+DCFF becomes its byte; any
# other fails as Python's encoder fails, after what was written before it.


@pytest.mark.parametrize("service", COMMANDS)
@pytest.mark.parametrize("char", ["\udc80", "\udcff"])
async def test_an_escapable_lone_surrogate_prints_as_its_byte(
    models: ApiSession, service: str, char: str
) -> None:
    text = f"a{char}b"
    reply = body(service, text_json(text), "[]")
    printed = await run(models, service, reply, rc=0)
    assert printed.stdout == json_output(service, text, []), printed.describe()
    rows = await run(models, service, reply, "--output", "text", rc=0)
    assert rows.stdout == f"{text}\n", rows.describe()


@pytest.mark.parametrize("service", COMMANDS)
@pytest.mark.parametrize("text", ["a\ud800b", "a\udc80\ud800b", "\udfff"])
async def test_a_lone_surrogate_it_cannot_escape_fails_as_python_s_encoder_does(
    models: ApiSession, service: str, text: str
) -> None:
    reply = body(service, text_json(text), "[]")
    _, (first, member), _ = COMMANDS[service]
    expected, error = printed_by_python(json_chunks({first: text, member: []}))
    assert error is not None
    printed = await run(models, service, reply, rc=255)
    assert (printed.stdout, printed.stderr) == (expected, general_failure(error)), (
        printed.describe()
    )
    # The text formatter writes the row of scalars with one write.
    rows_expected, rows_error = printed_by_python([text, "\n"])
    assert rows_error is not None
    rows = await run(models, service, reply, "--output", "text", rc=255)
    assert (rows.stdout, rows.stderr) == (rows_expected, general_failure(rows_error)), (
        rows.describe()
    )


@pytest.mark.parametrize("service", COMMANDS)
async def test_a_lone_surrogate_in_a_map_key_prints_as_its_value_does(
    models: ApiSession, service: str
) -> None:
    doc = {"k\udc80": "v", "K\ud800": "w"}
    reply = body(service, '"t"', json.dumps(doc))
    _, (first, member), _ = COMMANDS[service]
    expected, error = printed_by_python(json_chunks({first: "t", member: doc}))
    assert error is not None
    printed = await run(models, service, reply, rc=255)
    assert (printed.stdout, printed.stderr) == (expected, general_failure(error)), (
        printed.describe()
    )
    escapable = body(service, '"t"', json.dumps({"k\udc80": "v"}))
    shown = await run(models, service, escapable, "--output", "text", rc=0)
    assert shown.stdout == f"t\n{member.upper()}\tv\n", shown.describe()


# What is never printed, or printed to stderr, which writes with
# backslashreplace, takes no part.


@pytest.mark.parametrize("service", COMMANDS)
async def test_a_lone_surrogate_never_printed_takes_no_part(
    models: ApiSession, service: str
) -> None:
    reply = body(service, '"t"', text_json("\ud800"))
    off = await run(models, service, reply, "--output", "off", rc=0)
    assert (off.stdout, off.stderr) == ("", ""), off.describe()
    _, (first, _), content_type = COMMANDS[service]
    dropped = HTTPResponse(
        status=200,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"{first}": "t", "Unmodelled": {text_json("\ud800")}}}'.encode(),
    )
    shown = await run(models, service, dropped, rc=0)
    assert shown.stdout == json.dumps({first: "t"}, indent=4) + "\n", shown.describe()


ERROR_OPERATIONS: Final = {"zjson": "Describe", "zrestjson": "GetDocument"}


@pytest.mark.parametrize("service", COMMANDS)
async def test_a_lone_surrogate_in_an_error_prints_backslash_escaped(
    models: ApiSession, service: str
) -> None:
    _, _, content_type = COMMANDS[service]
    message = text_json("no\ud800 way \udc80")
    reply = HTTPResponse(
        status=400,
        headers={"Content-Type": content_type, "x-amzn-RequestId": "r"},
        body=f'{{"__type": "Unknown", "message": {message}, "Detail": "\\ud800"}}'.encode(),
    )
    printed = await run(models, service, reply, rc=254)
    prefix = (
        "\naws: [ERROR]: An error occurred (Unknown) when calling the "
        f"{ERROR_OPERATIONS[service]} operation"
    )
    assert printed.stderr.startswith(prefix), printed.describe()
    assert printed.stderr.endswith(": no\\ud800 way \\udc80\n"), printed.describe()
    assert printed.stdout == "", printed.describe()


# A paginated call writes text page by page, so the pages before the one
# that fails stay written; JSON output writes the full result.

PAGES: Final = ("zjson list-things", ("Things", "NextToken"), "application/x-amz-json-1.0")


def page(things: list[str], token: str | None) -> HTTPResponse:
    content: dict[str, object] = {"Things": things}
    if token is not None:
        content["NextToken"] = token
    return HTTPResponse(
        status=200,
        headers={"Content-Type": PAGES[2], "x-amzn-RequestId": "r"},
        body=json.dumps(content).encode(),
    )


@pytest.mark.parametrize("char, rc", [("\udc80", 0), ("\ud800", 255)])
async def test_a_lone_surrogate_in_a_later_page_fails_after_the_pages_before_it(
    models: ApiSession, char: str, rc: int
) -> None:
    replies = [page(["a"], "t1"), page([f"b{char}"], None)]
    command = PAGES[0].split()
    rows = await models.run([*command, "--output", "text"], replies=replies, expected_rc=rc)
    rows_expected, rows_error = printed_by_python(["THINGS\ta\n", f"THINGS\tb{char}\n"])
    assert rows.stdout == rows_expected, rows.describe()
    assert rows.stderr == ("" if rows_error is None else general_failure(rows_error)), (
        rows.describe()
    )
    printed = await models.run(command, replies=replies, expected_rc=rc)
    expected, error = printed_by_python(json_chunks({"Things": ["a", f"b{char}"]}))
    assert printed.stdout == expected, printed.describe()
    assert printed.stderr == ("" if error is None else general_failure(error)), (
        printed.describe()
    )


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
@pytest.mark.parametrize("doc", [nested(PYTHON_DEPTH + 100)])
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
@pytest.mark.parametrize("detail", [nested(PYTHON_DEPTH + 100)])
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
