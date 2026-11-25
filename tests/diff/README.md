# Differential harness

`cargo xtask diff` runs a corpus of CLI invocations on two sides, A and B,
against the blackbox TLS stub, and compares for each invocation the exit
status, stdout, stderr and every request: method, URL, headers in wire
order, body. It passes when every difference is a known one.

```sh
cargo xtask build                    # the candidate: the default artifact, present and current
cargo xtask diff                     # candidate:native against itself (determinism)
cargo xtask diff --a candidate:native --b candidate:python
cargo xtask diff --a path/to/old/aws:native --b candidate:native
cargo xtask diff --self-test         # the harness must catch a seeded defect
```

A side is `candidate` (the default artifact, or `--aws-binary PATH`) or a
binary's path, then `:native`, `:python` or `:unset`, the value of
`AWS_CLI_NATIVE_ROUTER`, or none. The mode follows from the sides:
`oracle` when the routes differ, `baseline` when only the binaries do, and
`determinism` when they are the same. Other options: `--corpus FILE`
(repeatable; default `corpus/*.jsonl`), `--filter TEXT` (row ids containing
it), `--known FILE` (default `known-differences.json`), `--out DIR`
(default `target/diff/<mode>`).

Nothing leaves the machine. One `localstub` server listens behind its TLS
intercept proxy; every run goes through it (`HTTPS_PROXY`, the proxy's CA as
`AWS_CA_BUNDLE`), so each request keeps the host the CLI chose. Each run has
a cleared environment: fake keys, empty config and credentials files,
`us-west-2`, no instance metadata, no pager (`AWS_PAGER=""`, without which
the native route declines), a scratch HOME and working directory at the same
paths for every run, and stdin empty. Runs time out after 60 s.

## The corpus

`corpus/*.jsonl`, one row per line:

| Field | Meaning |
|---|---|
| `id` | unique across the files read |
| `source` | what the row was made from (optional) |
| `argv` | the arguments after `aws` |
| `env` | variables over the base environment; `null` removes one. The harness owns HOME, the proxy and CA variables, the AWS config and credentials file variables, AWS_DATA_PATH and AWS_CLI_NATIVE_ROUTER |
| `data_path` | AWS_DATA_PATH, relative to the repository root (`tests/models` for the fake models), or absent |
| `responses` | the stub's reply to the run's n-th request: `status` (200), `headers` as ordered pairs, `body` or `body_base64`. The last repeats; past 64 requests the stub closes the connection instead |
| `timeout_s` | overrides the 60 s timeout (optional) |

`seed.jsonl` holds three rows (json, rest-json and query on the fake
models) that the self-test uses.

## What is normalized, and fingerprints

Both sides alike, and nothing else: the SigV4 signature and scope date, the
`X-Amz-Date`, the `amz-sdk-invocation-id` (numbered by first appearance),
the `ttl=` of `amz-sdk-request`, the User-Agent's host tokens (`os/`,
`md/arch`, `md/distrib`, the CRT's `platform/`), the Python route's
`sid/` and the order of its `m/` codes, and generated version 4 UUIDs in
bodies and queries (numbered; those the row gives stay).
`xtask/src/diff/normalize.rs` lists each.

A difference is named by its stage and first differing field, such as
`exit:0->255`, `stdout:$.Items[].When`, `request[*].header:user-agent:lang`
or `request[*].body:Tags.member.#.Key`. `xtask/src/diff/compare.rs` lists
every form.

## Known differences

`known-differences.json` lists the differences accepted: each entry has a
`fingerprint`, the `cases` it holds for (row ids, or `"*"`), an optional
`mode` and a `reason`. It starts empty. A run's `report.json` proposes an
entry, with an empty reason, for each group no entry covers; adding one is
a reviewed change, never the harness's.

## Output

`target/diff/<mode>/` holds `job.json` (what ran), `captures.jsonl` (one
line per run, as recorded) and `report.json`: the sides, the counts
(identical, known, mismatched), the groups by fingerprint with every
instance and both sides' excerpts, the proposed entries, and the known
entries no row used. The summary on stdout gives the counts and one line
per group.

## Reusing the runner

`run.py --venv DIR --aws-cli-source DIR --job FILE` provisions the venv
(the in-tree AWS CLI's runtime lock with the blackbox test lock) and runs
`capture.py`, whose docstring gives the job and capture formats. Another
harness can write a job and read the captures the same way.
