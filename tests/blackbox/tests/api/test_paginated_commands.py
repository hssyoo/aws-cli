"""Paginated operations of the repository's fake models (tests/models,
through `AWS_DATA_PATH`): the requests every page sends and what each
output format prints of them, as the Python route gives them
(`CLIOperationCaller._make_client_call`, botocore's `PageIterator`, and
awscli's `paginate` customization and formatters).

Each paginator follows a real service's:

- zjson ListThings: NextToken both ways, MaxResults the limit;
- zjson ListKeys (kms ListKeys): Marker in, NextMarker out, and
  `more_results` (Truncated);
- zjson FilterEvents (logs FilterLogEvents): two result keys;
- zquery ListThings (sns ListTopics): no limit key;
- zquery DescribeWidgets (rds DescribeDBInstances): a Marker both ways;
- zquery DescribeDefaults (rds DescribeEngineDefaultParameters): a dotted
  output token and result key;
- zquery ListJobs (importexport ListJobs): an expression output token
  (`Jobs[-1].JobId`), which the native route leaves to Python;
- zrestjson ListFunctions (lambda ListFunctions): token and limit in the
  query string, the limit being the operation's own MaxItems, which
  `--max-items` shadows;
- zrestjson SearchUsers (connect SearchUsers): token and limit in the
  body, and a non-aggregate key.

A run forcing the Python route checks every expectation against Python.
The default run sends these commands down the native route once it
paginates, all but ListJobs: `test_each_paginated_command_s_route` names
each one's route, and checks the refusal of ListJobs on a run forcing the
native route, which the other tests of ListJobs skip.
"""

import base64
import json
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any, Final

import pytest
import pytest_asyncio

from apihelpers import (
    ApiSession,
    Call,
    CommandRun,
    Reply,
    api_session,
    fake_model_protocol,
    printed,
    retries_clause,
)
from harness_plugin import FORCED_NATIVE, MAIN_USAGE, refusal

pytestmark = pytest.mark.asyncio

# The case whose paginator only Python runs, which a forced-native run
# refuses as test_each_paginated_command_s_route checks.
PYTHON_PAGINATOR: Final = "query-expression-token"

MODELS: Final = Path(__file__).resolve().parents[3] / "models"
SERVICES: Final = ("zjson", "zquery", "zrestjson")
USAGE: Final = "\n" + MAIN_USAGE


@pytest_asyncio.fixture
async def paged(aws_cli: str, tmp_path: Path) -> AsyncIterator[ApiSession]:
    async with api_session(aws_cli, tmp_path) as session:
        session.env["AWS_DATA_PATH"] = str(MODELS)
        session.env["TZ"] = "UTC"
        session.protocol = fake_model_protocol(MODELS, SERVICES)
        yield session


def token(**keys: Any) -> str:
    """The token the CLI prints as `NextToken` (`TokenEncoder.encode`):
    the JSON of the service's tokens, and of `boto_truncate_amount` when
    it stopped inside a page, in base64."""
    return base64.b64encode(json.dumps(keys).encode()).decode()


def params(run: CommandRun) -> list[dict[str, Any]]:
    return [call.params for call in run.calls]


def wire(call: Call) -> tuple[str, bytes]:
    """The request's path with its query string, and its body."""
    return call.request.effective_path, call.request.body or b""


def features(call: Call) -> set[str]:
    """The feature ids of the request's `m/` User-Agent metric, which
    botocore writes in no fixed order (`UserAgentComponent` of a set)."""
    agent = call.request.headers.get("user-agent") or ""
    (metric,) = [part for part in agent.split() if part.startswith("m/")]
    return set(metric.removeprefix("m/").split(","))


# ListThings' pages: two, one and two things.
THINGS: Final = [
    {"Things": ["a", "b"], "NextToken": "t1"},
    {"Things": ["c"], "NextToken": "t2"},
    {"Things": ["d", "e"]},
]
WIDGETS: Final = [{"WidgetId": "w1", "Size": 1}, {"WidgetId": "w2", "Size": 2}]
PARAMETERS: Final = [{"Name": "a", "Value": "1"}, {"Name": "b", "Value": "2"}]
KEYS: Final = [{"KeyId": "k1"}, {"KeyId": "k2"}]
EVENTS: Final = [{"eventId": "e1"}, {"eventId": "e2"}]
STREAMS: Final = [{"logStreamName": "s1"}, {"logStreamName": "s2"}]
FUNCTIONS: Final = [{"FunctionName": "f1"}, {"FunctionName": "f2"}]
USERS: Final = [{"Id": "u1"}, {"Id": "u2"}]
JOBS: Final = [{"JobId": "j1"}, {"JobId": "j2"}]
DEFAULTS: Final = [
    {"Defaults": {"Family": "f", "Marker": "m1", "Parameters": PARAMETERS[:1]}},
    {"Defaults": {"Family": "f", "Parameters": PARAMETERS[1:]}},
]
KEY_PAGES: Final = [
    {"Keys": KEYS[:1], "NextMarker": "m1", "Truncated": True},
    # Truncated false ends the pages, whatever the token says.
    {"Keys": KEYS[1:], "NextMarker": "m2", "Truncated": False},
]
EVENT_PAGES: Final = [
    {"events": EVENTS[:1], "searchedLogStreams": STREAMS[:1], "nextToken": "n1"},
    {"events": EVENTS[1:], "searchedLogStreams": STREAMS[1:]},
]
USER_PAGES: Final = [
    {"Users": USERS[:1], "NextToken": "n1", "ApproximateTotalCount": 2},
    {"Users": USERS[1:], "ApproximateTotalCount": 3},
]


# Several pages merged.

# Each paginator's command, its pages, each request's parameters, and the
# full result (`build_full_result`).
MERGES: Final = {
    "json-next-token": (
        "zjson list-things",
        THINGS,
        [{}, {"NextToken": "t1"}, {"NextToken": "t2"}],
        {"Things": ["a", "b", "c", "d", "e"]},
    ),
    "json-more-results": (
        "zjson list-keys",
        KEY_PAGES,
        [{}, {"Marker": "m1"}],
        {"Keys": KEYS},
    ),
    "json-two-result-keys": (
        "zjson filter-events",
        EVENT_PAGES,
        [{}, {"nextToken": "n1"}],
        {"events": EVENTS, "searchedLogStreams": STREAMS},
    ),
    "query-no-limit-key": (
        "zquery list-things",
        [{"Things": ["a"], "NextToken": "t1"}, {"Things": ["b"]}],
        [{}, {"NextToken": "t1"}],
        {"Things": ["a", "b"]},
    ),
    "query-marker": (
        "zquery describe-widgets",
        [{"Widgets": WIDGETS[:1], "Marker": "m1"}, {"Widgets": WIDGETS[1:]}],
        [{}, {"Marker": "m1"}],
        {"Widgets": WIDGETS},
    ),
    # Only the result key is kept of the structure holding it: its Family
    # is neither a result key nor a non-aggregate one.
    "query-dotted": (
        "zquery describe-defaults --family f",
        DEFAULTS,
        [{"Family": "f"}, {"Family": "f", "Marker": "m1"}],
        {"Defaults": {"Parameters": PARAMETERS}},
    ),
    "query-expression-token": (
        "zquery list-jobs",
        [
            {"Jobs": JOBS[:1], "IsTruncated": True},
            {"Jobs": JOBS[1:], "IsTruncated": False},
        ],
        [{}, {"Marker": "j1"}],
        {"Jobs": JOBS},
    ),
    "rest-json-query-string": (
        "zrestjson list-functions",
        [{"Functions": FUNCTIONS[:1], "NextMarker": "m1"}, {"Functions": FUNCTIONS[1:]}],
        [{}, {"Marker": "m1"}],
        {"Functions": FUNCTIONS},
    ),
    # A non-aggregate key keeps the first page's value.
    "rest-json-body": (
        "zrestjson search-users --instance-id i",
        USER_PAGES,
        [{"InstanceId": "i"}, {"InstanceId": "i", "NextToken": "n1"}],
        {"Users": USERS, "ApproximateTotalCount": 2},
    ),
}


@pytest.mark.parametrize("case", MERGES)
async def test_the_pages_merge_into_one_result(paged: ApiSession, case: str) -> None:
    if FORCED_NATIVE and case == PYTHON_PAGINATOR:
        pytest.skip("forced native refuses it (paginator), as the route test checks")
    command, pages, sent, merged = MERGES[case]
    run = await paged.run(command, replies=pages)
    assert params(run) == sent, run.describe()
    assert (run.stdout, run.stderr) == (printed(merged), ""), run.describe()


async def test_a_non_aggregate_key_missing_from_the_first_page_is_null(
    paged: ApiSession,
) -> None:
    # `_record_non_aggregate_key_values` reads the first page alone, and
    # sets what it does not find to None.
    pages = [{"Users": USERS[:1], "NextToken": "n1"}, USER_PAGES[1]]
    run = await paged.run("zrestjson search-users --instance-id i", replies=pages)
    assert run.stdout == printed(
        {"Users": USERS, "ApproximateTotalCount": None}
    ), run.describe()


async def test_an_empty_token_ends_the_pages(paged: ApiSession) -> None:
    # `_get_next_token` reads an empty token as none.
    run = await paged.run(
        "zjson list-things", replies=[{"Things": ["a"], "NextToken": ""}]
    )
    assert params(run) == [{}]
    assert run.stdout == printed({"Things": ["a"]}), run.describe()


# Text, page by page.

# The text formatter prints each page's result keys as it arrives, the
# non-aggregate part only with the first.
TEXTS: Final = {
    "json-next-token": (
        "zjson list-things",
        THINGS,
        "THINGS\ta\nTHINGS\tb\nTHINGS\tc\nTHINGS\td\nTHINGS\te\n",
    ),
    # Each page's events, then its streams.
    "json-two-result-keys": (
        "zjson filter-events",
        EVENT_PAGES,
        "EVENTS\te1\nSEARCHEDLOGSTREAMS\ts1\nEVENTS\te2\nSEARCHEDLOGSTREAMS\ts2\n",
    ),
    "query-marker": (
        "zquery describe-widgets",
        [{"Widgets": WIDGETS[:1], "Marker": "m1"}, {"Widgets": WIDGETS[1:]}],
        "WIDGETS\t1\tw1\nWIDGETS\t2\tw2\n",
    ),
    "query-dotted": (
        "zquery describe-defaults --family f",
        DEFAULTS,
        "PARAMETERS\ta\t1\nPARAMETERS\tb\t2\n",
    ),
    "rest-json-body": (
        "zrestjson search-users --instance-id i",
        USER_PAGES,
        "2\nUSERS\tu1\nUSERS\tu2\n",
    ),
    # A page without the result key prints it as None.
    "page-without-result-key": (
        "zjson list-things",
        [
            {"Things": ["a"], "NextToken": "t1"},
            {"NextToken": "t2"},
            {"Things": ["b"]},
        ],
        "THINGS\ta\nNone\nTHINGS\tb\n",
    ),
}


@pytest.mark.parametrize("case", TEXTS)
async def test_text_prints_each_page(paged: ApiSession, case: str) -> None:
    command, pages, text = TEXTS[case]
    run = await paged.run(f"{command} --output text", replies=pages)
    assert (run.stdout, run.stderr) == (text, ""), run.describe()


# --max-items.

# `--max-items N` over THINGS: the requests it takes, what it keeps, and
# the token it prints, which names the token of the page it stopped in and
# how much of that page it kept, or the next page's token on a boundary.
TRUNCATIONS: Final = {
    "mid-first-page": (
        1, 1, ["a"], token(NextToken=None, boto_truncate_amount=1)
    ),
    "first-page-boundary": (2, 1, ["a", "b"], token(NextToken="t1")),
    "later-page-boundary": (3, 2, ["a", "b", "c"], token(NextToken="t2")),
    "mid-later-page": (
        4, 3, ["a", "b", "c", "d"],
        token(NextToken="t2", boto_truncate_amount=1),
    ),
    # The last page ends the pages before the boundary is checked.
    "every-item": (5, 3, ["a", "b", "c", "d", "e"], None),
    "more-than-every-item": (9, 3, ["a", "b", "c", "d", "e"], None),
}


@pytest.mark.parametrize("case", TRUNCATIONS)
async def test_max_items_truncates_the_result(paged: ApiSession, case: str) -> None:
    max_items, requests, kept, resume = TRUNCATIONS[case]
    command = f"zjson list-things --max-items {max_items}"
    run = await paged.run(command, replies=THINGS[:requests])
    assert len(run.calls) == requests, run.describe()
    full = {"Things": kept} | ({} if resume is None else {"NextToken": resume})
    assert (run.stdout, run.stderr) == (printed(full), ""), run.describe()
    run = await paged.run(f"{command} --output text", replies=THINGS[:requests])
    rows = "".join(f"THINGS\t{thing}\n" for thing in kept)
    if resume is not None:
        rows += f"NEXTTOKEN\t{resume}\n"
    assert (run.stdout, run.stderr) == (rows, ""), run.describe()


# Truncation by other paginators: the command, its pages, and the result.
OTHER_TRUNCATIONS: Final = {
    # The token names the input token, beside the limit key.
    "rest-json-query-string": (
        "zrestjson list-functions --page-size 2 --max-items 3",
        [
            {"Functions": FUNCTIONS, "NextMarker": "m1"},
            {"Functions": [{"FunctionName": "f3"}, {"FunctionName": "f4"}],
             "NextMarker": "m2"},
        ],
        {
            "Functions": [*FUNCTIONS, {"FunctionName": "f3"}],
            "NextToken": token(Marker="m1", boto_truncate_amount=1),
        },
    ),
    "query-dotted": (
        "zquery describe-defaults --family f --max-items 1",
        [{"Defaults": {"Family": "f", "Marker": "m1", "Parameters": PARAMETERS}}],
        {
            "Defaults": {"Parameters": PARAMETERS[:1]},
            "NextToken": token(Marker=None, boto_truncate_amount=1),
        },
    ),
    # Only the first result key counts and is truncated.
    "json-two-result-keys": (
        "zjson filter-events --max-items 1",
        [{"events": EVENTS, "searchedLogStreams": STREAMS, "nextToken": "n1"}],
        {
            "events": EVENTS[:1],
            "searchedLogStreams": STREAMS,
            "NextToken": token(nextToken=None, boto_truncate_amount=1),
        },
    ),
    "rest-json-body": (
        "zrestjson search-users --instance-id i --max-items 1",
        [{"Users": USERS, "NextToken": "n1", "ApproximateTotalCount": 2}],
        {
            "Users": USERS[:1],
            "ApproximateTotalCount": 2,
            "NextToken": token(NextToken=None, boto_truncate_amount=1),
        },
    ),
}


@pytest.mark.parametrize("case", OTHER_TRUNCATIONS)
async def test_max_items_truncates_each_paginator(
    paged: ApiSession, case: str
) -> None:
    command, pages, full = OTHER_TRUNCATIONS[case]
    run = await paged.run(command, replies=pages)
    assert len(run.calls) == len(pages), run.describe()
    assert (run.stdout, run.stderr) == (printed(full), ""), run.describe()


async def test_text_prints_the_token_after_the_pages(paged: ApiSession) -> None:
    run = await paged.run(
        "zrestjson search-users --instance-id i --max-items 1 --output text",
        replies=[{"Users": USERS, "NextToken": "n1", "ApproximateTotalCount": 2}],
    )
    assert run.stdout == (
        "2\nUSERS\tu1\n"
        f"NEXTTOKEN\t{token(NextToken=None, boto_truncate_amount=1)}\n"
    ), run.describe()


@pytest.mark.parametrize(
    "max_items,kept,amount", [(0, [], 0), (-1, ["a"], -1)], ids=["zero", "negative"]
)
async def test_a_non_positive_max_items_warns(
    paged: ApiSession, max_items: int, kept: list[str], amount: int
) -> None:
    # `PageArgument.add_to_params` warns; the truncation then keeps
    # `len(page) - truncate_amount` items, a negative slice for -1.
    run = await paged.run(
        f"zjson list-things --max-items {max_items}", replies=THINGS[:1]
    )
    assert (run.stdout, run.stderr) == (
        printed(
            {
                "Things": kept,
                "NextToken": token(NextToken=None, boto_truncate_amount=amount),
            }
        ),
        "warning: Non-positive values for --max-items may result in "
        "undefined behavior.\n",
    ), run.describe()


# --starting-token.


async def test_the_printed_token_resumes_where_max_items_stopped(
    paged: ApiSession,
) -> None:
    run = await paged.run("zjson list-things --max-items 4", replies=THINGS)
    resume = json.loads(run.stdout)["NextToken"]
    run = await paged.run(
        ["zjson", "list-things", "--starting-token", resume], replies=THINGS[2:]
    )
    # The page it stopped in again, less what was kept of it.
    assert params(run) == [{"NextToken": "t2"}], run.describe()
    assert (run.stdout, run.stderr) == (printed({"Things": ["e"]}), "")


# A starting token, `--max-items`, the pages, each request's parameters,
# and the result.
RESUMES: Final = {
    # No service token: the first request has none.
    "first-page-truncated": (
        token(NextToken=None, boto_truncate_amount=1),
        2,
        THINGS[:2],
        [{}, {"NextToken": "t1"}],
        {"Things": ["b", "c"], "NextToken": token(NextToken="t2")},
    ),
    # The amount kept counts from the page's start, past what the starting
    # token skipped.
    "truncated-again-in-the-same-page": (
        token(NextToken=None, boto_truncate_amount=1),
        1,
        [{"Things": ["a", "b", "c"], "NextToken": "t1"}],
        [{}],
        {
            "Things": ["b"],
            "NextToken": token(NextToken=None, boto_truncate_amount=2),
        },
    ),
    "truncated-in-a-later-page": (
        token(NextToken="t1", boto_truncate_amount=1),
        2,
        [{"Things": ["b", "c"], "NextToken": "t2"}, {"Things": ["d", "e"]}],
        [{"NextToken": "t1"}, {"NextToken": "t2"}],
        {
            "Things": ["c", "d"],
            "NextToken": token(NextToken="t2", boto_truncate_amount=1),
        },
    ),
    "no-truncation": (
        token(NextToken="t1"),
        None,
        THINGS[1:],
        [{"NextToken": "t1"}, {"NextToken": "t2"}],
        {"Things": ["c", "d", "e"]},
    ),
    # `_parse_starting_token_deprecated`: `token___index`.
    "old-style-with-index": (
        "t1___1",
        None,
        [{"Things": ["x", "y", "z"]}],
        [{"NextToken": "t1"}],
        {"Things": ["y", "z"]},
    ),
    # A token that is neither passes as the service's own.
    "opaque": (
        "bogus",
        None,
        [{"Things": ["x"]}],
        [{"NextToken": "bogus"}],
        {"Things": ["x"]},
    ),
}


@pytest.mark.parametrize("case", RESUMES)
async def test_a_starting_token_resumes(paged: ApiSession, case: str) -> None:
    starting, max_items, pages, sent, full = RESUMES[case]
    command = ["zjson", "list-things", "--starting-token", starting]
    if max_items is not None:
        command += ["--max-items", str(max_items)]
    run = await paged.run(command, replies=pages)
    assert params(run) == sent, run.describe()
    assert (run.stdout, run.stderr) == (printed(full), ""), run.describe()


async def test_resuming_empties_the_other_result_keys(paged: ApiSession) -> None:
    # `_handle_first_request` slices the first result key and empties the
    # others, which the truncated page already printed.
    run = await paged.run(
        [
            "zjson", "filter-events", "--starting-token",
            token(nextToken=None, boto_truncate_amount=1),
        ],
        replies=[{"events": EVENTS, "searchedLogStreams": STREAMS[:1]}],
    )
    assert run.stdout == printed(
        {"events": EVENTS[1:], "searchedLogStreams": []}
    ), run.describe()


async def test_a_resumed_text_run_prints_its_token(paged: ApiSession) -> None:
    run = await paged.run(
        [
            "zjson", "list-things", "--output", "text", "--max-items", "1",
            "--starting-token", token(NextToken="t1", boto_truncate_amount=1),
        ],
        replies=[{"Things": ["b", "c", "d"], "NextToken": "t2"}],
    )
    assert run.stdout == (
        "THINGS\tc\n"
        f"NEXTTOKEN\t{token(NextToken='t1', boto_truncate_amount=2)}\n"
    ), run.describe()


@pytest.mark.parametrize(
    "starting,stderr,rc",
    [
        # More parts than input tokens (`_convert_deprecated_starting_token`).
        ("a___b___c", "\naws: [ERROR]: Bad starting token: a___b___c\n", 255),
        # A token naming no input token is injected as given, and fails
        # validation.
        (
            token(Foo="x"),
            "\naws: [ERROR]: An error occurred (ParamValidation): Parameter "
            'validation failed:\nUnknown parameter in input: "Foo", must be '
            "one of: MaxResults, NextToken\n",
            252,
        ),
    ],
    ids=["too-many-parts", "unknown-key"],
)
async def test_a_bad_starting_token_fails_before_any_request(
    paged: ApiSession, starting: str, stderr: str, rc: int
) -> None:
    run = await paged.run(
        ["zjson", "list-things", "--starting-token", starting],
        replies=[],
        expected_rc=rc,
    )
    assert run.calls == ()
    assert (run.stdout, run.stderr) == ("", stderr), run.describe()


# --page-size.

# The command and each request's path and body: the limit key is set
# before the first request (`_inject_starting_params`), so it precedes the
# token each later page adds, in the query string and in a JSON body alike.
PAGE_SIZES: Final = {
    "json": (
        "zjson list-things --page-size 2",
        THINGS,
        [
            ("/", b'{"MaxResults": 2}'),
            ("/", b'{"MaxResults": 2, "NextToken": "t1"}'),
            ("/", b'{"MaxResults": 2, "NextToken": "t2"}'),
        ],
    ),
    # The starting token is set first.
    "json-with-starting-token": (
        f"zjson list-things --starting-token {token(NextToken='t1')} --page-size 2",
        THINGS[1:],
        [
            ("/", b'{"NextToken": "t1", "MaxResults": 2}'),
            ("/", b'{"NextToken": "t2", "MaxResults": 2}'),
        ],
    ),
    "query": (
        "zquery describe-widgets --page-size 1",
        [{"Widgets": WIDGETS[:1], "Marker": "m1"}, {"Widgets": WIDGETS[1:]}],
        [
            ("/", b"Action=DescribeWidgets&Version=2020-01-01&MaxRecords=1"),
            (
                "/",
                b"Action=DescribeWidgets&Version=2020-01-01&MaxRecords=1"
                b"&Marker=m1",
            ),
        ],
    ),
    "rest-json-query-string": (
        "zrestjson list-functions --page-size 5",
        [{"Functions": FUNCTIONS[:1], "NextMarker": "m1"}, {"Functions": FUNCTIONS[1:]}],
        [("/functions?MaxItems=5", b""), ("/functions?MaxItems=5&Marker=m1", b"")],
    ),
    "rest-json-body": (
        "zrestjson search-users --instance-id i --page-size 1",
        USER_PAGES,
        [
            ("/search-users", b'{"InstanceId": "i", "MaxResults": 1}'),
            (
                "/search-users",
                b'{"InstanceId": "i", "MaxResults": 1, "NextToken": "n1"}',
            ),
        ],
    ),
}


@pytest.mark.parametrize("case", PAGE_SIZES)
async def test_page_size_is_the_limit_key_of_every_request(
    paged: ApiSession, case: str
) -> None:
    command, pages, requests = PAGE_SIZES[case]
    run = await paged.run(command, replies=pages)
    assert [wire(call) for call in run.calls] == requests, run.describe()


async def test_a_paginator_without_a_limit_key_has_no_page_size(
    paged: ApiSession,
) -> None:
    run = await paged.run("zquery list-things --page-size 2", expected_rc=252)
    assert run.calls == ()
    assert run.stderr == (
        f"{USAGE}\n\naws: [ERROR]: Unknown options: --page-size, 2\n"
    ), run.describe()
    # Its own limit member is no input token, so it pages on.
    run = await paged.run(
        "zquery list-things --max-results 2",
        replies=[{"Things": ["a"], "NextToken": "t1"}, {"Things": ["b"]}],
    )
    assert params(run) == [
        {"MaxResults": 2},
        {"MaxResults": 2, "NextToken": "t1"},
    ], run.describe()
    assert run.stdout == printed({"Things": ["a", "b"]})


@pytest.mark.parametrize("option", ["--max-items", "--page-size"])
async def test_a_paging_option_parses_as_its_type(
    paged: ApiSession, option: str
) -> None:
    run = await paged.run(f"zjson list-things {option} x", expected_rc=252)
    assert run.calls == ()
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (ParamValidation): argument "
        f"{option}: invalid int value: 'x'\n{USAGE}"
    ), run.describe()


# Paging off.


async def test_no_paginate_makes_one_request(paged: ApiSession) -> None:
    # The operation's own output, the service's token included.
    run = await paged.run("zjson list-things --no-paginate", replies=THINGS[:1])
    assert params(run) == [{}]
    assert run.stdout == printed(THINGS[0]), run.describe()
    run = await paged.run(
        "zjson list-things --no-paginate --output text", replies=THINGS[:1]
    )
    assert run.stdout == "t1\nTHINGS\ta\nTHINGS\tb\n", run.describe()


# An input token or the limit key, given as the operation's own argument,
# turns paging off (`check_should_enable_pagination`).
OWN_ARGUMENTS: Final = {
    "input-token": (
        "zjson list-things --next-token x",
        {"NextToken": "x"},
        THINGS[0],
    ),
    "limit-key": (
        "zjson list-things --max-results 7",
        {"MaxResults": 7},
        THINGS[0],
    ),
    "rest-json-marker": (
        "zrestjson list-functions --marker m0",
        {"Marker": "m0"},
        {"NextMarker": "m1", "Functions": FUNCTIONS[:1]},
    ),
}


@pytest.mark.parametrize("case", OWN_ARGUMENTS)
async def test_the_operation_s_own_paging_argument_turns_paging_off(
    paged: ApiSession, case: str
) -> None:
    command, sent, page = OWN_ARGUMENTS[case]
    run = await paged.run(command, replies=[page])
    assert params(run) == [sent], run.describe()
    assert (run.stdout, run.stderr) == (printed(page), ""), run.describe()


@pytest.mark.parametrize(
    "command,named",
    [
        ("zjson list-things --no-paginate --max-items 2", "--max-items"),
        (
            "zjson list-things --no-paginate --starting-token a --page-size 1 "
            "--max-items 1",
            "--starting-token, --page-size, --max-items",
        ),
        (
            "zjson list-things --next-token x --page-size 2 --starting-token y",
            "--starting-token, --page-size",
        ),
        (
            "zrestjson search-users --instance-id i --next-token n0 --max-items 1",
            "--max-items",
        ),
        # The operation's own --max-items is no paging argument.
        (
            "zrestjson list-functions --max-items 1 --no-paginate --page-size 3",
            "--page-size",
        ),
    ],
    ids=["no-paginate", "every-option", "own-token", "body-token", "shadowed"],
)
async def test_paging_options_cannot_join_paging_off(
    paged: ApiSession, command: str, named: str
) -> None:
    run = await paged.run(command, replies=[], expected_rc=252)
    assert run.calls == ()
    assert (run.stdout, run.stderr) == (
        "",
        "\naws: [ERROR]: An error occurred (ParamValidation): Cannot specify "
        f"--no-paginate along with pagination arguments: {named}\n",
    ), run.describe()


# The operation's own MaxItems, which --max-items shadows.


async def test_max_items_pages_and_shadows_the_operation_s_own(
    paged: ApiSession,
) -> None:
    page = {"Functions": FUNCTIONS, "NextMarker": "m1"}
    run = await paged.run("zrestjson list-functions --max-items 1", replies=[page])
    assert [wire(call) for call in run.calls] == [("/functions", b"")]
    assert run.stdout == printed(
        {
            "Functions": FUNCTIONS[:1],
            "NextToken": token(Marker=None, boto_truncate_amount=1),
        }
    ), run.describe()


@pytest.mark.parametrize(
    "command,path",
    [
        ("zrestjson list-functions --no-paginate --max-items 1", "/functions?MaxItems=1"),
        (
            "zrestjson list-functions --marker m0 --max-items 1",
            "/functions?Marker=m0&MaxItems=1",
        ),
    ],
    ids=["no-paginate", "own-token"],
)
async def test_paging_off_gives_max_items_back_to_the_operation(
    paged: ApiSession, command: str, path: str
) -> None:
    # `check_should_enable_pagination` restores the shadowed argument, and
    # `ensure_paging_params_not_set` lets it be.
    page = {"Functions": FUNCTIONS[:1], "NextMarker": "m1"}
    run = await paged.run(command, replies=[page])
    assert [wire(call) for call in run.calls] == [(path, b"")], run.describe()
    assert (run.stdout, run.stderr) == (
        printed({"NextMarker": "m1", "Functions": FUNCTIONS[:1]}),
        "",
    ), run.describe()
    run = await paged.run(f"{command} --output text", replies=[page])
    assert run.stdout == "m1\nFUNCTIONS\tf1\n", run.describe()


# Failures.


async def test_the_same_token_twice_stops_the_pages(paged: ApiSession) -> None:
    pages = [{"Things": ["a"], "NextToken": "t1"}, {"Things": ["b"], "NextToken": "t1"}]
    stderr = (
        "\naws: [ERROR]: Error during pagination: The same next token was "
        "received twice: {'NextToken': 't1'}\n"
    )
    run = await paged.run("zjson list-things", replies=pages, expected_rc=255)
    assert params(run) == [{}, {"NextToken": "t1"}], run.describe()
    assert (run.stdout, run.stderr) == ("", stderr), run.describe()
    # Text has printed the second page when its token repeats.
    run = await paged.run(
        "zjson list-things --output text", replies=pages, expected_rc=255
    )
    assert (run.stdout, run.stderr) == ("THINGS\ta\nTHINGS\tb\n", stderr)


@pytest.mark.parametrize(
    "output,stdout",
    [("json", ""), ("text", "THINGS\ta\nTHINGS\tb\n"), ("off", "")],
)
async def test_an_error_on_the_second_page(
    paged: ApiSession, output: str, stdout: str
) -> None:
    # JSON builds the full result before printing, so it prints nothing;
    # text has printed the first page.
    failure = Reply(
        {"Error": {"Code": "InvalidToken", "Message": "token expired"}}, status=400
    )
    run = await paged.run(
        f"zjson list-things --output {output}",
        replies=[THINGS[0], failure],
        expected_rc=254,
    )
    assert params(run) == [{}, {"NextToken": "t1"}], run.describe()
    retries = retries_clause(paged.env, retryable=False)
    assert (run.stdout, run.stderr) == (
        stdout,
        "\naws: [ERROR]: An error occurred (InvalidToken) when calling the "
        f"ListThings operation{retries}: token expired\n",
    ), run.describe()


# Empty results, and no output.


@pytest.mark.parametrize(
    "page,json_stdout,text_stdout",
    [
        ({"Things": []}, printed({"Things": []}), ""),
        # No result key at all: JSON prints nothing of `{}`; text prints the
        # page's None.
        ({}, "", "None\n"),
    ],
    ids=["empty-list", "no-result-key"],
)
async def test_an_empty_result(
    paged: ApiSession, page: dict[str, Any], json_stdout: str, text_stdout: str
) -> None:
    run = await paged.run("zjson list-things", replies=[page])
    assert (run.stdout, run.stderr) == (json_stdout, ""), run.describe()
    run = await paged.run("zjson list-things --output text", replies=[page])
    assert (run.stdout, run.stderr) == (text_stdout, ""), run.describe()


async def test_output_off_still_requests_every_page(paged: ApiSession) -> None:
    run = await paged.run("zjson list-things --output off", replies=THINGS)
    assert params(run) == [{}, {"NextToken": "t1"}, {"NextToken": "t2"}]
    assert (run.stdout, run.stderr) == ("", ""), run.describe()


# The User-Agent.


async def test_every_page_names_the_paginator_feature(paged: ApiSession) -> None:
    # `PageIterator._make_request` registers PAGINATOR, `C`, for each call.
    single = await paged.run("zjson list-things --no-paginate", replies=THINGS[:1])
    (call,) = single.calls
    unpaged = features(call)
    assert "C" not in unpaged, unpaged
    run = await paged.run("zjson list-things", replies=THINGS)
    assert [features(call) for call in run.calls] == [unpaged | {"C"}] * 3


# Routes.

# Every command above but ListJobs's, whose expression token only Python
# reads (`Jobs[-1].JobId`).
ROUTES: Final = {
    "json-next-token": ("zjson list-things --max-items 1 --page-size 1", None),
    "json-more-results": ("zjson list-keys", None),
    "json-two-result-keys": ("zjson filter-events", None),
    "json-no-paginate": ("zjson list-things --no-paginate", None),
    "json-own-token": ("zjson list-things --next-token x", None),
    "query-no-limit-key": ("zquery list-things", None),
    "query-marker": ("zquery describe-widgets --page-size 1", None),
    "query-dotted": ("zquery describe-defaults --family f", None),
    "rest-json-query-string": ("zrestjson list-functions --max-items 1", None),
    "rest-json-body": ("zrestjson search-users --instance-id i", None),
    "json-starting-token": (
        f"zjson list-things --starting-token {token(NextToken='t1')}",
        None,
    ),
    "json-text": ("zjson list-things --output text", None),
    "json-off": ("zjson list-things --output off", None),
    "json-non-positive-max-items": ("zjson list-things --max-items 0", None),
    "rest-json-shadowed-max-items": (
        "zrestjson list-functions --no-paginate --max-items 1",
        None,
    ),
    "query-expression-token": ("zquery list-jobs", "paginator"),
}


@pytest.mark.native_only
@pytest.mark.parametrize("case", ROUTES)
async def test_each_paginated_command_s_route(paged: ApiSession, case: str) -> None:
    command, fallback = ROUTES[case]
    if FORCED_NATIVE and fallback is not None:
        run = await paged.run(command, expected_rc=None)
        assert (run.rc, run.stderr, run.calls) == (255, refusal(fallback), ()), run.describe()
        return
    run = await paged.run(command, replies={})
    agent = run.calls[0].request.headers.get("user-agent") or ""
    tokens = agent.split()
    route = (
        any(part.startswith("lang/rust#") for part in tokens),
        [part for part in tokens if part.startswith("md/fallback#")],
    )
    if fallback is None:
        assert route == (True, []), agent
    else:
        assert route == (False, [f"md/fallback#{fallback}"]), agent
