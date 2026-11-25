"""Measure the AWS CLI's commands against the loopback stub.

`cargo xtask perf` runs this through run.py, which provides the venv, and
prints the table. For each case of the case file, at each of its schemes
and latencies, the command runs on the native and the Python route in
turn, round-robin: every round runs every series once, the first `warmup`
rounds unrecorded, so a passing load falls on every series alike.

Each run starts the CLI from a scratch environment (fake keys, empty
credential files, a scratch HOME, the instance metadata service off, the
stub as `--endpoint-url` and `AWS_ENDPOINT_URL`), with stdout and stderr
going to files, and waits for it with `wait4`, whose resource usage gives
the child's peak RSS and CPU user and system time. Wall time runs from
just before the spawn to the reap; the one-minute load average just
before the spawn is recorded with each run. A run fails the measurement
unless the CLI exits 0 and asked for exactly the case's pages: then the
numbers would not be of the command meant.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import signal
import ssl
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from string import Template
from typing import Any, Final

import trustme

from stub import Script, Stub, frame

FORMAT: Final = 1
RUN_TIMEOUT_SECONDS: Final = 120
ACCESS_KEY: Final = "AKIDPERFHARNESS00000"
SECRET_KEY: Final = "perf-harness-secret-key-not-a-real-one"
REGION: Final = "us-east-1"
CONFIG: Final = "[default]\ns3 =\n  addressing_style = path\n"
ROUTES: Final = ("native", "python")
# ru_maxrss is in bytes on macOS and in KiB on Linux.
RSS_UNIT: Final = 1 if sys.platform == "darwin" else 1024


@dataclass(frozen=True)
class Case:
    name: str
    argv: list[str]
    fake_models: bool
    pages: int


@dataclass(frozen=True)
class Setting:
    """One case at one scheme and latency, with its pages framed."""

    case: Case
    scheme: str
    latency_ms: int
    script: Script


@dataclass(frozen=True)
class Environment:
    """Where every run happens: the scratch files and the stubs."""

    scratch: Path
    models: Path
    ca_bundle: Path
    stubs: dict[str, Stub]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aws-binary", type=Path, required=True)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    return parser.parse_args()


def pages(script: dict[str, Any]) -> list[bytes]:
    """The case's framed responses: page n holds `items_per_page` items,
    numbered from n * items_per_page and joined by `separator`, and every
    page but the last a marker naming page n + 1."""
    page = Template(script["page"])
    item = Template(script.get("item", ""))
    marker = Template(script.get("marker", ""))
    per_page = script.get("items_per_page", 0)
    separator = script.get("separator", "")
    count = script["pages"]
    framed = []
    for number in range(count):
        items = separator.join(
            item.substitute(index=number * per_page + offset) for offset in range(per_page)
        )
        token = marker.substitute(next=number + 1) if number + 1 < count else ""
        body = page.substitute(items=items, marker=token).encode("utf-8")
        framed.append(frame(200, script["content_type"], body))
    return framed


def settings(path: Path) -> tuple[dict[str, Any], list[Setting]]:
    """The case file, and each of its cases at each scheme and latency."""
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("format") != FORMAT:
        raise SystemExit(f"{path}: expected format {FORMAT}")
    chosen = []
    for entry in data["cases"]:
        response = entry["response"]
        case = Case(entry["name"], entry["argv"], entry.get("fake_models", False), response["pages"])
        framed = pages(response)
        for scheme in entry["schemes"]:
            for latency in entry["latencies_ms"]:
                chosen.append(Setting(case, scheme, latency, Script(framed, latency / 1000)))
    return data, chosen


def environment_for(case: Case, scheme: str, route: str, env: Environment) -> dict[str, str]:
    variables = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(env.scratch / "home"),
        "AWS_CONFIG_FILE": str(env.scratch / "config"),
        "AWS_SHARED_CREDENTIALS_FILE": str(env.scratch / "credentials"),
        "AWS_ACCESS_KEY_ID": ACCESS_KEY,
        "AWS_SECRET_ACCESS_KEY": SECRET_KEY,
        "AWS_DEFAULT_REGION": REGION,
        "AWS_EC2_METADATA_DISABLED": "true",
        "AWS_ENDPOINT_URL": env.stubs[scheme].url,
        "AWS_PAGER": "",
        "AWS_CLI_NATIVE_ROUTER": route,
    }
    if scheme == "https":
        variables["AWS_CA_BUNDLE"] = str(env.ca_bundle)
    if case.fake_models:
        variables["AWS_DATA_PATH"] = str(env.models)
    return variables


def run_once(binary: Path, setting: Setting, route: str, env: Environment) -> dict[str, Any]:
    """One run of the setting's command, measured."""
    case, stub = setting.case, env.stubs[setting.scheme]
    argv = [str(binary), *case.argv, "--endpoint-url", stub.url, "--output", "json"]
    stdout_path = env.scratch / "stdout"
    stderr_path = env.scratch / "stderr"
    stub.reset()
    load = os.getloadavg()[0]
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        started = time.perf_counter()
        child = subprocess.Popen(
            argv,
            env=environment_for(case, setting.scheme, route, env),
            cwd=env.scratch / "work",
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
        timer = threading.Timer(RUN_TIMEOUT_SECONDS, os.killpg, (child.pid, signal.SIGKILL))
        timer.start()
        _, status, usage = os.wait4(child.pid, 0)
        wall = time.perf_counter() - started
        timer.cancel()
    returncode = os.waitstatus_to_exitcode(status)
    counts = stub.counts()
    problem = None
    if returncode != 0:
        problem = f"exited {returncode}"
    elif counts.requests != case.pages:
        problem = f"made {counts.requests} requests for {case.pages} pages"
    if problem is not None:
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-2000:]
        raise SystemExit(
            f"perf case {case.name} ({setting.scheme}, {route} route) {problem}: "
            f"{' '.join(argv)}\n{tail}"
        )
    return {
        "wall_s": wall,
        "user_s": usage.ru_utime,
        "sys_s": usage.ru_stime,
        "max_rss_bytes": usage.ru_maxrss * RSS_UNIT,
        "connections": counts.connections,
        "requests": counts.requests,
        "load_average": load,
    }


def measure(binary: Path, plan: list[Setting], counts: tuple[int, int], env: Environment) -> list[dict[str, Any]]:
    """Every series, measured round-robin."""
    warmup, runs = counts
    recorded: dict[tuple[int, str], list[dict[str, Any]]] = {}
    for number in range(warmup + runs):
        for index, setting in enumerate(plan):
            env.stubs[setting.scheme].load(setting.script)
            for route in ROUTES:
                sample = run_once(binary, setting, route, env)
                if number >= warmup:
                    recorded.setdefault((index, route), []).append(sample)
        print(f"perf: round {number + 1} of {warmup + runs}", file=sys.stderr)
    return [
        {
            "case": setting.case.name,
            "scheme": setting.scheme,
            "latency_ms": setting.latency_ms,
            "route": route,
            "pages": setting.case.pages,
            "runs": recorded[(index, route)],
        }
        for index, setting in enumerate(plan)
        for route in ROUTES
    ]


def cpu_model() -> str:
    if sys.platform == "darwin":
        result = subprocess.run(
            ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"],
            capture_output=True,
            text=True,
            check=False,
        )
        return result.stdout.strip() or platform.processor()
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        for line in cpuinfo.read_text(encoding="utf-8").splitlines():
            name, _, value = line.partition(":")
            if name.strip() in ("model name", "Model"):
                return value.strip()
    return platform.processor() or "unknown"


def prepare(scratch: Path, models: Path) -> Environment:
    for directory in ("home", "work"):
        (scratch / directory).mkdir(parents=True, exist_ok=True)
    (scratch / "config").write_text(CONFIG, encoding="utf-8")
    (scratch / "credentials").write_text("", encoding="utf-8")
    authority = trustme.CA()
    ca_bundle = scratch / "ca.pem"
    authority.cert_pem.write_to_path(str(ca_bundle))
    context = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    authority.issue_cert("127.0.0.1", "localhost").configure_cert(context)
    stubs = {"http": Stub(None), "https": Stub(context)}
    return Environment(scratch, models, ca_bundle, stubs)


def main() -> None:
    args = parse_args()
    data, plan = settings(args.cases)
    env = prepare(args.scratch, args.models.resolve())
    series = measure(args.aws_binary, plan, (data["warmup"], data["runs"]), env)
    samples = {
        "format": FORMAT,
        "cpu": cpu_model(),
        "cpus": os.cpu_count() or 0,
        "warmup": data["warmup"],
        "runs": data["runs"],
        "series": series,
    }
    args.out.write_text(json.dumps(samples, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
