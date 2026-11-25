"""Connection reuse: a command of the repository's fake models (tests/models,
through `AWS_DATA_PATH`) opens one connection, however many pages it
reads, and sends exactly one request per page, on each protocol.

botocore keeps one urllib3 pool per client and reads each response to its
end, so every page after the first goes out on the same connection; the
native route's pool matches it. The connections are counted at the stub:
the TLS intercept proxy hands each connection the CLI opens to the server
as one connection of its own, which the server records when it closes.
Both routes must give the same counts, so the run forcing the Python
route checks the expectations against Python.
"""

import asyncio
from typing import Any, Final

import pytest
from localstub import AsyncHTTPTestServer

from apihelpers import ApiSession, fake_models_fixture

pytestmark = pytest.mark.asyncio

# How long the stub may take to record the closes of the connections a
# finished command leaves, and how long it is then given to record any
# further, request-less one.
CLOSE_SECONDS: Final = 5.0
STRAGGLER_SECONDS: Final = 0.2


models = fake_models_fixture("zjson", "zquery", "zrestjson")


async def connections(server: AsyncHTTPTestServer, requests: int) -> int:
    """The connections a finished command opened: once the closed ones
    account for its `requests`, and a moment more for any that served
    none."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + CLOSE_SECONDS
    while loop.time() < deadline:
        served = sum(closed.requests_completed for closed in server.closed_connections)
        if served >= requests:
            break
        await asyncio.sleep(0.01)
    await asyncio.sleep(STRAGGLER_SECONDS)
    return len(server.closed_connections)


def pages(key: str, token: str, count: int, item: Any) -> list[dict[str, Any]]:
    """`count` pages of one item each under `key`, each but the last with
    `token` naming the next."""
    return [
        {key: [item], **({token: f"t{page + 1}"} if page + 1 < count else {})}
        for page in range(count)
    ]


# Each protocol's paginated list command and its five pages.
PAGED: Final = {
    "json": ("zjson list-things", pages("Things", "NextToken", 5, "a")),
    "query": ("zquery list-things", pages("Things", "NextToken", 5, "a")),
    "rest-json": (
        "zrestjson list-functions",
        pages("Functions", "NextMarker", 5, {"FunctionName": "f"}),
    ),
}


@pytest.mark.parametrize("protocol", PAGED)
async def test_a_paged_command_reuses_one_connection_for_every_page(
    models: ApiSession, protocol: str
) -> None:
    command, replies = PAGED[protocol]
    run = await models.run(command, replies=replies)
    assert len(run.calls) == len(replies), run.describe()
    assert await connections(models.server, len(replies)) == 1, run.describe()


# Each protocol's single call and its reply.
SINGLE: Final = {
    "json": ("zjson get-thing --thing-id t", {"ThingId": "t", "Count": 1}),
    "query": ("zquery get-thing --thing-id t", {"ThingId": "t", "Count": 1}),
    "rest-json": ("zrestjson get-thing --thing-id t", {"ThingId": "t", "Count": 1}),
}


@pytest.mark.parametrize("protocol", SINGLE)
async def test_a_single_call_makes_one_connection_and_one_request(
    models: ApiSession, protocol: str
) -> None:
    command, reply = SINGLE[protocol]
    run = await models.run(command, replies=[reply])
    assert len(run.calls) == 1, run.describe()
    assert await connections(models.server, 1) == 1, run.describe()
