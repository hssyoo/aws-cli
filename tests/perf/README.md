# Performance: a quick look

`cargo xtask perf` runs a few of the AWS CLI's commands against loopback
stub services on the native route and on the Python route, and prints
their medians side by side. It is a sanity look for obvious slowness, not
a gate: it never fails on timing. It takes about a minute.

```sh
cargo xtask build              # perf refuses a missing or stale binary
cargo xtask perf               # the table on stdout, samples in target/perf/result.json
cargo xtask perf --aws-binary path/to/aws --out other.json
```

The binary is the default artifact, as `blackbox` selects it (it must
exist and be no older than any source Cargo built it from), or
`--aws-binary PATH`. Run it as the other CLI suites run (`serial.sh`
across worktrees): it takes the worktree's CLI test lock, and other load
on the machine moves the numbers.

Nothing leaves the machine. Every command runs with the stub as
`--endpoint-url` and as `AWS_ENDPOINT_URL`, fake keys, empty credential
files, a scratch `HOME` and the instance metadata service off.

## What runs

`cases.json` lists the commands, all over HTTPS: a single call per
protocol (`sts get-caller-identity` on query, and the fake models'
`get-thing` on json and rest-json), a json list of 10 pages at 20 ms and
of 100 pages at 0 ms and 20 ms, a 4.4 MB rest-json page, and `s3 ls`.
`run.py` provisions a venv from the blackbox harness's hash-pinned lock
(for `trustme`, which makes the stub's test CA) and runs `measure.py`. It
runs every command on both routes, round-robin, the first round
unrecorded, and the table gives the median of the 5 recorded runs:

| Column | Source |
|---|---|
| wall ms | from just before the spawn to the reap |
| cpu ms, MB | the child's `wait4` resource usage: user plus system time, and peak RSS |
| conns/reqs | connections and requests the stub counted, native then Python |
| load average | the one-minute load average before each run, its range in the heading |

A run that exits non-zero, or makes other than its case's page count of
requests, stops the measurement: its numbers would not be of the command
meant. The connection counts are shown, not judged here; the blackbox
test `api/test_connection_reuse.py` asserts them.

**First run.** On macOS the first run of a freshly built binary costs
about 1.3 s while the system scans the new executable. The warm-up round
absorbs it.

## The stub

`stub.py` serves each case's pages over HTTP/1.1 with keep-alive and
models a network round trip of `rtt` (the case's `latencies_ms`): one
`rtt` after `accept` for TCP's handshake and, over HTTPS, one more for
TLS 1.3's, before anything is read; and one `rtt` before each response.
So a fresh connection's first request costs 2 `rtt` over HTTP and 3 over
HTTPS, and a request on a reused connection 1. A wait sleeps in halving
steps and spins its last 2 ms, so it ends within microseconds of its
deadline despite the timer leeway macOS allows.

## Adding a case

A case is data: the command, whether it needs the fake models under
`tests/models` (`fake_models`, which sets `AWS_DATA_PATH`), its schemes
and latencies, and its response: `pages` pages of `content_type`, each
built from `page` with `$items` (`items_per_page` copies of `item`, joined
by `separator`, `$index` numbering the items across pages) and `$marker`
(`marker` with `$next` the next page's number, on every page but the
last). The native route must be able to run it.
