"""The differential harness's capture: run every row of a job on each side
against the blackbox TLS stub, and record what the CLI did.

The job (JSON, written by `cargo xtask diff`) names:

- `sides`: each a `name`, a `binary` and a `route`, the value of
  AWS_CLI_NATIVE_ROUTER (`native` or `python`), or `unset` to leave it out;
- `rows`: each an `id`, `argv`, `env` (a null value removes a variable),
  `data_path` (AWS_DATA_PATH, or null), `responses` (each a `status`,
  `headers` as ordered pairs, and a `body` or `body_base64`) and an optional
  `timeout_s`;
- `scratch`, a directory this owns, and `captures`, the output file.

One `localstub` server listens behind its TLS intercept proxy for the whole
job. Every run goes through the proxy (HTTPS_PROXY), so each request keeps
the host the CLI chose, and the CLI trusts the proxy's CA (AWS_CA_BUNDLE).
Each row runs on each side in turn, in the job's order, with a cleared
environment (`base_env`, then the row's `env`), a scratch HOME and working
directory recreated at the same paths for every run, and stdin empty. The
server answers the run's n-th request with the row's n-th response, the
last repeating, and closes the connection instead past `REQUEST_LIMIT`
requests, so a run that loops ends.

Each run is one line of `captures`: the row id, the side, the exit status
(null when the run timed out and was killed), stdout, stderr, and every
request as the server received it: method, target, HTTP version, headers in
wire order, body (without chunk framing) and whether the body was complete.
Byte strings are written as Latin-1 text, one character per byte.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
import shutil
import signal
import subprocess
from pathlib import Path
from typing import Any, Final

from localstub import (
    AsyncHTTPTestServer,
    AsyncTLSInterceptProxy,
    CloseConnection,
    HTTPResponse,
    RecordedHTTPRequest,
)

FORMAT: Final = 1
# Replies a run gets, the last repeating, before its connections close.
REQUEST_LIMIT: Final = 64
TIMEOUT_SECONDS: Final = 60
ROUTER: Final = "AWS_CLI_NATIVE_ROUTER"
# The variables the harness sets; a row cannot.
OWNED: Final = frozenset(
    {
        "HOME",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "AWS_CA_BUNDLE",
        "AWS_CONFIG_FILE",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_DATA_PATH",
        ROUTER,
    }
)


def latin1(data: bytes) -> str:
    return data.decode("latin-1")


def base_env(scratch: Path, proxy: str, ca: str) -> dict[str, str]:
    """What every run starts from: fake keys, empty AWS files, no instance
    metadata, no pager, and the proxy as the only way out."""
    files = scratch / "home" / ".aws"
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": str(scratch / "home"),
        "TZ": "UTC",
        "LC_ALL": "C.UTF-8",
        "AWS_CONFIG_FILE": str(files / "config"),
        "AWS_SHARED_CREDENTIALS_FILE": str(files / "credentials"),
        "AWS_EC2_METADATA_DISABLED": "true",
        "AWS_PAGER": "",
        "AWS_ACCESS_KEY_ID": "AKIDEXAMPLE",
        "AWS_SECRET_ACCESS_KEY": "secret",
        "AWS_DEFAULT_REGION": "us-west-2",
        "HTTPS_PROXY": proxy,
        "HTTP_PROXY": proxy,
        "AWS_CA_BUNDLE": ca,
    }


def run_env(base: dict[str, str], row: dict[str, Any], route: str) -> dict[str, str]:
    env = dict(base)
    for name, value in row.get("env", {}).items():
        if name in OWNED:
            raise SystemExit(f"row {row['id']}: the harness owns {name}")
        if value is None:
            env.pop(name, None)
        else:
            env[name] = value
    if row.get("data_path") is not None:
        env["AWS_DATA_PATH"] = row["data_path"]
    if route != "unset":
        env[ROUTER] = route
    return env


def reply(spec: dict[str, Any]) -> HTTPResponse:
    if "body_base64" in spec:
        body = base64.b64decode(spec["body_base64"])
    else:
        body = spec.get("body", "").encode("utf-8")
    headers = [(name, value) for name, value in spec.get("headers", [])]
    return HTTPResponse.raw(body, status=spec.get("status", 200), headers=headers)


def load_replies(server: AsyncHTTPTestServer, row: dict[str, Any]) -> None:
    replies: list[HTTPResponse | CloseConnection] = [
        reply(spec) for spec in row["responses"]
    ] or [HTTPResponse.raw(b"")]
    replies += [replies[-1]] * (REQUEST_LIMIT - len(replies))
    replies += [CloseConnection()] * REQUEST_LIMIT
    server.set_response_sequence(replies)
    server.clear_requests()


def fresh_scratch(scratch: Path) -> Path:
    """The run's HOME, its empty AWS files, and its working directory."""
    shutil.rmtree(scratch, ignore_errors=True)
    files = scratch / "home" / ".aws"
    files.mkdir(parents=True)
    (files / "config").write_text("", encoding="utf-8")
    (files / "credentials").write_text("", encoding="utf-8")
    work = scratch / "work"
    work.mkdir()
    return work


async def execute(
    binary: str, argv: list[str], env: dict[str, str], cwd: Path, timeout: float
) -> tuple[int | None, bytes, bytes]:
    """The run's exit status, or None if it outlived `timeout` and was
    killed with its process group, and what it wrote."""
    process = await asyncio.create_subprocess_exec(
        binary,
        *argv,
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    communicating = asyncio.ensure_future(process.communicate())
    try:
        stdout, stderr = await asyncio.wait_for(
            asyncio.shield(communicating), timeout
        )
    except TimeoutError:
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = await communicating
        return None, stdout, stderr
    return process.returncode, stdout, stderr


def request_record(request: RecordedHTTPRequest) -> dict[str, Any]:
    received = request.as_received
    return {
        "method": received.method,
        "target": received.target,
        "version": request.http_version,
        "headers": [
            [latin1(name), latin1(value)] for name, value in received.headers.raw
        ],
        "body": latin1(received.body or b""),
        "complete": request.body_complete,
    }


async def run_row(
    server: AsyncHTTPTestServer,
    base: dict[str, str],
    scratch: Path,
    row: dict[str, Any],
    side: dict[str, str],
) -> dict[str, Any]:
    work = fresh_scratch(scratch)
    load_replies(server, row)
    env = run_env(base, row, side["route"])
    timeout = row.get("timeout_s") or TIMEOUT_SECONDS
    status, stdout, stderr = await execute(
        side["binary"], row["argv"], env, work, timeout
    )
    return {
        "id": row["id"],
        "side": side["name"],
        "exit": status,
        "timed_out": status is None,
        "stdout": latin1(stdout),
        "stderr": latin1(stderr),
        "requests": [request_record(request) for request in server.requests],
    }


async def capture(job: dict[str, Any]) -> None:
    scratch = Path(job["scratch"])
    out = Path(job["captures"])
    async with (
        AsyncHTTPTestServer() as server,
        AsyncTLSInterceptProxy(server=server) as proxy,
    ):
        base = base_env(scratch, proxy.endpoint_url, str(proxy.ca.ca_pem_path()))
        with out.open("w", encoding="utf-8") as stream:
            for row in job["rows"]:
                for side in job["sides"]:
                    record = await run_row(server, base, scratch, row, side)
                    stream.write(json.dumps(record) + "\n")
                    stream.flush()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--job", type=Path, required=True)
    args = parser.parse_args()
    job = json.loads(args.job.read_text(encoding="utf-8"))
    if job.get("format") != FORMAT:
        raise SystemExit(f"job format {job.get('format')!r}, expected {FORMAT}")
    asyncio.run(capture(job))


if __name__ == "__main__":
    main()
