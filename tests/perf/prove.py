"""Prove what `cargo xtask perf --check` catches, and what it lets pass.

Run from the repository root, after `cargo xtask build`, on the host's
python3:

    python3 tests/perf/prove.py regression [--per-page-us N ...]
    python3 tests/perf/prove.py load [--busy N]

`regression` builds `HEAD` with `planted/paging-cost.patch` applied, its
`@PER_PAGE_US@` set to N, which makes the paginator busy-wait N
microseconds per page, and checks that build against the unpatched
default artifact, in the same rounds, once the load average has settled
after the build. The proof holds when the check fails, and fails only
on paged native series: the planted cost is in the native paginator, so
the Python route and the single calls must not move. Each size is a
separate build (incremental after the first) under `target/perf/proof/`.

`load` checks the default artifact against a copy of itself while N busy
loops (one per CPU by default) load the machine. The proof holds when the
check passes or says it could not judge (exit 2: the machine was loaded,
or the runs too noisy); it fails if the check ever reports a regression.

Each writes its result files under `target/perf/proof/` and exits 0 when
the proof holds.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Final

ROOT: Final = Path(__file__).resolve().parents[2]
PROOF: Final = ROOT / "target" / "perf" / "proof"
PATCH: Final = Path(__file__).resolve().parent / "planted" / "paging-cost.patch"
# The file the patch plants its cost in.
PLANTED_FILE: Final = Path("crates/client/src/paginate.rs")
# The series the planted cost is in: the native route of paged cases.
PAGED_NATIVE: Final = ("-pages-", " native ")
SETTLE_SECONDS: Final = 30
# After a build, a check waits until the one-minute load average is under
# a quarter of the CPUs, for at most this long.
QUIET_SECONDS: Final = 300


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    print(f"prove: {' '.join(command)}", file=sys.stderr, flush=True)
    return subprocess.run(command, text=True, check=False, **kwargs)  # type: ignore[call-overload]


def checked(command: list[str], **kwargs: object) -> str:
    result = run(command, capture_output=True, **kwargs)
    if result.returncode != 0:
        raise SystemExit(f"{' '.join(command)} failed:\n{result.stderr}")
    return result.stdout.strip()


def default_artifact() -> Path:
    """The default artifact `cargo xtask build` made."""
    found = sorted(ROOT.glob("target/*/release-safe/aws"))
    if len(found) != 1:
        raise SystemExit(f"expected one default artifact, found {found}; run cargo xtask build")
    return found[0]


def kept_copy(binary: Path, name: str) -> Path:
    """A copy of `binary` the comparison's base can be, apart from the
    artifact a build may replace."""
    PROOF.mkdir(parents=True, exist_ok=True)
    copy = PROOF / name
    shutil.copy2(binary, copy)
    return copy


def planted_build(commit: str, per_page_us: int) -> Path:
    """`commit` built with the planted paging cost of `per_page_us`."""
    source = PROOF / "src"
    if source.exists():
        shutil.rmtree(source)
    source.mkdir(parents=True)
    archive = subprocess.Popen(
        ["git", "archive", "--format=tar", commit], cwd=ROOT, stdout=subprocess.PIPE
    )
    unpacked = subprocess.run(["tar", "-x", "-C", str(source)], stdin=archive.stdout, check=False)
    if archive.wait() != 0 or unpacked.returncode != 0:
        raise SystemExit(f"cannot export {commit}")
    patch = PATCH.read_text(encoding="utf-8").replace("@PER_PAGE_US@", str(per_page_us))
    # `patch`, not `git apply`: the export lies inside this repository's
    # worktree, and `git apply` run there silently skips every path
    # outside the export's directory, which is all of them.
    applied = run(["patch", "-p1", "--forward", "--batch"], cwd=source, input=patch)
    planted = f"from_micros({per_page_us})"
    if applied.returncode != 0 or planted not in (source / PLANTED_FILE).read_text("utf-8"):
        raise SystemExit(f"{PATCH} does not apply to {commit}; refresh it")
    target = PROOF / "target"
    env = {**os.environ, "CARGO_TARGET_DIR": str(target)}
    if run(["cargo", "xtask", "build"], cwd=source, env=env).returncode != 0:
        raise SystemExit("the planted build failed")
    built = sorted(target.glob("*/release-safe/aws"))
    if len(built) != 1:
        raise SystemExit(f"expected one planted artifact, found {built}")
    shutil.rmtree(source)
    return kept_copy(built[0], f"aws-planted-{per_page_us}us")


def settle() -> None:
    """Wait for the load average a build left to fall under a quarter of
    the CPUs, or `QUIET_SECONDS`."""
    quiet = (os.cpu_count() or 4) / 4
    deadline = time.monotonic() + QUIET_SECONDS
    while os.getloadavg()[0] > quiet and time.monotonic() < deadline:
        time.sleep(5)
    print(f"prove: load average {os.getloadavg()[0]:.2f} before the check", file=sys.stderr)


def check(arguments: list[str], out: Path) -> tuple[int, str]:
    """`cargo xtask perf --check` with `arguments`: its exit status and
    its output."""
    command = ["cargo", "xtask", "perf", "--check", *arguments, "--out", str(out)]
    result = run(command, cwd=ROOT, capture_output=True)
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    return result.returncode, result.stdout


def regression(per_page_us: list[int]) -> bool:
    commit = checked(["git", "rev-parse", "HEAD"], cwd=ROOT)
    base = kept_copy(default_artifact(), "aws-base")
    holds = True
    builds = {size: planted_build(commit, size) for size in per_page_us}
    contents = {path.read_bytes() for path in [base, *builds.values()]}
    if len(contents) != len(builds) + 1:
        raise SystemExit("a planted build is the same binary as another: the plant did not apply")
    for size, planted in builds.items():
        settle()
        status, output = check(
            [
                "--aws-binary", str(planted), "--commit", f"{commit} with {size} us per page planted",
                "--base-binary", str(base), "--base-commit", commit,
            ],
            PROOF / f"regression-{size}us.json",
        )
        regressed = [line for line in output.splitlines() if "REGRESSED" in line]
        elsewhere = [line for line in regressed if not all(part in line for part in PAGED_NATIVE)]
        caught = status == 1 and bool(regressed) and not elsewhere
        print(f"prove: {size} us per page planted: exit {status}; "
              f"{len(regressed)} regressed, {len(elsewhere)} outside paged native series: "
              f"{'caught' if caught else 'NOT caught'}")
        for line in regressed:
            print(f"prove:   {line}")
        holds = holds and caught
    return holds


def load(busy: int) -> bool:
    commit = checked(["git", "rev-parse", "HEAD"], cwd=ROOT)
    base = kept_copy(default_artifact(), "aws-itself")
    loops = [subprocess.Popen([sys.executable, "-c", "while True: pass"]) for _ in range(busy)]
    try:
        print(f"prove: {busy} busy loops running; settling {SETTLE_SECONDS} s", file=sys.stderr)
        time.sleep(SETTLE_SECONDS)
        before = os.getloadavg()[0]
        status, output = check(
            ["--base-binary", str(base), "--base-commit", commit], PROOF / f"load-{busy}.json"
        )
        after = os.getloadavg()[0]
    finally:
        for loop in loops:
            loop.kill()
            loop.wait()
    verdict = {0: "passed", 1: "FAILED", 2: "could not judge"}.get(status, f"exited {status}")
    loaded = sum("MACHINE LOADED" in line for line in output.splitlines())
    noisy = sum("TOO NOISY" in line for line in output.splitlines())
    print(f"prove: under {busy} busy loops (load average {before:.1f} to {after:.1f}) the "
          f"check {verdict}: {loaded} findings machine loaded, {noisy} too noisy")
    return status in (0, 2)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    proofs = parser.add_subparsers(dest="proof", required=True)
    planted = proofs.add_parser("regression")
    planted.add_argument("--per-page-us", type=int, action="append")
    loaded = proofs.add_parser("load")
    loaded.add_argument("--busy", type=int, default=os.cpu_count() or 1)
    args = parser.parse_args()
    if args.proof == "regression":
        holds = regression(args.per_page_us or [15])
    else:
        holds = load(args.busy)
    print(f"prove: the proof {'holds' if holds else 'DOES NOT HOLD'}")
    sys.exit(0 if holds else 1)


if __name__ == "__main__":
    main()
