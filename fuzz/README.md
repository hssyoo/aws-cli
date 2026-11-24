# Transfer fuzz models

Seventeen targets drive the source pool, the download writer, the recursive
scheduler, a recursive download's local paths, the S3 error-body reader, and
the dynamic client's readers: the XML reader, the query and json response
and error parsers, the model reader and validator, the timestamp readers
and argparse's number conversion. Each checks its code against an
independent model, except `pool_operations`, which checks the pool's
invariants after every operation. A target passes libFuzzer's bytes straight to its model. The models use
fake allocation, request and disk operations; they make no AWS calls. A
campaign builds no C: crt-sys's build script skips it under `cfg(fuzzing)`,
where crt-sys defines each C function it declares as one that aborts, so the
targets link on every linker and a model that reached the CRT would fail its
campaign.

| Target | Model | What it checks |
| --- | --- | --- |
| `pool_reservations` | `crt::source_pool::model::reservations` | Per-shard reserved sizes and available slots |
| `pool_ownership` | `crt::source_pool::model::ownership` | FIFO futures, cancellation, ticket references, and allocation failures |
| `pool_operations` | `crt::source_pool::model::operations` | Accounting, slot ownership, buffer exclusivity, FIFO order and stranded waiters after every operation, on one to four shards with one-shot backend failures, ending in a drain or a destroy. An armed re-entry nests one operation inside a backend call or completion, as another CRT thread could run it while the pool's lock is free |
| `writer_ranges` | `s3::download::testing::callback_ranges` | Byte occupancy, including empty ranges and full-width offsets |
| `writer_pages` | `s3::download::testing::page_completion` | Completed bytes and exact batch and forced page eviction |
| `writer_retries` | `s3::download::testing::write_retries` | Disk image, call ledger, direct alignment, bounce buffers, padding, and exact terminal errors |
| `writer_output` | `s3::download::testing::output_composition` | The file each writer composes through `Output` from shuffled, overlapping, out-of-bounds and empty bodies under injected faults, against the bytes it accepted: refusals change nothing, received bytes match the accepted ranges, and a success leaves the object's bytes and length with each mapped page evicted once |
| `scheduler_orderings` | `s3::scheduler::model::ownership` | Submission, finish-before-adoption, cancellation and shutdown orderings with a finish worker, checked by end-state counts |
| `scheduler_phases` | `s3::scheduler::model::smith_ownership` | Per-request phases, releases, cancels, output lines and slot counts after every step |
| `path_containment` | `s3::testing::path_containment` | Destination paths of any listing key stay under the root, or name its parent when the key does; temp names keep the parent and the name limit; absolute paths are absolute and idempotent |
| `error_body` | `crt::s3::error_body::fuzz_read` | Fields of any body the reader accepts decode to UTF-8 |
| `client_xml` | `client::testing::fuzz::xml` | `client::xml::parse` of any bytes gives a tree or expat's error, the same twice |
| `protocol_responses` | `client::testing::fuzz::responses` | Any response (status, headers, body) to any operation of the fake query, json 1.0, json 1.1 and rest-json models parses, or fails as `ResponseParserError`, under botocore's and the CLI's parser defaults |
| `paginator_token` | `client::testing::fuzz::starting_token` | `_parse_starting_token` of any text, under none, one or two input tokens: the same result twice, and a token that decodes to exactly the input tokens re-encodes (the `resume_token` setter) to one that decodes the same |
| `model_json` | `model::fuzz::model_json` | `Model::from_json` of any bytes; a model it builds validates an empty input for each operation |
| `model_validate` | `model::fuzz::validate` | `ParamValidator` of any JSON against every operation of the fake query and json models; a report prints |
| `timestamp_text` | `timestamp::fuzz::parse` | Every timestamp reader over any text; each value read prints in its form and reads back as itself |
| `argparse_convert` | `awscli_argparse::fuzz::convert_both` | argparse's `type=int` and `type=float` of any argv text: an `int` is canonical, a `float` reads back, and text `int()` takes `float()` takes as the same number |

Writer statistics are enabled in the fuzz build (`download-writer-debug`), so
the disk ledger checks the call, retry, bounce and padding counters the writes
return.

## Input format

Every model except `error_body` and the six parser targets, which read
their input as the text they parse (`protocol_responses` takes its first
three bytes as the model and operation, the status, and the headers),
reads its choices through `Choices` in its crate's `testing` module
(`crates/crt/src/testing.rs`, `crates/s3/src/testing.rs`), a thin layer over
`arbitrary::Unstructured`. A choice reads from the front of the input. A range
reads the fewest bytes that cover it, most significant first, and reduces them
modulo its size; a flag reads one byte. Once the input runs out, every later
choice reads as its minimum, so the empty input is the trace of minimum
choices. One input decodes to the same trace under libFuzzer, `cargo test` and
Miri. Both lockfiles pin `arbitrary` 1.4.2, and an xtask test keeps them on
the version the workspace declares.

## Seeds and replay tests

Named seeds are choice scripts in each model's tests, written with `Script`
from the crate's `testing` module as the values the seed means. A script's
replay checks that the model asks for exactly the scripted choices first, so
a model change that inserts or reorders a choice fails the script rather than
silently changing what it means.

`fuzz/corpus/<target>/` holds one file per script, named for it and holding
its bytes, which campaigns start from. The replay test fails when a file
differs from its script; after a model change, rewrite them with
`UPDATE_FUZZ_SEEDS=1 cargo xtask test -p crt -p s3`. Any other file in the
directory is replayed as it is. Inputs a campaign finds stay out of the tree.

Each model's replay test, which `cargo xtask test` runs, then sweeps 256
inputs filled from SplitMix64, one seed per case. A failing case prints its
seed and bytes, and those bytes reproduce the failure under the fuzz target.
`cargo xtask test --miri -p crt -p s3` runs the pool, writer and scheduler
replay tests with eight sweep cases each.

The six parser targets' curated inputs are plain inputs, not scripts: a
response, a model, a timestamp's text. `client::testing::fuzz`'s tests, which
`cargo xtask test -p client` runs, replay each file and every prefix of it
through the same body the target runs, then sweep 256 SplitMix64 inputs.

Two models have no target. `byte_budget` checks the pool's budget predicate
against wider arithmetic, and `shared_writer_reentry` runs the shared writer
service through one fixed schedule of gates. Both keep their scripts and
sweeps as unit tests.

The three listing-parser fuzz tests (semantic values, malformed pages, and
raw XML) have no target either. The parser runs through the native CRT's XML
parser, and this package cannot link the CRT: crt-sys links it statically only
for the musl target, while cargo-fuzz builds for the analysis nightly's GNU
host with AddressSanitizer and libFuzzer's C++ runtime. Those generators run
instead as replay tests in the stable suite
(`crates/crt/src/s3/listing/tests/fuzz.rs`): each replays fixed scripts, then
sweeps generated inputs through the same reader.

## Campaigns

Install the pinned runner and dated analysis toolchain:

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
rustup toolchain install nightly-2026-08-28 --component rust-src --profile minimal
```

Run every target with the CI budget, one target for five minutes, a repeat of
an earlier campaign by its seed, or a minimization of every campaign corpus:

```sh
cargo xtask fuzz --runs 1000
cargo xtask fuzz --target writer_retries --time 300
cargo xtask fuzz --target writer_retries --seed 1234 --runs 100000
cargo xtask fuzz --merge
```

The runner checks the cargo-fuzz version and selects the nightly from
`rust-analysis-toolchain.toml`. Each campaign reads two corpus directories:
its campaign corpus, `target/fuzz-campaign/corpus/<target>/`, which the
first campaign creates and later ones keep, and the curated inputs in
`fuzz/corpus/<target>/`. libFuzzer writes new inputs only to the campaign
corpus. `--runs` bounds the executions, counting those that load the corpus;
`--time` bounds the seconds; given both, a campaign stops at the first, and
given neither, it runs 1,000 executions. `--seed` fixes libFuzzer's seed;
without it each campaign draws a random seed, and the campaign line prints
the seed either way. Every target's inputs are limited to 4096 bytes.
`--merge` runs no campaign: it minimizes each campaign corpus with
libFuzzer's `-merge=1` into `target/fuzz-campaign/merged/<target>/` and
replaces the corpus with the result.

AddressSanitizer is enabled on the native GNU host. Its quarantine of freed
memory is cut to 16 MB unless `ASAN_OPTIONS` sets one, because the quarantine
counts requested bytes, and the direct writer's page-aligned bounce buffers
would otherwise push `writer_retries` past libFuzzer's 2 GB RSS limit within
a minute. On Linux the runner links the targets with the pinned LLVM's clang
and lld, as the release build links; on macOS they link with Apple's linker,
as it does. A failing campaign leaves
libFuzzer's reproducer under `fuzz/artifacts/<target>/`. Minimize it with
`cargo +nightly-2026-08-28 fuzz tmin <target> <file>`, then add the minimized
input to the model's tests as a named script.
