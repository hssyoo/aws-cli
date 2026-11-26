# Differential harness

`cargo xtask diff` runs a corpus of CLI invocations on two sides, A and B,
against the blackbox TLS stub, and compares for each invocation the exit
status, stdout, stderr and every request: method, URL, headers in wire
order, body. It passes when every difference is a known one.

```sh
cargo xtask build                    # the candidate: the default artifact, present and current
cargo xtask diff                     # candidate:native against itself (determinism)
cargo xtask diff --oracle            # candidate:native against candidate:python, botocore's route
cargo xtask diff --a path/to/old/aws:native --b candidate:native
cargo xtask diff --baseline d56b358bb # that commit's binary (built once, cached) against the candidate
cargo xtask diff --check-corpus      # corpus/parity.jsonl must be what its converter writes
cargo xtask diff --self-test         # the harness must catch a seeded defect, and see a stand-in whole
```

A side is `candidate` (the default artifact, or `--aws-binary PATH`) or a
binary's path, then `:native`, `:python` or `:unset`, the value of
`AWS_CLI_NATIVE_ROUTER`, or none. The mode follows from the sides:
`oracle` when the routes differ, `baseline` when only the binaries do, and
`determinism` when they are the same. Other options: `--corpus FILE`
(repeatable; default `corpus/*.jsonl`), `--filter TEXT` (row ids containing
it), `--known FILE` (default `known-differences.json`), `--out DIR`
(default `target/diff/<mode>`).

`--baseline SHA|PATH [--route R]` sets A to a baseline and B to the
candidate, both on route R (`native` by default); it takes neither `--a`,
`--b` nor `--oracle`. A path to a file is the baseline as it is. Otherwise
the commit's binary is built once: `target/diff/baseline/<sha>/src` is a
detached worktree of the commit, built by its own `cargo xtask build` into
`target/diff/baseline/<sha>/target` (the managed dependency store stays
shared), and the artifact is copied to `target/diff/baseline/<sha>/aws`
with `aws.stamp`, the commit and the copy's SHA-256. A later run reuses the
copy while its stamp holds (`diff: baseline <sha> reused`), and builds
again otherwise; it never builds from a worktree at another commit or with
tracked files changed, but names it. An admission change fails in this mode.

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
| `env` | variables over the base environment; `null` removes one. The harness owns HOME, every proxy variable (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, each in either case), AWS_CA_BUNDLE, the AWS config and credentials file variables, AWS_DATA_PATH and AWS_CLI_NATIVE_ROUTER |
| `data_path` | AWS_DATA_PATH, relative to the repository root (`tests/models` for the fake models), or absent |
| `responses` | the stub's reply to the run's n-th request: `status` (200), `headers` as ordered pairs, `body` or `body_base64`. The last repeats; from the 65th request on the stub closes each connection unanswered |
| `timeout_s` | overrides the 60 s timeout; at least 1 (optional) |
| `requests` | `0` for a row that sends no request (optional; no other count). A run that ends having sent none, its row not declaring it, is `requests:declared:none:<side>`, and one that sends a request though its row declares none `requests:declared:0:<side>`; no entry covers either. A run a native side declined, that timed out or that the capture did not see whole is left to its own fingerprint |

`seed.jsonl` holds three rows (json, rest-json and query on the fake
models) that the self-test uses. `parity.jsonl` holds the 86 cases of
`tests/parity/cases`, converted mechanically by `convert_parity.py` (its
docstring gives the rules) into the invocation a user would type: one
argument per input member, settings as arguments and variables, the canned
replies as they are, and `"requests": 0` where the case's call sends no
request. Regenerate it with `run.py --venv target/diff/venv
--aws-cli-source aws-cli convert_parity.py --aws-cli-source aws-cli --cases
tests/parity/cases --models tests/models --out corpus/parity.jsonl`;
`--check-corpus` fails when it drifts. Either run also replays each row on
the in-tree CLI, in process, and fails unless it sends what its case's
call in `tests/parity/oracle.py` sends (method, URL, headers but the
User-Agent, body; the signature by scope and signed headers), so a member
that never reaches the wire, at any binding, fails the conversion. The
check attacks itself first: synthetic evasions it must tell apart, and
each input member of each case dropped and altered, which it must see
wherever botocore's request changes. `standin.jsonl`, outside the corpus,
holds the self-test's rows for `standin.py`, a stand-in CLI that drives the
capture's other paths: a timeout, a request the stub cannot parse, the
environment, the reply sequence, the request limit and a chunked body.

## What is normalized, and fingerprints

Both sides alike, each value only when it has its expected form, and
nothing else: the SigV4 signature (64 lowercase hex digits), which becomes
its verdict, `<sig:ok>` or `<sig:bad>`, as recomputed from the request's
own bytes and the run's fake secret key (`<sig:unverified>` for SigV4a or
a run without a key; a scope date that is not `X-Amz-Date`'s, or a hex
`X-Amz-Content-SHA256` that is not the body's hash, is bad), and the scope date,
an `X-Amz-Date` of the form `YYYYMMDDTHHMMSSZ`, a UUID
`amz-sdk-invocation-id` (numbered by first appearance), a `ttl=` date in
`amz-sdk-request`, the host's release and distribution version in the
User-Agent (`os/linux#<release>`; the family, `md/arch` and the rest stay),
the Python route's `sid/` and the order of its `m/` codes, and generated
version 4 UUIDs in bodies, queries and other headers' values (numbered
across the run by first appearance; those the row gives, in its arguments,
variables or replies, stay). `xtask/src/diff/normalize.rs` lists
each.

A difference is named by its stage and a differing field, such as
`exit:0->255`, `stdout:$.Items[].When`, `stdout:$:order`,
`request[*].header:user-agent:lang` or `request[*].body:Tags.member.#.Key`.
Every check of a stage runs every time on what both sides share (order,
layout, repeats, empty query or form fields), and a stage whose bytes
differ with no check naming why is named `:format`, so one known
difference never hides another and a differing byte always shows.
`xtask/src/diff/compare.rs` lists every form.

A run the capture could not see whole is named `harness:<side>:<kind>`: a
request the stub could not parse (`protocol_error`), a connection the proxy
refused, requests its recorder dropped, a warning the stub logged, output a
stray process held open. Each request also records the hosts the CLI
asked the proxy to CONNECT to (`connects:`) and a chunked body's chunk
sizes and trailers (`body:framing`).

## Not native

A side on the native route that declines a row (exit 255 and only
`aws: AWS_CLI_NATIVE_ROUTER=native: native implementation unavailable
(<reason>); ...`) is not native: the row is not compared, and the report
lists the rows each reason declined. When two native sides disagree (one
declines, or for another reason) the admission changed:
`admission-changed:a|b|reason`, a mismatch no entry covers. Against
another route (the oracle) a refusal is only counted. The report also
counts each side's signatures by verdict, and a bad one on either side, the
row native or not, is `sig:bad:<side>`, a mismatch no entry covers.

## Known differences

`known-differences.json` lists the differences accepted: each entry has a
`fingerprint`, the `cases` it holds for (row ids, or `"*"`), the `mode` it
holds in (`oracle`, `baseline` or `determinism`) and a `reason`. It starts
empty. No entry may name a timeout, a harness error, an admission change,
a bad signature, a broken request declaration or a stage's overflow past its named differences (`:...`):
a row with one is always mismatched. A text difference is grouped by a
masked fingerprint, but an entry covers it only by naming its full lines,
digits and all (the instance's `key`), so a known message never hides
another; a `:bytes` entry must list its rows. A run's `report.json` proposes an entry, with an empty
reason, for each group no entry covers; adding one is a reviewed change,
never the harness's.

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
