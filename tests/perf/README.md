# Performance tests

`cargo xtask perf` measures the AWS CLI's commands against loopback stub
services, on the native route and on the Python route, and
`cargo xtask perf --check` holds the result to a committed baseline.

Nothing leaves the machine. Every command runs with the stub as
`--endpoint-url` and as `AWS_ENDPOINT_URL`, fake keys, empty credential
files, a scratch `HOME` and the instance metadata service off.

## Running

```sh
cargo xtask build                    # perf refuses a missing or stale binary
cargo xtask perf                     # every case; table on stdout
cargo xtask perf --check             # ... and judge it against the baseline
cargo xtask perf --case query-single --route native --runs 20
cargo xtask perf --aws-binary path/to/aws --commit abc1234 --out before.json
```

| Option | Meaning |
|---|---|
| `--check` | Judge the result against `baselines/<os>-<arch>.json`. A platform without a baseline is reported and not judged. |
| `--write-baseline` | Write the result as its platform's baseline (see below). |
| `--aws-binary PATH`, `--commit REV` | Measure another binary, such as a build of an earlier commit, and record the commit it came from. Without it the default artifact is measured, as `blackbox` selects it: it must exist and be no older than any source Cargo built it from. |
| `--case NAME`, `--route native\|python` | Only these cases or routes (repeatable). |
| `--runs N`, `--warmup N` | Recorded and unrecorded runs per series, over the case file's (10 and 2). |
| `--out PATH` | The result file; `target/perf/result.json` by default. |

The runs take the worktree's CLI test lock. Across worktrees, run perf as
the other CLI suites run (`serial.sh`), or the CRT's machine-wide process
lock and the extra load both distort the numbers.

## What is measured

`run.py` provisions a venv from the blackbox harness's hash-pinned lock
(for `trustme`, which makes the stub's test CA) and runs `measure.py`. That
starts two stubs (`stub.py`), one HTTP and one HTTPS, and for each case of
`cases.json`, each of its schemes and latencies, it runs the command
`warmup` times unrecorded and then `runs` times recorded, the two routes
taking turns run by run so that drift on the machine falls on both alike.
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
ranks (numpy's default). The one-minute load average before and after
each series is recorded with it.

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

`target/perf/result.json` (format 1) records the platform key, the host
(OS, release, CPU model, CPU count), the binary's path and commit (and
whether the worktree had uncommitted changes to tracked files), the run
counts, and every series with its summary and its raw runs. A baseline is
the same file without the raw runs.

## The gate

`--check` judges each series of the result against the baseline series of
the same case, scheme, latency and route:

| Measurement | Gate | Why |
|---|---|---|
| connections opened, requests made | equal to the baseline's, in every run | Counts have no noise. A change is a behaviour change (a lost connection pool shows here first), so it fails even when the time is within bounds. |
| wall time p50 | may grow by 20%, and at least 5 ms | Two complete runs half an hour apart on the reference machine (an M4 Pro shared with other builds, load average 3 to 9) moved zero-latency medians by 8% typically and 17% at most. A 10% bound failed 2 to 4 series of each run against the other; 20% failed none. The 5 ms floor is several times the spread of a 13 to 20 ms native call (MAD 0.1 to 0.3 ms), so small cases do not fail on a few milliseconds of scheduling. |
| CPU (user + system) p50 | may grow by 20%, and at least 5 ms, on series without injected latency | The same two runs moved zero-latency CPU medians by up to 14%. With injected latency the CPU time of the same command moved by up to 69% between the runs, on both routes: a process that idles through a round trip is rescheduled onto a slower core or a lower clock, so its CPU time follows the machine, not the code. It is reported there, not gated. |
| peak RSS p50 | may grow by 10%, and at least 4 MiB | Peak RSS moved by under 2% between the runs; 10% (about 2 MiB native, 7 to 9 MiB on Python) is past any noise seen, and the floor keeps allocator rounding from failing a small case. |
| p95, throughputs, output size | reported, not gated | The tail of ten runs is a single run; throughputs follow from the median wall time. |

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

A baseline is the result of a complete run (every case, both routes, the
case file's run counts) of the default artifact of a committed head with no
uncommitted changes to tracked files; `--write-baseline` refuses anything
else. On an idle machine of the platform:

```sh
git status --short                   # clean
cargo xtask build
cargo xtask perf --write-baseline    # writes baselines/<os>-<arch>.json
cargo xtask perf --check             # a second run must pass against it
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
   `cargo xtask perf --check --out target/perf/after.json`. Every CHANGED
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
on both routes, so the native route must be able to run it.

## Linux and CI

The harness runs on macOS and Linux (`ru_maxrss` is read in the unit each
reports). Each platform has its own baseline, `<os>-<arch>` as Rust names
it; one without a baseline is reported and not judged.

CI should run it as a job of its own, not inside `cargo xtask test`: a
full run takes several minutes, and timing needs a machine that is doing
nothing else. On a dedicated runner of each platform that has a baseline,
after `cargo xtask build`, run `cargo xtask perf --check` and keep
`target/perf/result.json` as an artifact. A shared or virtualised runner
will often be too noisy to judge; the check says so rather than passing.
