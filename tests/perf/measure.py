"""Measure the AWS CLI's commands against the loopback stub.

`cargo xtask perf` runs this through run.py, which provides the venv;
xtask reads the samples it writes and does every statistic. For each case
of the case file, each of its schemes and latencies, and each route, it
runs the command `warmup` times unrecorded and then `runs` times recorded,
the routes taking turns so that drift on the machine falls on both alike.

Each run starts the CLI from a scratch environment (fake keys, empty
credential files, a scratch HOME, the instance metadata service off, the
stub as `--endpoint-url` and `AWS_ENDPOINT_URL`), with stdout and stderr
going to files, and waits for it with `wait4`, whose resource usage gives
the child's peak RSS and CPU user and system time. Wall time runs from
just before the spawn to the reap. A run fails the measurement unless the
CLI exits 0 and asked for exactly the case's pages.
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
    schemes: list[str]
    latencies_ms: list[int]
    pages: int
    script: dict[str, Any]


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
    parser.add_argument("--runs", type=int)
    parser.add_argument("--warmup", type=int)
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--route", action="append", choices=ROUTES)
    return parser.parse_args()


def load_cases(path: Path, selected: list[str]) -> tuple[dict[str, Any], list[Case]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("format") != FORMAT:
        raise SystemExit(f"{path}: expected format {FORMAT}")
    cases = [
        Case(
            name=entry["name"],
            argv=entry["argv"],
            fake_models=entry.get("fake_models", False),
            schemes=entry["schemes"],
            latencies_ms=entry["latencies_ms"],
            pages=entry["response"]["pages"],
            script=entry["response"],
        )
        for entry in data["cases"]
    ]
    names = {case.name for case in cases}
    unknown = [name for name in selected if name not in names]
    if unknown:
        raise SystemExit(f"unknown case {', '.join(unknown)}; cases: {sorted(names)}")
    if selected:
        cases = [case for case in cases if case.name in selected]
    return data, cases


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
            item.substitute(index=number * per_page + offset)
            for offset in range(per_page)
        )
        token = marker.substitute(next=number + 1) if number + 1 < count else ""
        body = page.substitute(items=items, marker=token).encode("utf-8")
        framed.append(frame(200, script["content_type"], body))
    return framed


def environment_for(case: Case, scheme: str, route: str, env: Environment) -> dict[str, str]:
    stub = env.stubs[scheme]
    variables = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(env.scratch / "home"),
        "AWS_CONFIG_FILE": str(env.scratch / "config"),
        "AWS_SHARED_CREDENTIALS_FILE": str(env.scratch / "credentials"),
        "AWS_ACCESS_KEY_ID": ACCESS_KEY,
        "AWS_SECRET_ACCESS_KEY": SECRET_KEY,
        "AWS_DEFAULT_REGION": REGION,
        "AWS_EC2_METADATA_DISABLED": "true",
        "AWS_ENDPOINT_URL": stub.url,
        "AWS_PAGER": "",
        "AWS_CLI_NATIVE_ROUTER": route,
    }
    if scheme == "https":
        variables["AWS_CA_BUNDLE"] = str(env.ca_bundle)
    if case.fake_models:
        variables["AWS_DATA_PATH"] = str(env.models)
    return variables


def run_once(
    binary: Path, case: Case, scheme: str, route: str, env: Environment
) -> dict[str, Any]:
    """One run of the case's command, measured."""
    stub = env.stubs[scheme]
    argv = [str(binary), *case.argv, "--endpoint-url", stub.url, "--output", "json"]
    stdout_path = env.scratch / "stdout"
    stderr_path = env.scratch / "stderr"
    stub.reset()
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        started = time.perf_counter()
        child = subprocess.Popen(
            argv,
            env=environment_for(case, scheme, route, env),
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
    child.returncode = os.waitstatus_to_exitcode(status)
    counts = stub.counts()
    sample = {
        "wall_s": wall,
        "user_s": usage.ru_utime,
        "sys_s": usage.ru_stime,
        "max_rss_bytes": usage.ru_maxrss * RSS_UNIT,
        "connections": counts.connections,
        "requests": counts.requests,
        "response_bytes": counts.response_bytes,
        "stdout_bytes": stdout_path.stat().st_size,
    }
    problem = None
    if child.returncode != 0:
        problem = f"exited {child.returncode}"
    elif counts.requests != case.pages:
        problem = f"made {counts.requests} requests for {case.pages} pages"
    if problem is not None:
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-2000:]
        raise SystemExit(
            f"perf case {case.name} ({scheme}, {route} route) {problem}: "
            f"{' '.join(argv)}\n{tail}"
        )
    return sample


def measure(
    binary: Path, case: Case, routes: list[str], counts: tuple[int, int], env: Environment
) -> list[dict[str, Any]]:
    """Every series of one case: one per scheme, latency and route."""
    warmup, runs = counts
    framed = pages(case.script)
    series = []
    for scheme in case.schemes:
        for latency in case.latencies_ms:
            env.stubs[scheme].load(Script(pages=framed, rtt=latency / 1000))
            load_before = os.getloadavg()
            samples: dict[str, list[dict[str, Any]]] = {route: [] for route in routes}
            for index in range(warmup + runs):
                for route in routes:
                    sample = run_once(binary, case, scheme, route, env)
                    if index >= warmup:
                        samples[route].append(sample)
            load_after = os.getloadavg()
            for route in routes:
                print(f"perf: {case.name} {scheme} {latency} ms {route}", file=sys.stderr)
                series.append(
                    {
                        "case": case.name,
                        "scheme": scheme,
                        "latency_ms": latency,
                        "route": route,
                        "pages": case.pages,
                        "load_average": [load_before[0], load_after[0]],
                        "runs": samples[route],
                    }
                )
    return series


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


def host() -> dict[str, Any]:
    return {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "os": platform.platform(),
        "cpu": cpu_model(),
        "cpus": os.cpu_count() or 0,
    }


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
    data, cases = load_cases(args.cases, args.case)
    routes = args.route or list(ROUTES)
    warmup = data["warmup"] if args.warmup is None else args.warmup
    runs = data["runs"] if args.runs is None else args.runs
    if runs < 1 or warmup < 0:
        raise SystemExit("--runs must be at least 1 and --warmup at least 0")
    env = prepare(args.scratch, args.models.resolve())
    started = time.time()
    series = []
    for case in cases:
        series.extend(measure(args.aws_binary, case, routes, (warmup, runs), env))
    samples = {
        "format": FORMAT,
        "host": host(),
        "started_unix_s": started,
        "warmup": warmup,
        "runs": runs,
        "series": series,
    }
    args.out.write_text(json.dumps(samples, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
