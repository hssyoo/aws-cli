# AGENTS.md: the Rust workspace

This workspace is a Rust port of AWS CLI v2. Commands run on a native route; anything not
yet ported falls back to an embedded Python interpreter running the code in `aws-cli/`.
Output must stay byte-identical to the Python CLI. `aws-cli/AGENTS.md` covers the Python tree only.

## Commands
Run everything from the repository root through `cargo xtask` (the pinned toolchain is in `mise.toml`).

    cargo xtask build                     # the CLI binary every harness below runs
    cargo xtask test                      # workspace tests plus source checks
    cargo xtask test -p <crate> --filter <substring>   # one crate, or one test by name
    cargo xtask lint                      # repository checks beyond rustfmt and clippy
    cargo fmt --check
    cargo xtask guard --base <rev>        # reports changes to protected paths
    cargo xtask diff                      # corpus run: determinism; --oracle compares native with Python
    cargo xtask blackbox -- -n 8          # pytest suites against the built binary, 8 workers
    cargo xtask blackbox -- -k <expr>     # a pytest subset

`tests/diff/README.md` documents `diff`. Run `build` before `diff`, `blackbox` or `perf`.

## Environment traps (Linux)
- `cmake` and a `python` must be on `PATH`, at the versions in `mise.toml`; the CRT and the payload build need them.
- `HOME` must be the physical path. If it is a symlink, blackbox tilde-expansion tests fail.
- Build and test product crates only through `cargo xtask`. Raw `cargo test -p <crate>` skips the link and CRT setup.
- The Python oracle contends for the CRT's process lock. `xtask` serializes CLI suites with
  `target/cli-tests.lock` (`xtask/src/process.rs`); don't run two CLI suites by hand at once.
- Heavy jobs (build, blackbox, diff) are memory-hungry; run one at a time.

## Invariants
- Output, exit status and requests are byte-identical to the Python CLI. `cargo xtask diff --oracle` checks this.
- A refusal (bad arguments, bad configuration) happens before credentials are read or the network is touched.
- Never weaken an oracle to get green: no deleted, skipped or `#[ignore]`d tests, no new suppressions,
  no entries added to an expected-failure list. If an oracle looks wrong, stop and report it.

Mechanical rules live in `cargo xtask lint` (`xtask/src/lint/`, including the crate layering in
`lint/layers.rs`) and `cargo xtask guard`. Read those rather than this file for what is enforced.

## Protected oracle paths
`xtask/guard/protected.txt` lists the paths a change is judged by: expected results, recorded upstream data,
harness code and configuration, toolchain pins. Changes there need explicit approval and land on their own.

## Expected failures
- `tests/blackbox/failures.txt`: native gaps, launcher differences and upstream issues for the blackbox run.
- `tests/blackbox/upstream-failures.txt`: tests the public CLI itself fails.
- `tests/integ/integ_expected_failures.txt`: conformance ledger for `cargo xtask integ`.
- `tests/diff/known-differences.json`: differences `cargo xtask diff` accepts.
Each entry names its reason. Shrink these lists; never grow them to hide a regression.

## Commits
`<crate or area>: <one sentence describing the behaviour>`, with a body for the why. Keep commits small.
