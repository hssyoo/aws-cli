"""The output pager, on a terminal: `aws sts get-caller-identity` against
the local stub with stdout on a pseudo-terminal, comparing what the
terminal receives, stderr, and the exit status, as the Python route gives
them (`OutputStreamFactory`, `LazyPager`, the pager error handler).

The pagers are stand-ins on `PATH`: each records its name, argv and the
environment variables the CLI sets for it, then copies its input to the
terminal. Every command reaches only the session's stub.
"""

import asyncio
import os
import select
import signal
import stat
import subprocess
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Final

import pytest

from apihelpers import ApiSession, _protocol, _Responder
from harness_plugin import COMMAND_ENVIRONMENT, command_environment

pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(os.name == "nt", reason="POSIX pseudo-terminals only"),
]

COMMAND: Final = ["sts", "get-caller-identity"]
IDENTITY: Final = {
    "UserId": "AIDACKCEVSQ6C2EXAMPLE",
    "Account": "123456789012",
    "Arn": "arn:aws:iam::123456789012:user/tester",
}
OUTPUT: Final = (
    "{\n"
    '    "UserId": "AIDACKCEVSQ6C2EXAMPLE",\n'
    '    "Account": "123456789012",\n'
    '    "Arn": "arn:aws:iam::123456789012:user/tester"\n'
    "}\n"
)
TIMEOUT_SECONDS: Final = 30.0
# Every forced-native run refuses what routing gives to Python.
FORCED_NATIVE: Final = COMMAND_ENVIRONMENT.get("AWS_CLI_NATIVE_ROUTER") == "native"

# A pager that records what it was started with, then copies its input to
# the terminal. `$0` names the copy, so one script serves every name.
RECORDING_PAGER: Final = """#!/bin/sh
{
  printf 'name=%s\\n' "$(basename "$0")"
  printf 'argc=%s\\n' "$#"
  printf 'LESS=%s\\n' "${LESS-<unset>}"
  printf 'LD_LIBRARY_PATH=%s\\n' "${LD_LIBRARY_PATH-<unset>}"
  printf 'LD_LIBRARY_PATH_ORIG=%s\\n' "${LD_LIBRARY_PATH_ORIG-<unset>}"
  printf 'PYINSTALLER_RESET_ENVIRONMENT=%s\\n' "${PYINSTALLER_RESET_ENVIRONMENT-<unset>}"
} > "$PAGER_RECORD"
exec cat
"""
# A pager that never reads, and says when it started.
STALLING_PAGER: Final = """#!/bin/sh
: > "$PAGER_STARTED"
exec sleep 30
"""


@dataclass(frozen=True)
class PagedRun:
    """What one command showed on the terminal, on stderr, and its status."""

    terminal: str
    stderr: str
    returncode: int
    record: dict[str, str] | None

    def describe(self) -> str:
        return (
            f"status {self.returncode}\nterminal {self.terminal!r}\n"
            f"stderr {self.stderr!r}\nrecord {self.record!r}"
        )


@pytest.fixture
def pagers(tmp_path: Path) -> Path:
    """A directory of stand-in pagers: `less`, `pager-a`, `pager-b`,
    `pager-c` (recording), `stall`, and `not-executable`."""
    directory = tmp_path / "pagers"
    directory.mkdir()
    for name in ("less", "pager-a", "pager-b", "pager-c"):
        script = directory / name
        script.write_text(RECORDING_PAGER, encoding="utf-8")
        script.chmod(stat.S_IRWXU)
    stall = directory / "stall"
    stall.write_text(STALLING_PAGER, encoding="utf-8")
    stall.chmod(stat.S_IRWXU)
    (directory / "not-executable").write_text("#!/bin/sh\nexec cat\n", encoding="utf-8")
    return directory


def read_record(path: Path) -> dict[str, str] | None:
    if not path.exists():
        return None
    lines = path.read_text(encoding="utf-8").splitlines()
    return dict(line.split("=", 1) for line in lines)


def pager_environment(api: ApiSession, pagers: Path, extra: Mapping[str, str]) -> dict[str, str]:
    """The session's environment with the stand-ins first on `PATH` and
    no pager settings of the host's."""
    env = {
        key: value
        for key, value in api.env.items()
        if key not in ("AWS_PAGER", "PAGER", "LESS", "LD_LIBRARY_PATH", "LD_LIBRARY_PATH_ORIG")
    }
    env["PATH"] = f"{pagers}:/usr/bin:/bin"
    env["PAGER_RECORD"] = str(pagers / "record")
    env["PAGER_STARTED"] = str(pagers / "started")
    env.update(extra)
    return env


def drain(fd: int, deadline: float) -> bytes:
    """Everything the child writes to the terminal until it closes it."""
    output = bytearray()
    while (remaining := deadline - time.monotonic()) > 0:
        readable, _, _ = select.select([fd], [], [], remaining)
        if not readable:
            break
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        output += chunk
    return bytes(output)


def run_paged_blocking(
    api: ApiSession,
    pagers: Path,
    argv: Sequence[str],
    env: Mapping[str, str],
    *,
    replies: object = IDENTITY,
    terminal: bool = True,
    interrupt: bool = False,
) -> PagedRun:
    """`aws <argv>` with stdout on a pseudo-terminal (or a pipe), stderr
    on a pipe, against `replies`; with `interrupt`, SIGINT to the command's
    process group once the stalling pager has started, as Ctrl-C at a
    terminal sends it."""
    responder = _Responder(api.protocol or _protocol(), argv[0], replies)
    api.server.clear_requests()
    api.server.handler = responder.respond
    record = pagers / "record"
    started = pagers / "started"
    for path in (record, started):
        path.unlink(missing_ok=True)
    deadline = time.monotonic() + TIMEOUT_SECONDS
    parent, child = os.openpty() if terminal else (None, None)
    process = subprocess.Popen(
        [api.aws_cli, *argv],
        stdin=subprocess.DEVNULL,
        stdout=child if terminal else subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=command_environment(env),
        cwd=api.cwd,
        start_new_session=True,
    )
    if child is not None:
        os.close(child)
    try:
        if interrupt:
            while not started.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            os.killpg(process.pid, signal.SIGINT)
        if parent is not None:
            shown = drain(parent, deadline)
            stdout = b""
        else:
            assert process.stdout is not None
            stdout = process.stdout.read()
            shown = b""
        stderr = process.stderr.read() if process.stderr else b""
        process.wait(timeout=max(deadline - time.monotonic(), 1))
    finally:
        if parent is not None:
            os.close(parent)
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    text = (shown if terminal else stdout).decode()
    # The terminal's line discipline writes each newline as CRLF.
    return PagedRun(
        terminal=text.replace("\r\n", "\n"),
        stderr=stderr.decode(),
        returncode=process.returncode,
        record=read_record(record),
    )


async def run_paged(*args: object, **kwargs: object) -> PagedRun:
    """`run_paged_blocking` on a thread: the stub and its proxy serve on
    this test's event loop, which must keep running meanwhile."""
    return await asyncio.to_thread(run_paged_blocking, *args, **kwargs)  # type: ignore[arg-type]


def refusal(reason: str) -> str:
    return (
        "aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable "
        f"({reason}); Python fallback disabled\n"
    )


async def test_default_less_gets_frx(api: ApiSession, pagers: Path) -> None:
    run = await run_paged(api, pagers, COMMAND, pager_environment(api, pagers, {}))
    assert (run.returncode, run.terminal, run.stderr) == (0, OUTPUT, ""), run.describe()
    assert run.record is not None, run.describe()
    assert (run.record["name"], run.record["argc"], run.record["LESS"]) == ("less", "0", "FRX")


async def test_a_less_setting_is_kept(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"LESS": "-R"})
    run = await run_paged(api, pagers, COMMAND, env)
    assert (run.returncode, run.terminal) == (0, OUTPUT), run.describe()
    assert run.record is not None and run.record["LESS"] == "-R", run.describe()


@pytest.mark.parametrize(
    ("aws_pager", "cli_pager", "pager", "chosen"),
    [
        ("pager-a", "pager-b", "pager-c", "pager-a"),
        (None, "pager-b", "pager-c", "pager-b"),
        (None, None, "pager-c", "pager-c"),
        ("", "pager-b", "pager-c", None),
    ],
    ids=["aws-pager", "cli-pager", "pager", "empty-aws-pager"],
)
async def test_aws_pager_then_cli_pager_then_pager(
    api: ApiSession,
    pagers: Path,
    aws_pager: str | None,
    cli_pager: str | None,
    pager: str | None,
    chosen: str | None,
) -> None:
    settings = {"AWS_PAGER": aws_pager, "PAGER": pager}
    env = pager_environment(
        api, pagers, {key: value for key, value in settings.items() if value is not None}
    )
    if cli_pager is not None:
        with api.config_path.open("a", encoding="utf-8") as config:
            config.write(f"cli_pager = {cli_pager}\n")
    run = await run_paged(api, pagers, COMMAND, env)
    assert (run.returncode, run.terminal, run.stderr) == (0, OUTPUT, ""), run.describe()
    assert (run.record or {}).get("name") == chosen, run.describe()


async def test_no_cli_pager_writes_to_stdout(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "pager-a"})
    run = await run_paged(api, pagers, [*COMMAND, "--no-cli-pager"], env)
    assert (run.returncode, run.terminal, run.record) == (0, OUTPUT, None), run.describe()


async def test_stdout_that_is_no_terminal_is_not_paged(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "pager-a"})
    run = await run_paged(api, pagers, COMMAND, env, terminal=False)
    assert (run.returncode, run.terminal, run.record) == (0, OUTPUT, None), run.describe()


async def test_an_empty_response_starts_no_pager(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "pager-a"})
    run = await run_paged(api, pagers, COMMAND, env, replies={})
    assert (run.returncode, run.terminal, run.stderr, run.record) == (0, "", "", None), (
        run.describe()
    )


async def test_a_missing_pager_is_the_pager_error(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "no-such-pager"})
    run = await run_paged(api, pagers, COMMAND, env)
    assert run.returncode == 253, run.describe()
    assert run.stderr == (
        "\naws: [ERROR]: An error occurred (Pager): Unable to redirect output to pager. "
        "Received the following error when opening pager:\n"
        "[Errno 2] No such file or directory: 'no-such-pager'\n\n"
        'Learn more about configuring the output pager by running "aws help config-vars".\n'
    ), run.describe()
    assert run.terminal == "", run.describe()


async def test_a_pager_that_cannot_run_is_a_general_error(
    api: ApiSession, pagers: Path
) -> None:
    path = pagers / "not-executable"
    env = pager_environment(api, pagers, {"AWS_PAGER": str(path)})
    run = await run_paged(api, pagers, COMMAND, env)
    assert run.returncode == 255, run.describe()
    assert run.stderr == f"\naws: [ERROR]: [Errno 13] Permission denied: '{path}'\n", (
        run.describe()
    )


async def test_a_pager_that_quits_early_is_no_failure(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "sh -c 'exit 0'"})
    run = await run_paged(api, pagers, COMMAND, env)
    assert (run.returncode, run.stderr) == (0, ""), run.describe()


async def test_ctrl_c_while_the_pager_runs(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "stall"})
    run = await run_paged(api, pagers, COMMAND, env, interrupt=True)
    assert (run.returncode, run.terminal, run.stderr) == (130, "\n", ""), run.describe()


async def test_a_quoted_command_runs_as_split(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "sh -c 'cat'"})
    run = await run_paged(api, pagers, COMMAND, env)
    assert (run.returncode, run.terminal, run.stderr) == (0, OUTPUT, ""), run.describe()


async def test_an_unbalanced_quote_is_python_s(api: ApiSession, pagers: Path) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "sh -c 'cat"})
    run = await run_paged(api, pagers, COMMAND, env)
    if FORCED_NATIVE:
        assert (run.returncode, run.stderr) == (255, refusal("pager_setting")), run.describe()
        return
    assert run.returncode == 255, run.describe()
    assert run.stderr == "\naws: [ERROR]: No closing quotation\n", run.describe()


@pytest.mark.parametrize(
    ("given", "expected"),
    [
        ({"LD_LIBRARY_PATH": "/given", "LD_LIBRARY_PATH_ORIG": "/original"}, "/original"),
        ({"LD_LIBRARY_PATH": "/given"}, "<unset>"),
    ],
    ids=["restored", "removed"],
)
async def test_the_pager_s_library_path_is_the_original(
    api: ApiSession, pagers: Path, given: dict[str, str], expected: str
) -> None:
    env = pager_environment(api, pagers, {"AWS_PAGER": "pager-a", **given})
    run = await run_paged(api, pagers, COMMAND, env)
    assert (run.returncode, run.terminal) == (0, OUTPUT), run.describe()
    assert run.record is not None, run.describe()
    assert run.record["LD_LIBRARY_PATH"] == expected, run.describe()
    assert run.record["LD_LIBRARY_PATH_ORIG"] == given.get("LD_LIBRARY_PATH_ORIG", "<unset>")
    assert run.record["PYINSTALLER_RESET_ENVIRONMENT"] == "1", run.describe()
    assert run.record["LESS"] == "FRX", run.describe()
