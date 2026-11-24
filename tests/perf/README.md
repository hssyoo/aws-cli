# Performance tests

`cargo xtask perf` measures the AWS CLI's commands against loopback stub
services, on the native route and on the Python route, and
`cargo xtask perf --check` holds the result to a committed baseline.

There are two tiers:

| Tier | Runs | Series | Time on the reference machine | Use |
|---|---|---|---|---|
| quick (default) | each case's `quick` series: a single call per protocol at 0 ms, a 10-page and a 100-page json case at 20 ms, the 4.4 MB page and `s3 ls`, all over HTTPS | 14 | 71 s of measuring, about 1.5 min with startup | the gate on every change: `cargo xtask perf --check` |
| full (`--full`) | every scheme and latency of every case | 98 | 473 s, about 8 min | perf-focused changes, and the baseline |

The reference machine is an Apple M4 Pro (12 CPUs) on macOS 27.

Nothing leaves the machine. Every command runs with the stub as
`--endpoint-url` and as `AWS_ENDPOINT_URL`, fake keys, empty credential
files, a scratch `HOME` and the instance metadata service off.

## Running

```sh
cargo xtask build                    # perf refuses a missing or stale binary
cargo xtask perf                     # the quick tier; table on stdout
cargo xtask perf --check             # ... and judge it against the baseline
cargo xtask perf --check --full      # the whole matrix, judged
cargo xtask perf --case query-single --route native --runs 20
cargo xtask perf --aws-binary path/to/aws --commit abc1234 --out before.json
```

| Option | Meaning |
|---|---|
| `--check` | Judge the result against its tier's section of `baselines/<os>-<arch>.json`. A platform without a baseline is reported and not judged. |
| `--full` | The full tier instead of the quick one. |
| `--write-baseline` | Measure the full tier and then the quick tier, and write both as the platform's baseline (see below). |
| `--aws-binary PATH`, `--commit REV` | Measure another binary, such as a build of an earlier commit, and record the commit it came from. Without it the default artifact is measured, as `blackbox` selects it: it must exist and be no older than any source Cargo built it from. |
| `--case NAME`, `--route native\|python` | Only these cases or routes (repeatable). |
| `--runs N`, `--warmup N` | Recorded and unrecorded runs per series, over the case file's (10 and 2). |
| `--out PATH` | The result file; `target/perf/result.json` by default. `--write-baseline` writes the quick tier's beside it, as `<name>.quick.json`. |

The runs take the worktree's CLI test lock. Across worktrees, run perf as
the other CLI suites run (`serial.sh`), or the CRT's machine-wide process
lock and the extra load both distort the numbers.

## What is measured

`run.py` provisions a venv from the blackbox harness's hash-pinned lock
(for `trustme`, which makes the stub's test CA) and runs `measure.py`. That
starts two stubs (`stub.py`), one HTTP and one HTTPS. A series is a case
of `cases.json` at one of its schemes and latencies, on one route; the full
tier runs every one, the quick tier the pairs each case's `quick` entry
names. The runs go round-robin: each round runs every series once, the two routes
taking turns, and the first `warmup` rounds are not recorded. A passing
load on the machine (a build, another suite) then falls on every series
alike and shows as spread, which the gate's noise rule sees, instead of
shifting the medians of the few series that happened to run through it.
The route is forced with `AWS_CLI_NATIVE_ROUTER` (`native` or `python`): a
case the native route cannot run fails the measurement; it is never
silently measured on Python.

For each run:

| Measurement | Source |
|---|---|
| wall time | from just before the spawn to the reap |
| CPU user and system time, peak RSS | the child's `wait4` resource usage (`ru_utime`, `ru_stime`, `ru_maxrss`), not sampling |
| connections opened, requests made, response bytes | counted by the stub |
| output bytes | the size of the command's stdout, which goes to a file |

A run fails the measurement unless the CLI exits 0 and makes exactly the
case's page count of requests.

Each series (case, scheme, latency, route) is summarised by `cargo xtask
perf`: p50 and p95 of wall time, CPU time (user plus system), user and
system time apart, and peak RSS, each with its median absolute deviation;
requests/s and pages/s over the median wall time; MB/s (10^6 bytes of
response) over the median wall time; and the least and greatest
connection and request counts. Percentiles interpolate linearly between
ranks (numpy's default). The least and greatest one-minute load
average seen before a series' runs are recorded with it.

### Injected latency

The stub models a network round trip of `rtt` (the case's `latencies_ms`)
where one would fall:

- **Connection setup.** After `accept` and before reading anything (so
  before the TLS handshake), the stub waits one `rtt` for TCP's handshake
  and, over HTTPS, one more for TLS 1.3's, whatever version is negotiated.
  This is the cost a reused connection saves.
- **Each request.** After its last byte is read and before the response's
  first byte is written, the stub waits one `rtt`.

So a fresh connection's first request costs 2 `rtt` over HTTP and 3 over
HTTPS, and a request on a reused connection 1. The stub keeps a connection
open for further requests until the client closes it or asks for
`Connection: close`, as an HTTP/1.1 server does.

## The result file

`target/perf/result.json` (format 2) records the platform key, the host
(OS, release, CPU model, CPU count), the binary's path (relative to the
checkout when it lies inside it) and commit (and whether the worktree had
uncommitted changes to tracked files), the tier, the run counts, and every
series with its summary and its raw runs.

A platform's baseline, `baselines/<os>-<arch>.json`, is one file with two
sections, `quick` and `full`: a record of each tier, without the raw runs.
The quick series are a subset of the full ones, with the same ids and run
counts, but each tier is measured on its own, because a series' median
depends on the series it runs among. In the full tier's rounds, dominated
by long runs that idle through injected round trips, the short native
calls (about 14 ms) ran 15 to 17% slower, in wall and CPU time alike, than
in the quick tier's, most likely because a short burst after idle time
lands on a slower core or a lower clock. The Python calls (about 100 ms) differed by 1%, and the paged and
large cases by 2% or less. Judging a quick run against full-tier medians would
therefore report false improvements past the quick tier's tolerances.

## The gate

`--check` judges each series of the result against the series of the same
case, scheme, latency and route in its tier's section of the baseline:

| Measurement | Gate |
|---|---|
| connections opened, requests made | equal to the baseline's, in every run: counts have no noise, and a change is a behaviour change (a lost connection pool shows here first), so it fails even when the time is within bounds |
| wall time p50 | may grow by the tier's bound below |
| CPU (user + system) p50 | may grow by the tier's bound, on series without injected latency only |
| peak RSS p50 | may grow by the tier's bound |
| p95, throughputs, output size | reported, not gated: the tail of ten runs is a single run, and throughputs follow from the median wall time |

| Tier | wall p50 | CPU p50 (0 ms series) | peak RSS p50 |
|---|---|---|---|
| quick | 10%, and at least 2 ms | 5%, and at least 2 ms | 5%, and at least 1 MiB |
| full | 10%, and at least 3 ms | 10%, and at least 3 ms | 5%, and at least 1 MiB |

**Where the bounds come from.** One binary (d2769be4f) was measured five
times in the quick tier and three times in the full tier, interleaved over
forty minutes on the reference machine with nothing else running (load
average 1.1 to 1.6 at each start, after 6.9 at the first from the runs
before it). For each series and metric, the spread of its median is
the largest minus the smallest across those runs:

| Tier | Metric | Series | Spread of the median: p50, p90, max | Largest absolute spread on a series under 50 ms |
|---|---|---|---|---|
| quick | wall | 14 | 0.7%, 1.9%, 3.0% | 0.64 ms |
| quick | CPU (0 ms) | 10 | 0.5%, 1.1%, 1.1% | 0.35 ms |
| quick | RSS | 14 | 0.2%, 0.3%, 0.5% | 0.11 MiB (any size) |
| full | wall | 98 | 0.4%, 1.6%, 2.9% | 0.81 ms |
| full | CPU (0 ms) | 36 | 0.7%, 2.7%, 3.2% | 0.73 ms |
| full | RSS | 98 | 0.1%, 0.3%, 0.7% | 0.18 MiB (any size) |

Each bound is three times the largest spread, rounded up to the next 5%,
and each floor three times the largest absolute spread, rounded up to the
next millisecond or MiB. Three times covers a check comparing two medians
that each carry this error, and the few repeats understating the spread.
Replayed with these bounds, every quick run judged against every other
passes, the closest finding at 35% of its limit, and both later full runs
pass against the first, the closest at 34%.

**CPU time with injected latency** is not gated. In earlier runs the same
command's CPU time moved by up to 69% between runs on both routes: a
process that idles through a round trip is rescheduled onto a slower core
or a lower clock, so its CPU time follows the machine, not the code.

**The machine must be quiet.** The bounds hold only on a machine doing
nothing else. On this machine under other builds (load average 3 to 9),
earlier runs moved medians by 8% typically and 17% at most between runs, and
the gate would fail. The recorded load average says when that was so.

A growth past both bounds fails. A shrinkage past them is reported as an
improvement, which asks for the baseline to be refreshed.

**Noise.** Each median's standard error is estimated from its deviation
(1.2533 × 1.4826 × MAD / √n). When twice the standard error of the
difference between the two medians is wider than the allowed growth, the
runs cannot tell a regression from noise: the check reports the series as
**too noisy to judge**, with its load average, and fails, because the gate
cannot vouch for it. Rerun on a quieter machine, or with more `--runs`.

**Other machines.** Times and RSS are judged only against a baseline taken
on the same CPU model (the host's `cpu`); on another, only the counts are
judged and the rest is reported as not judged. A series the baseline lacks
is reported as new.

## Refreshing a baseline

A baseline is a complete run of both tiers (every case, both routes, the
case file's run counts) of the default artifact of a committed head with no
uncommitted changes to tracked files; `--write-baseline` refuses anything
else. It takes about 10 minutes. On an idle machine of the platform:

```sh
git status --short                   # clean
cargo xtask build
cargo xtask perf --write-baseline    # full then quick: baselines/<os>-<arch>.json
cargo xtask perf --check             # a second quick run must pass against it
cargo xtask perf --check --full      # and a second full run
git add tests/perf/baselines && git commit
```

Refresh it when a change makes the CLI faster (the check says so), when a
case is added or changed, and when the reference machine changes. Say in
the commit why the numbers moved.

**A change meant to move the numbers.** A change that alters what the CLI
does on the wire, such as reusing connections across pages, fails the
check against the old baseline: the connection counts it changes are
reported as CHANGED (they are judged exactly), and the times it improves as
improved. That failing check is the before-and-after judgement, so keep it:

1. On the change's head, before refreshing anything, run
   `cargo xtask perf --check --full --out target/perf/after.json` (and the
   quick `cargo xtask perf --check`). Every CHANGED
   and improved line must be one the change intends (for connection reuse:
   one connection where there were one per page, and shorter times on the
   paged and latency cases), and nothing may be REGRESSED.
2. Keep that output (it names both commits) for the change's record.
3. Then refresh the baseline on that same head as above, and commit it with
   the counts that changed and why in the message. From then on `--check`
   holds the new behaviour.

## Adding a case

A case in `cases.json` is data: the command, whether it needs the fake
models under `tests/models` (`fake_models`, which sets `AWS_DATA_PATH`),
its schemes and latencies, and its response. The response is `pages`
pages of `content_type`, each built from `page` with `$items` (the page's
`items_per_page` copies of `item`, joined by `separator`, with `$index`
numbering the items from 0 across pages) and `$marker` (`marker` with
`$next` the next page's number, on every page but the last). The case runs
on both routes, so the native route must be able to run it. A `quick`
entry (`schemes` and `latencies_ms`, among the case's own) puts those
series in the quick tier too; keep that tier to a couple of minutes.

## Linux and CI

The harness runs on macOS and Linux (`ru_maxrss` is read in the unit each
reports). Each platform has its own baseline, `<os>-<arch>` as Rust names
it; one without a baseline is reported and not judged.

CI should run it as a job of its own, not inside `cargo xtask test`:
timing needs a machine that is doing nothing else. On a dedicated runner of
each platform that has a baseline, after `cargo xtask build`, run
`cargo xtask perf --check` (quick) on every change and
`cargo xtask perf --check --full` for changes to the client, the transport
or the paginator, and keep `target/perf/result.json` as an artifact. A
shared or virtualised runner will often be too noisy to judge; the check
says so rather than passing.
