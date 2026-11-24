"""Measure the AWS CLI's commands against the loopback stub.

`cargo xtask perf` runs this through run.py, which provides the venv;
xtask reads the samples it writes and does every statistic. It measures
one or more binaries side by side (`--binary NAME=PATH`): in each round
every binary runs every series, their order reversed on alternate rounds,
so a comparison of two binaries sees the same machine. A series is a
case of the case file at one of its schemes and latencies, on one route.
The full tier runs every scheme and latency of every case; the quick tier
runs only the pairs each case's `quick` entry names, a subset of those.
The runs go round-robin: each round runs every series once, the routes
taking turns, and the first `warmup` rounds are not recorded. So a passing
load on the machine falls on every series alike, widening each one's
spread, which the gate's noise rule sees, rather than shifting the median
of the few series that happened to run through it.

Each run starts the CLI from a scratch environment (fake keys, empty
credential files, a scratch HOME, the instance metadata service off, the
stub as `--endpoint-url` and `AWS_ENDPOINT_URL`), with stdout and stderr
going to files, and waits for it with `wait4`, whose resource usage gives
the child's peak RSS and CPU user and system time. Wall time runs from
just before the spawn to the reap; the one-minute load average just
before the spawn is recorded with each run, and so is each binary's first
run, which warm-up absorbs. A run fails the measurement unless the CLI
exits 0 and asked for exactly the case's pages, except that a binary named
`--optional` (the base of a comparison) may refuse a series in its first
round: the series is then recorded as refused on it.
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
from dataclasses import dataclass, field
from pathlib import Path
from string import Template
from typing import Any, Final

import trustme

from stub import Script, Stub, frame

# The layout of the case file, and of the samples this writes.
CASES_FORMAT: Final = 1
FORMAT: Final = 2
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
    # The (scheme, latency) pairs of the quick tier; none when the case is
    # in the full tier alone.
    quick: list[tuple[str, int]]


@dataclass(frozen=True)
class Environment:
    """Where every run happens: the scratch files and the stubs."""

    scratch: Path
    models: Path
    ca_bundle: Path
    stubs: dict[str, Stub]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        action="append",
        type=labelled,
        required=True,
        help="NAME=PATH, repeatable: the binaries measured side by side",
    )
    parser.add_argument(
        "--optional",
        action="append",
        default=[],
        help="a binary NAME whose refusal of a series in warm-up marks the "
        "series unavailable on it instead of failing",
    )
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--runs", type=int)
    parser.add_argument("--warmup", type=int)
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--route", action="append", choices=ROUTES)
    parser.add_argument("--tier", choices=("quick", "full"), required=True)
    return parser.parse_args()


def labelled(argument: str) -> tuple[str, Path]:
    name, separator, path = argument.partition("=")
    if not separator or not name:
        raise argparse.ArgumentTypeError(f"expected NAME=PATH, not {argument!r}")
    return name, Path(path)


class RunFailed(Exception):
    """A run that did not exit 0 or asked for other than the case's pages."""


def load_cases(path: Path, selected: list[str]) -> tuple[dict[str, Any], list[Case]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("format") != CASES_FORMAT:
        raise SystemExit(f"{path}: expected format {CASES_FORMAT}")
    cases = [case(entry) for entry in data["cases"]]
    names = {case.name for case in cases}
    unknown = [name for name in selected if name not in names]
    if unknown:
        raise SystemExit(f"unknown case {', '.join(unknown)}; cases: {sorted(names)}")
    if selected:
        cases = [case for case in cases if case.name in selected]
    return data, cases


def case(entry: dict[str, Any]) -> Case:
    """One case of the case file. Its `quick` entry names the schemes and
    latencies the quick tier runs, which must be among the full tier's."""
    quick = entry.get("quick", {})
    pairs = [
        (scheme, latency)
        for scheme in quick.get("schemes", [])
        for latency in quick.get("latencies_ms", [])
    ]
    for scheme, latency in pairs:
        if scheme not in entry["schemes"] or latency not in entry["latencies_ms"]:
            raise SystemExit(
                f"case {entry['name']}: its quick {scheme} {latency} ms is not one "
                "of its full schemes and latencies"
            )
    return Case(
        name=entry["name"],
        argv=entry["argv"],
        fake_models=entry.get("fake_models", False),
        schemes=entry["schemes"],
        latencies_ms=entry["latencies_ms"],
        pages=entry["response"]["pages"],
        script=entry["response"],
        quick=pairs,
    )


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
    load = os.getloadavg()[0]
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
        "load_average": load,
    }
    problem = None
    if child.returncode != 0:
        problem = f"exited {child.returncode}"
    elif counts.requests != case.pages:
        problem = f"made {counts.requests} requests for {case.pages} pages"
    if problem is not None:
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-2000:]
        raise RunFailed(
            f"perf case {case.name} ({scheme}, {route} route) {problem}: "
            f"{' '.join(argv)}\n{tail}"
        )
    return sample


@dataclass(frozen=True)
class Setting:
    """One case at one scheme and latency, with its pages framed."""

    case: Case
    scheme: str
    latency_ms: int
    script: Script


def settings(cases: list[Case], tier: str) -> list[Setting]:
    """The tier's settings: every scheme and latency of every case for
    the full tier, each case's quick pairs for the quick one."""
    chosen = [
        (case, pair)
        for case in cases
        for pair in (
            case.quick
            if tier == "quick"
            else [(scheme, ms) for scheme in case.schemes for ms in case.latencies_ms]
        )
    ]
    return [
        Setting(case, scheme, latency, Script(pages(case.script), latency / 1000))
        for case, (scheme, latency) in chosen
    ]


@dataclass
class Plan:
    """What a measurement runs: the settings, the routes, the binaries in
    their labels' order, and the binaries allowed to refuse a series."""

    settings: list[Setting]
    routes: list[str]
    binaries: list[tuple[str, Path]]
    optional: set[str]


@dataclass
class Tally:
    """The measurement so far: each series' recorded runs, the series a
    binary refused, and each binary's first run."""

    runs: dict[tuple[str, int, str], list[dict[str, Any]]] = field(default_factory=dict)
    refused: dict[tuple[str, int, str], str] = field(default_factory=dict)
    first_run_s: dict[str, float] = field(default_factory=dict)


def run_round(plan: Plan, number: int, recorded: bool, env: Environment, tally: Tally) -> None:
    """One round: every setting once, each route in turn, and each binary
    in turn, the binaries' order reversed on every other round so neither
    always runs first."""
    order = plan.binaries if number % 2 == 0 else plan.binaries[::-1]
    for index, setting in enumerate(plan.settings):
        env.stubs[setting.scheme].load(setting.script)
        for route in plan.routes:
            for name, binary in order:
                key = (name, index, route)
                if key in tally.refused:
                    continue
                try:
                    sample = run_once(binary, setting.case, setting.scheme, route, env)
                except RunFailed as failure:
                    if number > 0 or name not in plan.optional:
                        raise SystemExit(f"{name} binary: {failure}") from failure
                    tally.refused[key] = str(failure).splitlines()[0]
                    continue
                tally.first_run_s.setdefault(name, sample["wall_s"])
                if recorded:
                    tally.runs.setdefault(key, []).append(sample)


def measure(plan: Plan, counts: tuple[int, int], env: Environment) -> dict[str, Any]:
    """Every series, measured round-robin, and the series refused."""
    warmup, runs = counts
    tally = Tally()
    for number in range(warmup + runs):
        run_round(plan, number, number >= warmup, env, tally)
        print(f"perf: round {number + 1} of {warmup + runs}", file=sys.stderr)
    series = []
    refused = []
    for index, setting in enumerate(plan.settings):
        for route in plan.routes:
            for name, _ in plan.binaries:
                identity = {
                    "binary": name,
                    "case": setting.case.name,
                    "scheme": setting.scheme,
                    "latency_ms": setting.latency_ms,
                    "route": route,
                }
                key = (name, index, route)
                if key in tally.refused:
                    refused.append({**identity, "reason": tally.refused[key]})
                    continue
                loads = [run["load_average"] for run in tally.runs[key]]
                series.append(
                    {
                        **identity,
                        "pages": setting.case.pages,
                        "load_average": [min(loads), max(loads)],
                        "runs": tally.runs[key],
                    }
                )
    return {"series": series, "refused": refused, "first_run_s": tally.first_run_s}


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
    chosen = settings(cases, args.tier)
    if not chosen:
        raise SystemExit(f"the {args.tier} tier selects no series from these cases")
    names = [name for name, _ in args.binary]
    if len(set(names)) != len(names) or not set(args.optional) <= set(names):
        raise SystemExit("binary names must differ, and --optional must name one")
    plan = Plan(chosen, routes, args.binary, set(args.optional))
    measured = measure(plan, (warmup, runs), env)
    samples = {
        "format": FORMAT,
        "tier": args.tier,
        "host": host(),
        "started_unix_s": started,
        "warmup": warmup,
        "runs": runs,
        **measured,
    }
    args.out.write_text(json.dumps(samples, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
