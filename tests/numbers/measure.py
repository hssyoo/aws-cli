"""The model client's numbers, rerunnable: timings, model loads, sizes.

    python3 tests/numbers/measure.py [--binary PATH] [--runs N] [--census]

Prints one table per measurement, each with what produced it:

- **Startup**: a native parse failure, `aws sts get-caller-identity
  --no-such-option` (exit 252, the plan's "native local command or parse
  failure"), forced native and forced Python. `aws --version` is no
  measure: every form of it routes to Python (`version_requested`).
- **End to end**: `aws sts get-caller-identity` against a stub on
  loopback, on the native route and the Python route
  (`AWS_CLI_NATIVE_ROUTER=python`): the wall time to exit, and the time
  from spawning the process to the request's first byte reaching the stub.
  The stub checks the route from the User-Agent's `lang/` token. Warm:
  three discarded runs first, then `--runs` (default 21), median and p90.
- **Model load** on the built binary's payload: `model`'s ignored test
  `payload_load_timings`, through `cargo xtask test` in the `release-safe`
  profile.
- **Size**: the binary against the pristine base's 163,904,712 bytes
  (`release-safe` at 1cc2b0482), its sections (`size -m` on macOS,
  `size -A` elsewhere), and the `__text` bytes per crate from `nm`'s
  symbols, by each symbol's first path segment.
- **Eligibility census** (`service-command`'s, builder 3's): its command,
  and with `--census` its counts.

Every command gets a cleared environment, fake keys, empty config and
credentials files, and a loopback endpoint; nothing reaches a real host.
"""
import argparse
import glob
import http.client
import os
import re
import socket
import socketserver
import statistics
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BASE_BYTES = 163_904_712
NEW_CRATES = (
    "model", "client", "command", "format", "service_command",
    "service_errors", "customizations", "sigv4", "timestamp", "sha256",
)
BODY = (
    b"<GetCallerIdentityResponse><GetCallerIdentityResult><Account>123456789012</Account>"
    b"<Arn>arn:aws:iam::123456789012:user/a</Arn><UserId>AIDEXAMPLE</UserId>"
    b"</GetCallerIdentityResult><ResponseMetadata><RequestId>r</RequestId>"
    b"</ResponseMetadata></GetCallerIdentityResponse>"
)


class Stub(socketserver.ThreadingTCPServer):
    """Answers each request with the caller identity, and records when its
    first byte arrived and the User-Agent it carried."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), StubHandler)
        self.arrivals = []

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_address[1]}"


class StubHandler(socketserver.BaseRequestHandler):
    def handle(self):
        connection = self.request
        if not connection.recv(1, socket.MSG_PEEK):
            return
        arrived = time.perf_counter()
        data = b""
        while b"\r\n\r\n" not in data:
            chunk = connection.recv(65536)
            if not chunk:
                return
            data += chunk
        head, _, rest = data.partition(b"\r\n\r\n")
        headers = http.client.parse_headers(
            __import__("io").BytesIO(head.split(b"\r\n", 1)[1] + b"\r\n\r\n")
        )
        length = int(headers.get("Content-Length") or 0)
        while len(rest) < length:
            rest += connection.recv(65536)
        self.server.arrivals.append((arrived, headers.get("User-Agent", "")))
        connection.sendall(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nConnection: close\r\n"
            + f"Content-Length: {len(BODY)}\r\n\r\n".encode()
            + BODY
        )


def environment(home, python):
    for name in ("config", "credentials"):
        open(os.path.join(home, name), "w").close()
    env = {
        "HOME": home,
        "PATH": "/usr/bin:/bin",
        "AWS_ACCESS_KEY_ID": "AKIDEXAMPLE",
        "AWS_SECRET_ACCESS_KEY": "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "AWS_DEFAULT_REGION": "us-east-1",
        "AWS_CONFIG_FILE": os.path.join(home, "config"),
        "AWS_SHARED_CREDENTIALS_FILE": os.path.join(home, "credentials"),
        "AWS_EC2_METADATA_DISABLED": "true",
        "AWS_MAX_ATTEMPTS": "1",
    }
    if python:
        env["AWS_CLI_NATIVE_ROUTER"] = "python"
    return env


def percentiles(samples):
    ordered = sorted(samples)
    p90 = ordered[min(len(ordered) - 1, int(round(0.9 * (len(ordered) - 1))))]
    return statistics.median(ordered) * 1e3, p90 * 1e3


def timed(argv, env, status=0):
    started = time.perf_counter()
    done = subprocess.run(argv, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    if done.returncode != status:
        raise SystemExit(f"{argv[1:]} failed ({done.returncode}): {done.stderr.decode()}")
    return started, time.perf_counter() - started


def measure_routes(binary, runs):
    stub = Stub()
    threading.Thread(target=stub.serve_forever, daemon=True).start()
    rows = []
    with tempfile.TemporaryDirectory() as home:
        for route, python in (("native", False), ("python", True)):
            env = environment(home, python)
            forced = {**env, "AWS_CLI_NATIVE_ROUTER": "python" if python else "native"}
            failure = [binary, "sts", "get-caller-identity", "--no-such-option"]
            version = [timed(failure, forced, 252)[1] for _ in range(3 + runs)][3:]
            call = [binary, "sts", "get-caller-identity", "--endpoint-url", stub.url]
            wall, first_byte = [], []
            for run in range(3 + runs):
                stub.arrivals.clear()
                started, elapsed = timed(call, env)
                (arrived, agent), = stub.arrivals
                expected = "lang/python" if python else "lang/rust"
                if expected not in agent:
                    raise SystemExit(f"{route} route sent {agent!r}")
                if run >= 3:
                    wall.append(elapsed)
                    first_byte.append(arrived - started)
            rows.append((route, percentiles(version), percentiles(wall), percentiles(first_byte)))
    stub.shutdown()
    load = ", ".join(f"{value:.2f}" for value in os.getloadavg())
    print(f"\n## Timings (warm; {runs} runs after 3 discarded; ms, median / p90)\n")
    print(f"Load average when done: {load} on {os.cpu_count()} CPUs; other work on the "
          "machine inflates these.\n")
    print("| route | parse failure, to exit | sts get-caller-identity, to exit | to first byte at the stub |")
    print("|---|---|---|---|")
    for route, version, wall, first in rows:
        cell = lambda pair: f"{pair[0]:.1f} / {pair[1]:.1f}"
        print(f"| {route} | {cell(version)} | {cell(wall)} | {cell(first)} |")


def measure_model_loads(binary):
    env = {**os.environ, "RUST_TEST_NOCAPTURE": "1", "AWS_NATIVE_TIMING_BINARY": binary}
    command = [
        "cargo", "xtask", "test", "-p", "model", "--profile", "release-safe",
        "--filter", "--ignored",
    ]
    done = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
    lines = [line for line in (done.stdout + done.stderr).splitlines()
             if line.startswith("model-load ")]
    print("\n## Model load on the built binary's payload (best of 7; ms)\n")
    print(f"`{' '.join(command)}` with `AWS_NATIVE_TIMING_BINARY={binary}`\n")
    if done.returncode != 0 or len(lines) < 2:
        print(f"failed ({done.returncode}):\n{done.stdout[-2000:]}{done.stderr[-2000:]}")
        return
    print("| service | service-2.json bytes | entry read | JSON parse | arena | `Model::load` |")
    print("|---|---|---|---|---|---|")
    for line in lines[1:]:
        _, service, size, read, parse, arena, load = line.split()
        print(f"| {service} | {int(size):,} | {read} | {parse} | {arena} | {load} |")


# A legacy-mangled symbol's first path segment: `_ZN<len><ident>`, where an
# impl's segment is `<Type as Trait>` and names the type's crate.
SEGMENT = re.compile(r"_?_ZN(\d+)")


def crate_of(symbol):
    match = SEGMENT.match(symbol)
    if not match:
        return "(not Rust)" if not symbol.startswith("__R") else "(v0-mangled)"
    start = match.end()
    ident = symbol[start:start + int(match.group(1))]
    ident = ident.replace("$LT$", "<").replace("..", "::").replace("$u20$", " ")
    ident = ident.lstrip("_<&*").removeprefix("mut ").removeprefix("const ")
    ident = ident.lstrip("<&*[(")
    return re.split(r"::|<| |\$", ident, maxsplit=1)[0] or "(other)"


def measure_size(binary):
    size = os.stat(binary).st_size
    print("\n## Size\n")
    print(f"`{binary}`: {size:,} bytes; pristine base `1cc2b0482` `release-safe`: "
          f"{BASE_BYTES:,}; delta {size - BASE_BYTES:+,} bytes.\n")
    command = ["size", "-m", binary] if sys.platform == "darwin" else ["size", "-A", binary]
    sections = subprocess.run(command, capture_output=True, text=True).stdout
    wanted = [line.strip() for line in sections.splitlines()
              if re.search(r"__text|__pyload|__const|\.text|\.rodata|pyload", line)]
    print(f"`{' '.join(command)}` (selected):\n")
    for line in wanted:
        print(f"    {line}")
    listing = subprocess.run(
        ["nm", "-m", "-n", "--defined-only", binary] if sys.platform == "darwin"
        else ["nm", "-n", "--defined-only", binary],
        capture_output=True, text=True,
    ).stdout
    symbols = []
    for line in listing.splitlines():
        parts = line.split()
        if len(parts) < 3:
            continue
        in_text = "(__TEXT,__text)" in line if sys.platform == "darwin" else parts[1] in "tT"
        if in_text:
            symbols.append((int(parts[0], 16), parts[-1]))
    totals = {}
    for (address, name), (following, _) in zip(symbols, symbols[1:]):
        totals[crate_of(name)] = totals.get(crate_of(name), 0) + following - address
    text_total = sum(totals.values())
    new_total = sum(totals.get(crate, 0) for crate in NEW_CRATES)
    print(f"\n`__text` by crate from `nm` symbol spans (approximate: generic std code "
          f"instantiated for a crate's types counts as std's; inlined code as its caller's):\n")
    print("| crate | text bytes |")
    print("|---|---|")
    for crate in NEW_CRATES:
        print(f"| {crate} | {totals.get(crate, 0):,} |")
    print(f"| **new crates together** | **{new_total:,}** |")
    print(f"| all of `__text` | {text_total:,} |")
    largest = sorted(totals.items(), key=lambda item: -item[1])[:12]
    print("\nThe largest attributions, for where generic code lands:\n")
    print("| crate | text bytes |")
    print("|---|---|")
    for crate, total in largest:
        print(f"| {crate} | {total:,} |")


def census(run):
    command = ["cargo", "xtask", "test", "-p", "service-command", "--filter",
               "every_operation_has_a_standing"]
    print("\n## Eligibility census (service-command)\n")
    print(f"`{' '.join(command)}`, which writes `target/service-eligibility.txt`.\n")
    if run:
        subprocess.run(command, cwd=ROOT, check=True, capture_output=True)
    path = os.path.join(ROOT, "target", "service-eligibility.txt")
    if os.path.exists(path):
        with open(path) as f:
            for line in f.read().splitlines():
                print(f"    {line}")
    else:
        print("(not yet written; pass --census)")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    found = sorted(glob.glob(os.path.join(ROOT, "target", "*", "release-safe", "aws")))
    parser.add_argument("--binary", default=found[0] if found else None)
    parser.add_argument("--runs", type=int, default=21)
    parser.add_argument("--census", action="store_true", help="run the census first")
    args = parser.parse_args()
    if not args.binary:
        raise SystemExit("no release-safe binary; run `cargo xtask build` or pass --binary")
    binary = os.path.abspath(args.binary)
    print(f"# Numbers for `{binary}`")
    measure_routes(binary, args.runs)
    measure_model_loads(binary)
    measure_size(binary)
    census(args.census)


main()
