# Rust component size analysis

This is an informational workflow, not a byte-budget gate. `--wasm` inputs are
never modified; `--manifest` runs Cargo normally and copies its build output.
Reports contain the original component, extracted core modules,
section accounting, tool versions, Cargo output/lockfile, and optional attribution
and optimized components. Run from the repository root with Python 3.11+, Rust's
`wasm32-wasip2` target, and `wasm-tools` 1.253.0 (older versions cannot decode the
Preview 3 component types used here).

```sh
cargo binstall --no-confirm --locked wasm-tools@1.253.0 twiggy@0.8.0
python3 -m unittest discover -s test-components/size-analysis -v
python3 test-components/size-analysis/analyze.py \
  --manifest test-components/agent-counters/Cargo.toml \
  --matrix --out tmp/counters-size
python3 test-components/size-analysis/analyze.py \
  --manifest test-components/benchmarks/benchmark-agent-rust/Cargo.toml \
  --matrix --out tmp/benchmark-size
```

## Reflection retention contracts

The `fixtures` workspace provides named, unstripped empty, non-reflective tool/client,
and explicit reflection components. Build them through `analyze.py`, then run:

```sh
python3 test-components/size-analysis/retention.py \
  --negative empty=tmp/retention-empty/current/component.wasm \
  --negative tool=tmp/retention-non-reflective/current/component.wasm \
  --positive reflection=tmp/retention-reflection/current/component.wasm \
  --enforce-option-b
```

The gate rejects reflection host imports and model/reflection helper retention
in both negative fixtures, and requires `get-agent-type` plus the positive
fixture's named reflection method.

The output directory must not exist. Builds use `--locked`; generate/update the
application's lockfile intentionally before analysis if necessary. Cargo artifact
messages select the WASM, so redirected target directories and workspaces work.
Use `--package` if a manifest builds multiple WASMs. Use `--wasm path.wasm` instead
of `--manifest` to analyze an already-built component or core module, or omit
`--matrix` to measure the current profile (`--profile dev` includes debug symbols).
Build times are wall-clock observations, not clean-build benchmarks: cache state
and concurrent work matter. Keep the same toolchain, lockfile, Cargo configuration,
and environment when comparing reports; do not run concurrent builds of the same
application. The tool records explicit matrix overrides, not arbitrary environment
variables or secrets.

## Read the right numbers

`summary.json` separates complete component, code, data, and custom-section bytes.
Code/data totals include section framing and sum across all embedded core modules.
`report/sections.json` is a non-overlapping byte table including nested component
framing; its rows sum exactly to the original file length. `objdump.txt` also
contains wasm-tools' human-readable table (whose sizes exclude section framing).
Neither number is linear-memory usage, compiled machine-code size, or wire size
after compression.

`report/core-N/module.wasm` contains each core in traversal order. With `twiggy`
installed, `symbols.json` records shallow symbol sizes and `crate-estimates.json`
groups demangled symbol namespaces. Names with Rust v0 disambiguator hashes work.
This is approximate ownership after LTO/inlining, **not** per-crate retained size:
shared generic code, trait methods, data and unknown symbols can be unattributed.
The attribution copy has demangled names, which can enlarge its custom sections;
use the original section table for total sizes. Missing/unsupported optional
attribution tools produce `attribution-unavailable.txt`, not a failed size report.
Stripped release binaries cannot recover symbol names; use the named baseline or
`CARGO_PROFILE_RELEASE_STRIP=debuginfo` for attribution.

## Defaults and independent comparisons

Generated Rust applications and the SDK release profile use `strip="symbols"`.
Debug profiles are unchanged. This strips Rust core names, DWARF and other linker
custom sections while preserving metadata needed by componentization. Rust's
component linker subsequently adds small `component-name` and `producers`
sections; these are intentionally retained. Do not blindly strip every custom
section before componentization: `component-type*` is required by `component new`.
Release stack traces lose Rust symbol names; retain a separately built named
artifact when diagnosing production failures. A dependency's Cargo profile does
not propagate to consumers: existing applications must opt in in their workspace
root; the template covers newly generated applications.

The matrix fixes LTO=true, opt-level=s, codegen-units=16, panic=unwind,
debug=false and strip=debuginfo, then changes **one** choice at a time:
strip=symbols, codegen-units=1, panic=abort, opt-level=z. `strip=none` is not the
baseline: it can retain debug information from prebuilt dependencies even with
debug=false. The benchmark fixture has a member-local s/LTO profile that Cargo
ignores; matrix overrides make comparisons explicit without silently changing
that application's effective optimization settings.

Keep `s` and LTO for generated applications, and keep Cargo's existing codegen and
panic defaults. One codegen unit and `z` reduce code on the measured fixtures, but
allocator timings do not establish whole-agent CPU/serialization/RPC performance.
Explicit panic=abort gives no meaningful size win: `rustc --print cfg --target
wasm32-wasip2` already reports panic="abort". Do not impose abort on the SDK's native
test consumers to chase a WASM-only size saving.

## Experimental Binaryen pipeline

Install Binaryen 123 and put its `bin` directory on PATH, then add `--optimize`.
For a complete component, the tool extracts every core, runs
`wasm-opt -Oz -g` with bulk-memory, sign-ext, nontrapping-float-to-int,
mutable-globals, multivalue and reference-types enabled, and re-embeds each core
at its original position. Unsupported input proposals fail rather than silently
broadening the feature set.
Nested section lengths are rewritten, but the wrapper's component types, aliases,
instances, canonical ABI wiring, and custom metadata are preserved. It validates
the result and requires the decoded WIT to match before/after. This is rebundling
an existing component, not reconstructing its world from untyped machine code.

For a raw core with `component-type*` custom sections, the tool restores those
sections byte-for-byte after Binaryen. Only then is this safe:

```sh
python3 test-components/size-analysis/analyze.py \
  --wasm typed-core.wasm --optimize --out tmp/typed-core-size
wasm-tools component new tmp/typed-core-size/current/optimized.wasm \
  -o tmp/optimized-component.wasm
wasm-tools validate --features all tmp/optimized-component.wasm
```

An unbundled core from an already-encoded component often no longer has that
metadata. Do not call `component new` on it and assume the original world survives.
Use the original wrapper instead. Preview 1 adapters also require their original
componentization configuration.

Do not substitute `--all-features`: Binaryen 123 introduced typed references that
passed wasm-tools validation but failed Golem's engine during schema extraction
("function references required for index reference types"). Validation with
`--features all` and equal WIT are structural checks, not semantic or
deployment-engine proof. `-Oz` is **not** enabled in release
builds or deployment paths. Test the resulting component with the actual executor
and workload before adopting it. Binaryen can change stack/fuel usage and timing.

## Export and constructor retention experiment

`retention.wat` has a minimal component export, an unreferenced private function,
an extra core export with a loop, and a start constructor whose effect is observable
through `run() == 42`. It does not change an SDK WIT world.

```sh
wasm-tools parse test-components/size-analysis/retention.wat -o tmp/retention.wasm
python3 test-components/size-analysis/analyze.py \
  --wasm tmp/retention.wasm --optimize --out tmp/retention-size
wasmtime run --invoke 'run()' tmp/retention-size/current/optimized.wasm
wasm-metadce tmp/retention-size/current/report/core-0/module.wasm \
  --graph-file test-components/size-analysis/retention-graph.json -g \
  -o tmp/retention-dce-core.wasm
PYTHONPATH=test-components/size-analysis python3 - <<'PY'
from pathlib import Path
from analyze import rewrite_cores
Path('tmp/retention-dce.wasm').write_bytes(rewrite_cores(
    Path('tmp/retention.wasm').read_bytes(),
    lambda _: Path('tmp/retention-dce-core.wasm').read_bytes()))
PY
wasm-tools validate --features all tmp/retention-dce.wasm
wasmtime run --invoke 'run()' tmp/retention-dce.wasm
```

Ordinary `-Oz` removes the private unreachable body but retains the unused core
export. The hand-authored graph accounts for this fixture's component alias and
lets `wasm-metadce` remove that export and its loop; the constructor effect remains.
This graph is **fixture-specific**, not a generic Golem optimization. Live canonical
exports (including post-return/callback functions), table references and ctor
registration remain roots. Replacing one export body with a stub cannot remove
code reachable through those other roots. Automatically deriving a safe graph
for arbitrary component instances/aliases is not implemented.

## Performance and CI

```sh
node test-components/size-analysis/benchmark-core.mjs \
  tmp/counters-size/baseline-s/report/core-0/module.wasm \
  tmp/counters-size/strip/report/core-0/module.wasm \
  tmp/counters-size/cgu1/report/core-0/module.wasm \
  tmp/counters-size/z/report/core-0/module.wasm > tmp/allocator-performance.json
```

This isolates core instantiation and 100,000 allocate/grow/free cycles (two warmups,
ten measured samples, alternating variant order), checks preserved bytes, and
throws on any host call. It does not initialize agents or measure Golem latency,
replay, JIT compilation or application throughput. Run the repository latency,
throughput and cold-start suites before changing code-generating defaults broadly.

`.github/workflows/component-size.yaml` runs on relevant PRs or manual dispatch,
uploads reports/partial results, and has no size or timing threshold. It is not a
required merge gate. It neither deploys nor publishes optimized components.

## Measurements, 2026-09-22

Linux x86_64 orb, rustc 1.98.1 / LLVM 22.1.8, wasm-tools 1.253.0, Binaryen 123,
twiggy 0.8.0, Node 24.19.0. The controlled matrix above produced these byte counts;
`strip + Oz` uses the restricted feature set, not the rejected all-features output.
These observations are not golden-file expectations or size limits.

| Component | Choice | Component bytes | Code bytes | Data bytes | Custom bytes |
|---|---|---:|---:|---:|---:|
| counters | s baseline | 2,266,562 | 1,494,546 | 397,051 | 321,598 |
| counters | strip symbols | 1,958,402 | 1,496,001 | 397,051 | 11,968 |
| counters | codegen-units=1 | 2,193,050 | 1,435,553 | 396,303 | 307,926 |
| counters | panic=abort | 2,269,254 | 1,495,916 | 397,051 | 322,905 |
| counters | opt-level=z | 2,147,654 | 1,262,630 | 400,591 | 430,011 |
| counters | strip + Oz | 1,682,546 | 1,221,454 | 396,241 | 11,996 |
| benchmark agent | s baseline | 2,185,586 | 1,435,072 | 393,279 | 304,370 |
| benchmark agent | strip symbols | 1,893,714 | 1,435,746 | 393,303 | 11,792 |
| benchmark agent | codegen-units=1 | 2,118,294 | 1,379,054 | 392,555 | 293,911 |
| benchmark agent | panic=abort | 2,186,387 | 1,435,206 | 393,279 | 305,040 |
| benchmark agent | opt-level=z | 2,070,284 | 1,210,010 | 396,851 | 409,521 |
| benchmark agent | strip + Oz | 1,630,871 | 1,174,130 | 392,507 | 11,813 |

Stripping saves 13.6% / 13.4% of complete components in the controlled matrix;
restricted Oz saves another 14.1% / 13.9%. Small code/data changes are real, not
assumed to be zero: Cargo profile changes rebuild and can affect code layout and
embedded diagnostic strings. Explicit panic=abort slightly increases these sizes.
With the benchmark workspace's actual opt-level=3/no-LTO settings unchanged,
the separate debuginfo-stripped → symbol-stripped comparison is 3,509,819 →
3,041,606 component bytes, 2,372,262 → 2,372,266 code bytes, and 601,369 → 601,377
data bytes. The generated application profile remains s/LTO.

Allocator smoke medians, milliseconds (100,000 cycles; same inputs for every
variant). V8 instantiation excludes compile time and does not initialize agents.
These small differences should not be read as statistically established speedups.

| Choice | Counters instantiate | Counters allocator | Benchmark instantiate | Benchmark allocator |
|---|---:|---:|---:|---:|
| s baseline | 0.472 | 15.298 | 0.481 | 16.010 |
| strip symbols | 0.511 | 15.040 | 0.512 | 16.122 |
| codegen-units=1 | 0.478 | 14.884 | 0.471 | 15.032 |
| panic=abort | 0.482 | 15.344 | 0.492 | 15.930 |
| opt-level=z | 0.549 | 15.304 | 0.472 | 15.459 |
| strip + restricted Oz | 0.481 | 14.907 | 0.457 | 15.035 |

For counters, named shallow attribution assigns 288,276 bytes to `golem_rust`,
253,364 to `regex_automata`, 177,655 to `golem_schema`, 170,134 to `core`, and
123,595 to `it_agent_counters`. Large individual functions include regex strategy
construction (43,124 bytes), agent definition lowering (37,362), and the tool
invocation wrapper (37,271). The 833,645 unattributed bytes include data, names,
and symbols not classifiable by namespace; they are not evidence of dead code.

Validation performed:

- Five Python accounting/metadata/attribution tests pass; Ruff checks and Node
  syntax check pass. Both Rust component matrices build and validate, and all
  optimized outputs preserve their decoded WIT contract.
- Counters debug build retains `.debug_info`, `.debug_line`, `.debug_str` and
  `name`; the release symbol-stripped cores do not contain them.
- `cargo test -p golem-worker-executor --test integration --
  worker_initialization::partial_creation_reloads_identity_and_original_initialization
  --report-time`: 1 passed with the stripped counters fixture.
- `cargo test -p golem-worker-executor --test integration --
  durability::automatic_snapshot_every_2nd_invocation durability::snapshot_based_recovery
  --exact --report-time`: 2 passed separately with baseline, stripped, and
  restricted-Oz counters components (six successful test executions).
- The all-features Oz experiment failed executor schema extraction before test
  execution; a standalone Wasmtime core compile exposed the typed-reference error.
  The workflow now uses the restricted flags above; no engine settings changed.
- The retention fixture measures 285 → 224 bytes with Oz and 177 bytes with the
  component-aware hand-authored metadce graph. Both optimized components return
  42. Raw-core `component-type` preservation followed by `component new` also
  validates and returns 42.

No full latency/throughput suite or production rollout was performed. Keep
code-generating changes opt-in until representative application benchmarks,
fuel/stack-sensitive workloads, and broader replay/RPC coverage justify them.
