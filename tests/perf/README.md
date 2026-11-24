# Performance tests

`cargo xtask perf` measures the AWS CLI's commands against loopback stub
services, on the native route and on the Python route. Its gate for a
change, `cargo xtask perf --check`, runs the change's binary (the head)
and a base binary interleaved in the same rounds and judges the head on
its ratio to the base, so whatever the machine does to both cancels.

Nothing leaves the machine. Every command runs with the stub as
`--endpoint-url` and as `AWS_ENDPOINT_URL`, fake keys, empty credential
files, a scratch `HOME` and the instance metadata service off.

## Running

```sh
cargo xtask build                    # perf refuses a missing or stale binary
cargo xtask perf --check             # the gate: HEAD against the main line's merge-base
cargo xtask perf --check --base abc1234
cargo xtask perf --check --base-binary path/to/old/aws --base-commit abc1234
cargo xtask perf --check --full      # the whole matrix
cargo xtask perf                     # report the numbers; judge nothing
cargo xtask perf --check-baseline    # absolute tracking against the stored record
```

| Option | Meaning |
|---|---|
| `--check` | Compare the head with the base in the same rounds (below). |
| `--base REV`, `--base-binary PATH` (`--base-commit REV`), `--mainline REF` | The base: a build of `REV`, a binary as given, or the merge-base of `HEAD` with the main line. |
| `--check-baseline` | Judge the binary against its tier's stored record in the platform's baseline file. |
| `--write-baseline` | Measure the full tier and then the quick tier, and store both records. |
| `--calibrate` (`--repeats N`) | Derive the tier's tolerances and store them (below). |
| `--full` | The full tier instead of the quick one. |
| `--aws-binary PATH`, `--commit REV` | The head: another binary than the default artifact, and the commit it came from. The default artifact is the one `blackbox` selects: it must exist and be no older than any source Cargo built it from. |
| `--case NAME`, `--route native\|python` | Only these cases or routes (repeatable). |
| `--runs N`, `--warmup N` | Recorded and unrecorded rounds, over the case file's (10 and 2). A judged run needs at least one warm-up round. |
| `--out PATH` | The result file; `target/perf/result.json` by default. |

A check exits 0 when it passes, 1 when a finding fails (a regression, a
changed count, a missing series) and 2 when it could not judge (the
machine was loaded, or the runs too noisy); it prints why.

The runs take the worktree's CLI test lock. Across worktrees, run perf as
the other CLI suites run (`serial.sh`), or the CRT's machine-wide process
lock and the extra load both distort the numbers.

## The tiers

| Tier | Series | What | Time (one binary / a comparison) |
|---|---|---|---|
| quick (default) | 16 per binary | each case's `quick` series, all over HTTPS: a single call per protocol at 0 ms; `json-pages-10` at 20 ms; `json-pages-100` at 0 ms (the paginator's own cost, CPU gated) and at 20 ms (connection reuse); the 4.4 MB page; `s3 ls` | about 1.2 min / 2.5 min |
| full (`--full`) | 98 per binary | every scheme and latency of every case | about 8 min / 16 min |

## The gate for a change: `--check`

**Base.** `--base-binary PATH` as given; else a build of `--base REV`;
else the merge-base of `HEAD` with the main line. The main line is never
guessed: it is `--mainline REF`, else the git setting `perf.mainline`
(`git config perf.mainline main` names it once for a checkout with no
remote), else the `origin` remote's default branch
(`refs/remotes/origin/HEAD`); with none, the check stops and says how to
name one. A commit is exported with `git archive` under
`target/perf/base/<commit>/`, built there by its own `cargo xtask build`
into a target directory every base build shares, and kept as
`target/perf/base/<commit>/aws`, so the next check against it does not
build again.

**Rounds.** Each round runs every series of both binaries, the two
binaries back to back for each series and route, and which goes first
alternates from round to round.

**Verdicts.**

| Measurement | Judged |
|---|---|
| connections opened, requests made | exactly: equal to the base's in every run. A count has no noise; a changed one (a lost connection pool, an extra request) fails on any machine. |
| wall time | the median of the per-round log ratios head/base, scaled to the base's median, against the calibrated tolerance |
| CPU (user + system) | the same, on series without injected latency only: a process that idles through a round trip is rescheduled onto a slower core or a lower clock, so its CPU time follows the machine |
| peak RSS | the least and the greatest peak of the runs, together (below) |
| p95, throughputs, output size | reported, not judged |

A ratio whose 95% resolution (twice its standard error, from the median
absolute deviation of the log ratios) is wider than the tolerance is
**too noisy to judge**. A series the base refuses (a feature the base did
not have) is reported and not judged.

**Peak RSS.** A regression in peak memory (a larger buffer, pages kept
alive, a leak) adds to every run's peak, so it raises every mode the runs
fall into, the lowest and the highest alike. Memory that depends on
timing (a buffer that grows while its reader lags, a reallocation that
cannot grow in place) splits the runs between modes without the code
changing. The share of runs in each mode moves the median and the p95, and
a mode only a few runs reach appears in one run set and not another,
moving the least or the greatest peak, but not both. Two builds of one
source measured here gave least peaks of 51.4 and 56.0 MiB, one run of one
of them in the lower mode. So the peak regresses only when both its least
and its greatest grow past the tolerance, by the smaller of the two. It
misses a regression that happens in only some runs.

**Load.** Each run records the one-minute load average before it. A
series whose runs' median load is above the calibration's highest plus one
per CPU (12 on the reference machine) has its times and memory reported
as **machine loaded**, not judged; its
counts are still judged. The paired ratios cancel what load does to both
binaries alike, which the load proof below shows.

**What it catches.** Each check prints the tolerances it judges with.
The quick calibration taken at 5286fd237 on the reference machine, an Apple
M4 Pro (12 CPUs), ran under a load average of 1.4 to 16.7 (other work was
running), and gave:

| Quick comparison | Tolerance | On `json-pages-100` at 0 ms, native (31.8 ms wall, 21.6 ms CPU) |
|---|---|---|
| wall | 20%, at least 7.5 ms | +7.5 ms (24%), 75 µs per page |
| CPU | 1%, at least 5.0 ms | +5.0 ms (23%), 50 µs per page |
| peak RSS | 1%, at least 3.0 MiB | +3.0 MiB at both ends |

Those bounds reflect the load the calibration saw; a calibration on a
quiet machine is tighter (two quiet sessions earlier moved medians by 3%
at most, which this rule makes about 10%). The current numbers are always
those in the baseline file, which the check prints. On the 14 ms single
calls the floor is the bound: a native call must slow by 7.5 ms wall or
5 ms CPU to fail. Counts catch a lost pool or an extra request at any
size.

## Calibration: `--calibrate`

`cargo xtask perf --calibrate` (quick) and `--calibrate --full` run the
binary against itself, as base and head in the same rounds, 5 times
(quick) or 3 times (full; `--repeats N` overrides), and write the tier's
tolerances into the platform's baseline file,
`baselines/<os>-<arch>.json`, with the machine, its load range, the
commit and the date. The gate reads them there and nowhere else; a tier
or platform without a calibration has its counts judged and its times and
memory reported only.

The rule: since the code is the same, every deviation is noise.

- **For the comparison**, each repeat's estimate of the head against the
  base: the ratio for wall and CPU, the shift both ends of the peaks share
  for RSS.
- **For the stored baseline**, the spread of every measurement's median:
  both labels of every repeat, both ends for RSS.

Each tolerance is three times the largest deviation. Three, because a
check sets one estimate against another and a few repeats understate the
spread. The floor is three times the largest deviation on a series under
50 ms (for RSS, on any series), rounded up to the next 0.5 ms or 0.5 MiB.
The relative bound is three times the largest relative deviation the floor
does not cover, rounded up to the next 1%, and at least 1%.

Calibrate on an idle machine: what the calibration sees is what the
tolerances allow.

## The proofs

Run from the repository root after `cargo xtask build`, with a quick
calibration in the baseline file:

```sh
python3 tests/perf/prove.py regression    # a planted paging cost is caught; half of it is not
python3 tests/perf/prove.py load          # under load, never "regressed"
python3 tests/perf/prove.py load --busy 6
```

**Planted regression.** `planted/paging-cost.patch` makes the paginator
busy-wait a given number of microseconds per page. `prove.py regression`
builds `HEAD` with it (an export under `target/perf/proof/`, patched with
`patch`, the size written into the source), lets the load the build left
settle, and checks the patched binary against the unpatched default
artifact. By default it plants 1.5 times and 0.5 times the smallest
per-page cost the quick calibration claims to catch on `json-pages-100` at
0 ms; `--per-page-us N` (must be caught) and `--under-us N` (must pass)
choose others. The larger must fail the check on paged native series
only, and the smaller must not fail it. At 5286fd237's calibration:

| Planted | Result |
|---|---|
| 113 µs per page (1.5×) | failed, exactly on `json-pages-100 https 0 ms native`: wall 43.5 ms against 31.8, CPU 32.8 ms against 21.6; nothing else |
| 37 µs per page (0.5×) | passed |

**Load.** `prove.py load` checks the default artifact against a copy of
itself while N busy loops (one per CPU by default) run. The proof holds
when the check passes or could not judge (exit 0 or 2), and fails if it
ever reports a regression. On the reference machine:

| Busy loops | Load average | Result |
|---|---|---|
| 6 | 3.5 to 6.5 | passed, nothing loaded or noisy |
| 12 | 10.5 to 17.1 | could not judge: 2 series too noisy; nothing regressed |

## The stored baseline: `--check-baseline`

The baseline file also holds each tier's record of the binary it was
taken on, without its runs (`--write-baseline`, on the default artifact
of a clean committed head). `--check-baseline` judges a result against its
tier's record: counts exactly; median wall and CPU and both ends of the
peaks with the calibration's baseline tolerances; only on the record's CPU
model; with a load limit of the calibration's highest plus a quarter per
CPU, since a stored record has no same-run reference; and a complete run
missing a series the record has fails. This is for absolute tracking over
time, not the gate for a change: a stored record moves with the
machine's state, which a same-run comparison cancels.

Each tier is recorded in its own rounds. A series' median depends on the
series it runs among: short native calls ran 15 to 17% slower inside the
full tier's rounds than inside the quick tier's.

## Refreshing

On the head to record, idle machine, clean tree:

```sh
cargo xtask build
cargo xtask perf --calibrate             # quick tolerances, about 12 min
cargo xtask perf --calibrate --full      # full tolerances, about 48 min
cargo xtask perf --write-baseline        # both records, about 10 min
cargo xtask perf --check-baseline        # a second run must pass against them
git add tests/perf/baselines && git commit
```

Recalibrate when the reference machine changes, and when a case is
added or changed. A change meant to move the numbers needs no refresh for
`--check`: its own comparison shows the change (connections CHANGED,
times improved). Refresh the stored record afterwards and say in the
commit what moved.

## What is measured

`run.py` provisions a venv from the blackbox harness's hash-pinned lock
(for `trustme`, which makes the stub's test CA) and runs `measure.py`. That
starts two stubs (`stub.py`), one HTTP and one HTTPS. A series is a case
of `cases.json` at one of its schemes and latencies, on one route, for one
binary; the full tier runs every scheme and latency, the quick tier the
pairs each case's `quick` entry names. The runs go round-robin: each round
runs every series once, the routes taking turns and the binaries taking
turns, and the first `warmup` rounds are not recorded. A passing load on
the machine then falls on every series alike and shows as spread, which
the noise rule sees. The route is forced with `AWS_CLI_NATIVE_ROUTER`
(`native` or `python`): a case the native route cannot run fails the
measurement (a comparison's base may refuse it instead); it is never
silently measured on Python.

For each run:

| Measurement | Source |
|---|---|
| wall time | from just before the spawn to the reap |
| CPU user and system time, peak RSS | the child's `wait4` resource usage (`ru_utime`, `ru_stime`, `ru_maxrss`), not sampling |
| one-minute load average | just before the spawn |
| connections opened, requests made, response bytes | counted by the stub |
| output bytes | the size of the command's stdout, which goes to a file |

A run fails the measurement unless the CLI exits 0 and makes exactly the
case's page count of requests. Each series is summarised: p50 and p95 of
wall time, CPU time (user plus system), user and system time apart, and
peak RSS, each with its median absolute deviation and its least and
greatest value; requests/s and pages/s over the median wall time; MB/s
(10^6 bytes of response) over the median wall time; and the least and
greatest connection and request counts. Percentiles interpolate linearly
between ranks (numpy's default).

**First run.** On macOS the first run of a freshly built binary costs
about 1.3 s (1.3 to 1.4 s measured here), while the system scans the new
executable. The warm-up rounds absorb it: each record keeps its binary's
first run (printed as "first run" in its heading), and a judged run
refuses `--warmup 0`.

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
`Connection: close`, as an HTTP/1.1 server does. A wait sleeps in halving
steps and spins its last 2 ms, so it ends within microseconds of its
deadline despite the timer leeway macOS allows.

## The result file

`target/perf/result.json` holds a record (one binary) or, for a
comparison, the base's and the head's records. A record gives the platform
key, the host (OS, release, CPU model, CPU count), the binary's path
(relative to the checkout when it lies inside it) and commit (and whether
the worktree had uncommitted changes to tracked files), the tier, the run
counts, the first run, every series with its summary and its raw runs, and
the series the binary refused.

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
Recalibrate after adding one.

## Linux and CI

The harness runs on macOS and Linux (`ru_maxrss` is read in the unit each
reports). Each platform has its own baseline file, `<os>-<arch>` as Rust
names it.

A comparison needs no reference machine: base and head share the runner,
so CI can run `cargo xtask perf --check` on any runner that is not
running other jobs at the same time, as its own job (not inside `cargo
xtask test`): on every change, after `cargo xtask build`, with the main
line named (`--mainline`, or `git config perf.mainline`, or a clone with
an `origin` remote). The first check against a base builds it; cache
`target/perf/base/` between jobs to keep that cost to one build per main
line commit. Its tolerances come from the platform's calibration:
calibrate once per runner type (`--calibrate` on an idle runner) and
commit the file, or the check judges counts only. Keep
`target/perf/result.json` as an artifact. A check that exits 2 could not
judge; rerun it, not the change. `--check --full` suits changes to the
client, the transport or the paginator.
