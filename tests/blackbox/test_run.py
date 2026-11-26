"""Self-tests for run.py's pytest-xdist handling: no option, from any
source, starts workers that share the host's CRT process lock.

    python3 -B -m unittest discover -s tests/blackbox -p test_run.py
"""

import contextlib
import io
import os
import unittest
from pathlib import Path
from unittest import mock

import run

BASE = ["--aws-binary", "/bin/true", "--binary-kind", "built"]
BASE += ["--aws-cli-source", "aws-cli", "--cache-dir", "/tmp"]


def parse(argv: list[str], addopts: str = "") -> int:
    """run.py's `-n` count for `argv`, or -1 when it refuses them."""
    environ = {**os.environ, "PYTEST_ADDOPTS": addopts}
    with (
        mock.patch.dict(os.environ, environ, clear=True),
        mock.patch("sys.argv", ["run.py", *BASE, *argv]),
        contextlib.redirect_stderr(io.StringIO()),
    ):
        try:
            return run.parse_args()[0].numprocesses
        except SystemExit:
            return -1


class XdistOptions(unittest.TestCase):
    def test_refuses_worker_options_in_the_arguments(self) -> None:
        for argv in (
            ["--tx", "8*popen"],
            ["--tx=8*popen"],
            ["--maxprocesses", "4"],
            ["--looponfail"],
            ["-xn8"],
            ["-vsn4"],
            ["-qd"],
            ["-xf"],
            ["-n", "8", "--dist", "each"],
            ["-n", "8", "--dist=loadfile"],
            ["-n", "8", "-d"],
            ["-n", "2", "-xn2"],
            ["-n", "bogus"],
            ["-n", "-1"],
        ):
            with self.subTest(argv=argv):
                self.assertEqual(parse(argv), -1)

    def test_refuses_worker_options_in_pytest_addopts(self) -> None:
        for addopts in ("-n 8", "-n8", "-qn2", "--numprocesses=auto", "--tx 4*popen"):
            for argv in ([], ["-n", "2"]):
                with self.subTest(addopts=addopts, argv=argv):
                    self.assertEqual(parse(argv, addopts), -1)

    def test_refuses_malformed_pytest_addopts(self) -> None:
        self.assertEqual(parse([], "-k 'unclosed"), -1)

    def test_accepts_other_options(self) -> None:
        for argv, addopts, workers in (
            ([], "", 0),
            (["-n", "8"], "", 8),
            (["-n8"], "", 8),
            (["-x", "-kand_dn", "-rfE"], "-q", 0),
        ):
            with self.subTest(argv=argv, addopts=addopts):
                self.assertEqual(parse(argv, addopts), workers)


class PytestCommand(unittest.TestCase):
    suite = run.Suite("repo", "tests", Path("req.txt"), Path("/cache/venv"))
    python = Path("/cache/venv/bin/python")

    def test_serial_disables_xdist(self) -> None:
        command = run.pytest_command(self.suite, self.python, Path("src"), ["-x"], 0)
        self.assertEqual(command[0], str(self.python))
        self.assertEqual(command[-4:], ["-p", "no:xdist", "-p", "no:xdist.looponfail"])

    def test_workers_and_controller_get_their_own_lock(self) -> None:
        command = run.pytest_command(self.suite, self.python, Path("src"), ["-x"], 3)
        wrapper = str(run.ISOLATED_CRT_LOCK)
        self.assertEqual(command[:2], [wrapper, str(self.python)])
        self.assertNotIn("no:xdist", command)
        self.assertEqual(
            command[-4:],
            ["--dist", "load", "--tx", f"3*popen//python={wrapper} {self.python}"],
        )


if __name__ == "__main__":
    unittest.main()
