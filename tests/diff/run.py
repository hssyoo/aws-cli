"""Provision the differential harness's venv and run one of this
directory's scripts in it: `run.py --venv DIR --aws-cli-source DIR SCRIPT
ARGS...`, SCRIPT being `capture.py` or `convert_parity.py`.

`cargo xtask diff` runs this on the host's python3. Any other caller may
too: capture.py's job names the binaries, routes and rows, and it writes
what each row did on each side (see its docstring).

The venv installs the in-tree AWS CLI's runtime lock with the blackbox
harness's hash-pinned test lock (tests/blackbox/requirements.txt), the
pair the blackbox repo suite installs: `localstub`, the TLS stub capture.py
drives, and botocore, which converters and parsed-reply rows use.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path
from typing import Final

# Executed by path: the shared harness modules live one level up.
sys.path.append(str(Path(__file__).resolve().parents[1]))

import harness_venv  # noqa: E402

VENV_DEPS_REVISION: Final = "1"
RUNTIME_LOCK: Final = "requirements/download-deps/system-sandbox-lock.txt"
SCRIPTS: Final = ("capture.py", "convert_parity.py")


def main() -> None:
    harness_venv.validate_host_python("the differential harness")
    parser = argparse.ArgumentParser()
    parser.add_argument("--venv", type=Path, required=True)
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("script", choices=SCRIPTS)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    python = harness_venv.ensure_venv(
        args.venv.resolve(),
        [
            args.aws_cli_source.resolve() / RUNTIME_LOCK,
            here.parent / "blackbox" / "requirements.txt",
        ],
        VENV_DEPS_REVISION,
    )
    # -E and -s keep the caller's PYTHON* variables and user site off the
    # import path.
    command = [
        str(python),
        "-B",
        "-E",
        "-s",
        str(here / args.script),
        *args.arguments,
    ]
    sys.exit(subprocess.run(command, check=False).returncode)


if __name__ == "__main__":
    main()
