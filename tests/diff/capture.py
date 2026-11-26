"""The differential harness's capture: run every row of a job on each side
against the blackbox TLS stub, and record what the CLI did.

The job (JSON, written by `cargo xtask diff`) names:

- `sides`: each a `name`, a `binary` and a `route`, the value of
  AWS_CLI_NATIVE_ROUTER (`native` or `python`), or `unset` to leave it out;
- `rows`: each an `id`, `argv`, `env` (a null value removes a variable),
  `data_path` (AWS_DATA_PATH, or null), `responses` (each a `status`,
  `headers` as ordered pairs, and a `body` or `body_base64`), an optional
  `timeout_s`, and an optional `requests` (0), which `cargo xtask diff`
  checks;
- `scratch`, a directory this owns, and `captures`, the output file.

One `localstub` server listens behind its TLS intercept proxy for the whole
job. Every run goes through the proxy (HTTPS_PROXY), so each request keeps
the host the CLI chose, and the CLI trusts the proxy's CA (AWS_CA_BUNDLE).
Each row runs on each side in turn, in the job's order, with a cleared
environment (`base_env`, then the row's `env`; a row cannot set the `OWNED`
variables), a scratch HOME and working directory recreated at the same
paths for every run, and stdin empty. The server answers the run's n-th
request with the row's n-th response, the last repeating, and from the
`REQUEST_LIMIT`-th request on closes the connection instead, so a run that
loops ends.

Each run is one line of `captures`: the row id, the side, the exit status
(null when the run timed out and was killed), stdout, stderr, the fake
secret key it had (which its signatures are checked against), the hosts of
the CONNECTs the proxy accepted (sorted), every request as the server
received it (method, target, HTTP version, headers in wire order, body
without chunk framing, a chunked body's size and trailer lines, and whether
the body was complete), and `harness`: what the capture could not see
whole, each `<kind>: <detail>` (a request the server could not parse, a
connection the proxy refused, requests the recorder dropped, a warning the
stub logged, output a stray process held open). Byte strings are written as
Latin-1 text, one character per byte.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import contextlib
import json
import logging
import os
import shutil
import signal
import ssl
import subprocess
from asyncio import StreamWriter
from pathlib import Path
from typing import Any, Final

from localstub import (
    AsyncHTTPTestServer,
    AsyncTLSInterceptProxy,
    CloseConnection,
    HTTPResponse,
    RecordedHTTPRequest,
)
from localstub.ca import TLSProxyCA

FORMAT: Final = 1
# Replies a run gets, the last repeating, before its connections close.
REQUEST_LIMIT: Final = 64
TIMEOUT_SECONDS: Final = 60
# How long a killed run's output may take to close.
KILL_GRACE_SECONDS: Final = 5
# How long the stub may take to record what a finished run's last
# connection did.
SETTLE_SECONDS: Final = 0.02
ROUTER: Final = "AWS_CLI_NATIVE_ROUTER"
# The variables the harness sets, which a row cannot: its HOME and files,
# the route, the fake models, and every proxy variable, so that nothing
# leaves the machine (xtask's corpus check refuses them too).
OWNED: Final = frozenset(
    {
        "HOME",
        "AWS_CONFIG_FILE",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_CA_BUNDLE",
        "AWS_DATA_PATH",
        ROUTER,
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    }
)
# The reasons a connection closes that mean a request was not seen whole.
HARNESS_CLOSES: Final = frozenset({"protocol_error", "error"})


def latin1(data: bytes) -> str:
    return data.decode("latin-1")


class ConnectRecorder(TLSProxyCA):
    """The proxy's CA, noting the host of each CONNECT it serves."""

    def __init__(self) -> None:
        super().__init__()
        self.hosts: list[str] = []

    def issue_context(self, host: str) -> ssl.SSLContext:
        self.hosts.append(host)
        return super().issue_context(host)


class Proxy(AsyncTLSInterceptProxy):
    """The blackbox stub's proxy, noting each connection it refuses (one
    that is not a CONNECT, or a malformed one), which no server sees."""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.refused: list[str] = []

    # localstub 0.1.1's one path for a refusal; the venv pins that version.
    async def _send_and_close(
        self, writer: StreamWriter, status_line: bytes, client_id: str
    ) -> None:
        self.refused.append(latin1(status_line))
        await super()._send_and_close(writer, status_line, client_id)


class Warnings(logging.Handler):
    """What the stub logs at WARNING or above."""

    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.messages: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.messages.append(record.getMessage().splitlines()[0])


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
    """The row's replies, the last repeating up to `REQUEST_LIMIT`; past
    them the server's handler closes each connection."""
    replies = [reply(spec) for spec in row["responses"]] or [HTTPResponse.raw(b"")]
    replies += [replies[-1]] * (REQUEST_LIMIT - len(replies))
    server.set_response_sequence(list(replies))
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
) -> tuple[int | None, bytes, bytes, list[str]]:
    """The run's exit status, or None if it outlived `timeout` and was
    killed with its process group, what it wrote, and the harness errors."""
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
        stdout, stderr = await asyncio.wait_for(asyncio.shield(communicating), timeout)
        return process.returncode, stdout, stderr, []
    except TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
    try:
        stdout, stderr = await asyncio.wait_for(communicating, KILL_GRACE_SECONDS)
    except TimeoutError:
        return None, b"", b"", ["pipes_held: a process outside the run's group kept its output open"]
    return None, stdout, stderr, []


def framing(request: RecordedHTTPRequest) -> list[str] | None:
    """A chunked body's chunk-size lines, then its trailer lines."""
    encoding = request.as_received.headers.get("transfer-encoding") or ""
    if "chunked" not in encoding.lower():
        return None
    wire = bytes(request.wire_raw_bytes)
    at = wire.find(b"\r\n\r\n") + 4
    lines: list[str] = []
    while (end := wire.find(b"\r\n", at)) >= 0:
        size_line = wire[at:end]
        lines.append(latin1(size_line))
        try:
            size = int(size_line.split(b";")[0], 16)
        except ValueError:
            return [*lines, "<malformed>"]
        at = end + 2 + (size + 2 if size else 0)
        if size == 0:
            break
    while (end := wire.find(b"\r\n", at)) > at:
        lines.append(latin1(wire[at:end]))
        at = end + 2
    return lines


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
        "framing": framing(request),
        "complete": request.body_complete,
    }


class Stub:
    """The server, its proxy and what they noted, reset for each run."""

    def __init__(
        self,
        server: AsyncHTTPTestServer,
        proxy: Proxy,
        ca: ConnectRecorder,
        warnings: Warnings,
    ) -> None:
        self.server = server
        self.proxy = proxy
        self.ca = ca
        self.warnings = warnings

    def reset(self, row: dict[str, Any]) -> None:
        load_replies(self.server, row)
        self.proxy.refused.clear()
        self.ca.hosts.clear()
        self.warnings.messages.clear()

    def harness_errors(self) -> list[str]:
        server = self.server
        errors = [
            f"{close.reason}: a connection closed in {close.phase}"
            for close in server.closed_connections
            if close.reason in HARNESS_CLOSES
        ]
        if server.dropped_requests:
            errors.append(f"dropped_requests: {server.dropped_requests}")
        errors += [f"proxy_refused: {line}" for line in self.proxy.refused]
        errors += [f"stub_warning: {message}" for message in self.warnings.messages]
        return errors


async def run_row(
    stub: Stub,
    base: dict[str, str],
    scratch: Path,
    row: dict[str, Any],
    side: dict[str, str],
) -> dict[str, Any]:
    work = fresh_scratch(scratch)
    stub.reset(row)
    env = run_env(base, row, side["route"])
    timeout = row.get("timeout_s")
    status, stdout, stderr, errors = await execute(
        side["binary"],
        row["argv"],
        env,
        work,
        TIMEOUT_SECONDS if timeout is None else timeout,
    )
    await asyncio.sleep(SETTLE_SECONDS)
    return {
        "id": row["id"],
        "side": side["name"],
        "harness": errors + stub.harness_errors(),
        "exit": status,
        "timed_out": status is None,
        "stdout": latin1(stdout),
        "stderr": latin1(stderr),
        "secret_key": env.get("AWS_SECRET_ACCESS_KEY"),
        "connects": sorted(stub.ca.hosts),
        "requests": [request_record(request) for request in stub.server.requests],
    }


async def capture(job: dict[str, Any]) -> None:
    scratch = Path(job["scratch"])
    out = Path(job["captures"])
    warnings = Warnings()
    logging.getLogger("localstub").addHandler(warnings)
    ca = ConnectRecorder()
    async with (
        AsyncHTTPTestServer() as server,
        Proxy(server=server, ca=ca) as proxy,
    ):
        server.handler = lambda _: CloseConnection()
        stub = Stub(server, proxy, ca, warnings)
        base = base_env(scratch, proxy.endpoint_url, str(ca.ca_pem_path()))
        with out.open("w", encoding="utf-8") as stream:
            for row in job["rows"]:
                for side in job["sides"]:
                    record = await run_row(stub, base, scratch, row, side)
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
