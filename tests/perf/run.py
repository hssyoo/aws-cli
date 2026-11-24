"""Provision the performance harness's venv and run measure.py in it.

`cargo xtask perf` runs this on the host's python3. The stub's test CA
comes from `trustme`, which the repository's blackbox venv already pins,
so the venv installs that same hash-pinned lock
(tests/blackbox/requirements.txt) and nothing else. Every other argument
goes to measure.py.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import Final

# Executed by path: the shared harness modules live one level up.
sys.path.append(str(Path(__file__).resolve().parents[1]))

import harness_venv

VENV_DEPS_REVISION: Final = "1"


def main() -> None:
    harness_venv.validate_host_python("performance tests")
    here = Path(__file__).resolve().parent
    arguments = sys.argv[1:]
    if len(arguments) < 2 or arguments[0] != "--venv":
        raise SystemExit("usage: run.py --venv DIR MEASURE-ARGS...")
    venv = Path(arguments[1]).resolve()
    python = harness_venv.ensure_venv(
        venv,
        [here.parent / "blackbox" / "requirements.txt"],
        VENV_DEPS_REVISION,
    )
    # -E and -s keep the caller's PYTHON* variables and user site off the
    # import path; measure.py imports its sibling stub.py from its own
    # directory.
    command = [str(python), "-B", "-E", "-s", str(here / "measure.py"), *arguments[2:]]
    result = subprocess.run(command, check=False)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
