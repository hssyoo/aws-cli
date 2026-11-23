"""cargo xtask parity's Python side: provision the venv, capture botocore's
results for every case into target/parity, and print where they are.

The capture runs in the venv the upstream runtime lock pins, with a cleared,
explicit environment: nothing from the caller's shell reaches botocore, and
nothing is sent. Vectors are keyed by everything that shapes them (the
interpreter, the oracle, the cases, the fake models, the in-tree AWS CLI's
Python sources and the lock), so a change to any of them captures afresh.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

sys.path.append(str(Path(__file__).resolve().parents[1]))
import harness_venv  # noqa: E402

VENV_DEPS_REVISION = "1"
RUNTIME_LOCK = "requirements/download-deps/system-sandbox-lock.txt"


def files(*roots: Path, pattern: str = "*") -> list[Path]:
    found: list[Path] = []
    for root in roots:
        found.extend(path for path in sorted(root.rglob(pattern)) if path.is_file())
    return found


def key(paths: list[Path], interpreter: str) -> str:
    digest = hashlib.sha256(interpreter.encode())
    for path in paths:
        digest.update(str(path).encode())
        digest.update(harness_venv.sha256(path).encode())
    return digest.hexdigest()[:24]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    args = parser.parse_args()
    harness_venv.validate_host_python("the parity capture")
    source = args.aws_cli_source.resolve()
    cache = args.cache_dir.resolve()
    lock = source / RUNTIME_LOCK
    python = harness_venv.ensure_venv(cache / "venv", [lock], VENV_DEPS_REVISION)
    here = Path(__file__).resolve().parent
    inputs = [here / "oracle.py", here / "run.py", lock]
    inputs += files(args.cases.resolve(), args.models.resolve())
    inputs += files(source / "awscli", pattern="*.py")
    vectors = cache / "vectors" / f"{key(inputs, harness_venv.host_python_id())}.json"
    if not vectors.exists():
        capture(python, source, cache, args, vectors)
    print(json.dumps({"vectors": str(vectors), "python": str(python)}))


def capture(python: Path, source: Path, cache: Path, args, vectors: Path) -> None:
    home = cache / "home"
    home.mkdir(parents=True, exist_ok=True)
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "TZ": "UTC",
        "LC_ALL": "C.UTF-8",
        "PYTHONHASHSEED": "0",
        "PYTHONPYCACHEPREFIX": str(cache / "pycache"),
        "AWS_CONFIG_FILE": "/dev/null",
        "AWS_SHARED_CREDENTIALS_FILE": "/dev/null",
        "AWS_EC2_METADATA_DISABLED": "true",
    }
    partial = vectors.with_suffix(".partial")
    partial.parent.mkdir(parents=True, exist_ok=True)
    command = [
        str(python), "-I", "-B", str(Path(__file__).resolve().parent / "oracle.py"),
        "--aws-cli-source", str(source), "--cases", str(args.cases.resolve()),
        "--models", str(args.models.resolve()), "--out", str(partial),
    ]
    completed = subprocess.run(command, env=env, timeout=1200, check=False)
    if completed.returncode != 0:
        raise SystemExit(f"parity capture failed with exit code {completed.returncode}")
    partial.replace(vectors)


if __name__ == "__main__":
    main()
