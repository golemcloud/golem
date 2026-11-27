# Component size analysis

The `golem-size-analysis` crate (repository root) measures component size. This directory holds
its inputs: the retention fixtures, the retention experiment and the allocator smoke benchmark.

## Six-SDK fixtures

`sdk-fixtures/` supplies data-only checked call lists and representative source overlays for
TypeScript, Effect, Scala, Rust, MoonBit and Go. The fixture registry and descriptor contract live
in `golem-size-analysis/src/fixtures.rs`. Each SDK has a generated starter, an existing streaming
tool provider and a representative agent. The CLI integration suite assembles these applications
from the current templates and provider sources; the overlays are not independent applications.

Prepare the SDKs through the existing CLI test prerequisites: build the TS/Effect bundles and
guests, publish Scala with `golemPublishLocal`, and install the Rust, MoonBit and patched Go
toolchains. Then build the native CLI test binaries and run a selected SDK, for example:

```sh
cargo test -p golem-cli --test integration -- app::size_fixtures::rust_starter --report-time
```

The six SDK tests each loop over the three fixture kinds and their two presets. TS, Effect and
Scala use `quick`/`optimized`; Rust, MoonBit and Go use `debug`/`release`. SDK overrides always
select this checkout, even when the general CLI suite uses published SDK packages. For debugging
one SDK, set `GOLEM_CLI_TEST_DIR` to an empty persistent directory before the run. Size evidence is
written beneath each generated application's `golem-temp/size-evidence/` directory.
Go measurements append `-trimpath` to `GOFLAGS` so absolute source paths do not change uploaded
bytes between directories. Keep that flag and all other build inputs fixed when comparing runs.

Every descriptor binds `${run}` once in its constructor string arguments and HTTP paths. Calls
through one alias share state; a fresh binding addresses a different keyed agent. Invoke arguments
are ordered CLI JSON values. HTTP expectations include status and JSON body. Tool arguments carry
argv and stdin bytes; tool expectations include the typed result and stdout/stderr bytes. A
successful unit result is explicit JSON `null`, not a missing invocation result. Stream support
comes from the discovered command schema, including empty streams and explicit stdin EOF.

The representative adds 7 then 11, reads 18 through HTTP, reads nondefault runtime configuration,
updates and independently reads a separate same-app agent, invokes a typed provider client and
checks two asymmetric slices of a literal payload. Its dependency also consumes a runtime-selected
slice. `payload.txt` contains 100,000 ASCII hex bytes: concatenate the SHA-256 hex digests of UTF-8
`golem-size-fixture:0` through `golem-size-fixture:1562`, then take the first 100,000 bytes. Its
SHA-256 is `e7694100ad6d20e0586fe5af2e3322f8561886cfa5c13fc8ec10c49665bf57d9`.

Measure only the selected preset's resolved `componentWasm`. The harness compares the deployed
revision's downloaded bytes with that file, then passes the same bytes to the existing collector.
TS/Effect retain `main.modules.json` from the final Rollup output, with app-relative paths
including virtual CommonJS modules. The map is a build target and is regenerated when missing.
Scala retains one linked-bundle bucket. Compiled SDKs use named/debug companion artifacts where needed. Proportional JS ownership
and scaled compiled attribution are estimates, not exact ownership of anonymous data segments.

The first call and an equivalent call on a fresh agent must have equal checked results and complete
oplogs without outbound network operations. Rust, MoonBit and Go starters also rebuild with an
unused feature import and require byte-identical selected upload artifacts. A compiler that changes
those bytes fails this check; the harness does not strip differences or skip the assertion.

## The local report

This is an explicitly invoked local report, not a byte-budget gate. Run the representative
fixture builds, the export-contract and retention checks, the release-profile matrix and the
allocator smoke test with:

```sh
cargo make component-size-report             # or add --optimize for the Binaryen comparison
```

It needs Rust's `wasm32-wasip2` target. Node runs the allocator smoke benchmark when it is on
PATH; `--optimize` needs Binaryen's `wasm-opt`. The report writes `tmp/component-size-report/report.md`,
a combined machine-readable `report.json`, and per-component directories with the component,
extracted cores, the section table, twiggy symbols, crate estimates, WIT, Cargo output, lockfile
and tool versions. The output directory must not exist. Builds use `--locked`; generate or update
an application's lockfile intentionally before analysis. Cargo artifact messages select the WASM,
so redirected target directories and workspaces work.

Build times are wall-clock observations, not clean-build benchmarks: cache state and concurrent
work matter. Keep the same toolchain, lockfile, Cargo configuration and environment when comparing
reports; do not run concurrent builds of the same application. The report records explicit matrix
overrides, not arbitrary environment variables or secrets.

## Measuring one component

`collect` prints the size, gzip size, section table and Rule 2 breakdown of one uploaded file as
deterministic JSON; `--diagnostics` writes the fine breakdown (twiggy symbols and namespaces):

```sh
cargo run -p golem-size-analysis -- collect --sdk rust --wasm component.wasm \
  --user my-component [--named-wasm named.wasm] [--diagnostics symbols.json]
cargo run -p golem-size-analysis -- collect --sdk ts --wasm app.preinitialized.wasm \
  --guest agent_guest.wasm --module main.js --module-map modules.json
cargo run -p golem-size-analysis -- collect --sdk go --wasm release.wasm \
  --named-wasm debug.wasm --user example.com/app --go-packages go-packages.json
```

The buckets add up to the uploaded file:

- TS and Effect: the collector injects the bundle into a temporary guest copy to construct a
  reference, without requiring a `.dynamic.wasm` build output. The data-section bytes added to this reference
  are split into SDK, dependencies and user code in proportion to the rollup module map. The
  snapshot bucket is the uploaded component's data growth over that reference. It excludes source
  storage already assigned to the bundle; it is not the entire memory image written by Wizer.
  Changes to names or other sections during injection remain in the runtime bucket, not the bundle.
  Optimized builds pass source to the initializer directly, so the reference is not their input.
  Quick builds match the injected reference and have no snapshot bucket.
- Scala: runtime, one bundle bucket and the snapshot.
- Rust: twiggy crate attribution of a named build (`--named-wasm`, or the uploaded file itself
  when it keeps names), scaled to the uploaded file when they differ. A trait impl belongs to the
  crate of its type, or of its trait when the type is not a path (`<[u8] as core::fmt::Debug>`).
- Go and MoonBit: twiggy over the `name` section of a debug build, scaled to the release file and
  marked approximate.

Go requires the resolved package list from `go list -deps -json .`, saved as `go-packages.json`
using the build's toolchain and flags. These fixtures use `GOOS=wasip1`, `GOARCH=wasm`,
`-buildmode=c-shared` and `-tags=componentizego_async`. Module ownership and the `Standard` flag
select the buckets; only full package paths match symbol boundaries.
Go symbols are matched against those paths using the pinned linker's Wasm name encoding
(characters other than ASCII letters, digits, `_` and `.` become `_`). This encoding is lossy;
the fixture evidence retains raw matching symbols instead of claiming to recover package paths.
The standard library of the pinned Go toolchain, its
vendored packages and linker-generated `type:` and `go:` symbols are the runtime; any other
package, including a local module with a dotless path, is a dependency. MoonBit symbols are
mangled by `moonc` (`_M0FP28username5hello12use__generic`); the collector decodes their package
(`username/hello`), and `moonbitlang/core` with the compiler's `moonbit.*` and `tlsf/*` helpers
is the runtime. Imports, data segments, section headers and code the compiler generates are
`other`. A scaled breakdown leaves out the custom sections that only the named build carries
(its `name` section and DWARF `.debug_*` sections), so they do not shrink the attributed buckets.
A main core module without function names in its `name` section fails instead of attributing
nothing.

## Reflection retention contracts

The `fixtures` workspace provides named, unstripped empty, non-reflective tool/client, and
explicit reflection components. `./build-components.sh rust` builds `retention-canonical-floor`,
`retention-empty`, `retention-non-reflective` and `retention-reflection` into `test-components/`,
and the `golem-size-analysis` `retention` test checks them:

```sh
cargo test -p golem-size-analysis --test retention
```

The test rejects reflection host imports and model/reflection helper retention in both negative
fixtures, and requires `get-agent-type` plus the positive fixture's named reflection method. The
test also checks that the empty fixture keeps the canonical floor's five-interface export contract
and imports nothing new. The report runs the same checks on its own builds.

The canonical floor depends only on the pinned WIT bindings, not the SDK. Its absent snapshot
loader uses the generated ABI to drop nested constructor allocations and owned resources before
returning the same error as an empty SDK component. Its import envelope therefore includes the
binding runtime needed by that path; it is not a theoretical minimum across handwritten ABIs.
`empty_preserves_canonical_floor_contract` compares the prebuilt floor and empty component with
the same strict export/import checker used by the report.

## Read the right numbers

`summary.json` separates complete component, gzip, code, data, and custom-section bytes.
Code/data totals include section framing and sum across all embedded core modules.
`report/sections.json` is a non-overlapping byte table including nested component framing; its
rows sum exactly to the original file length. Neither number is linear-memory usage or compiled
machine-code size. The gzip size uses the one fixed level every Golem size measurement uses.

`report/core-N/module.wasm` contains each core in traversal order. `symbols.json` records twiggy's
shallow symbol sizes (demangled in process) and `crate-estimates.json` groups symbol namespaces.
Names with Rust v0 disambiguator hashes work. This is approximate ownership after LTO/inlining,
**not** per-crate retained size: shared generic code, trait methods, data and unknown symbols can
be unattributed. Stripped release binaries cannot recover symbol names; their report has
`attribution-unavailable.txt`. Use the named baseline or `CARGO_PROFILE_RELEASE_STRIP=debuginfo`
for attribution.

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

Install Binaryen 123, put its `bin` directory on PATH, then run the report with `--optimize`, or
optimize one component:

```sh
cargo run -p golem-size-analysis -- optimize --wasm component.wasm -o optimized.wasm
```

For a complete component, the tool extracts every core, runs `wasm-opt -Oz -g` with bulk-memory,
sign-ext, nontrapping-float-to-int, mutable-globals, multivalue and reference-types enabled, and
re-embeds each core at its original position. Unsupported input proposals fail rather than
silently broadening the feature set. Nested section lengths are rewritten, but the wrapper's
component types, aliases, instances, canonical ABI wiring, and custom metadata are preserved. It
validates the result and requires the decoded WIT to match before/after. This is rebundling an
existing component, not reconstructing its world from untyped machine code.

For a raw core with `component-type*` custom sections, the tool restores those sections
byte-for-byte after Binaryen. Only then is this safe:

```sh
cargo run -p golem-size-analysis -- optimize --wasm typed-core.wasm -o optimized-core.wasm
wasm-tools component new optimized-core.wasm -o tmp/optimized-component.wasm
wasm-tools validate --features all tmp/optimized-component.wasm
```

An unbundled core from an already-encoded component often no longer has that metadata. Do not
call `component new` on it and assume the original world survives. Use the original wrapper
instead. Preview 1 adapters also require their original componentization configuration.

Do not substitute `--all-features`: Binaryen 123 introduced typed references that passed
wasm-tools validation but failed Golem's engine during schema extraction ("function references
required for index reference types"). Validation with every feature and equal WIT are structural
checks, not semantic or deployment-engine proof. `-Oz` is **not** enabled in release builds or
deployment paths. Test the resulting component with the actual executor and workload before
adopting it. Binaryen can change stack/fuel usage and timing.

## Export and constructor retention experiment

`retention.wat` has a minimal component export, an unreferenced private function,
an extra core export with a loop, and a start constructor whose effect is observable
through `run() == 42`. It does not change an SDK WIT world.

```sh
wasm-tools parse test-components/size-analysis/retention.wat -o tmp/retention.wasm
cargo run -p golem-size-analysis -- optimize --wasm tmp/retention.wasm -o tmp/retention-Oz.wasm
wasmtime run --invoke 'run()' tmp/retention-Oz.wasm
```

Ordinary `-Oz` removes the private unreachable body but retains the unused core
export. The metadce step ran `wasm-metadce` on the extracted core with
`retention-graph.json` and re-embedded the result in the original wrapper. The hand-authored graph accounts for this fixture's component alias and
lets `wasm-metadce` remove that export and its loop; the constructor effect remains.
This graph is **fixture-specific**, not a generic Golem optimization. Live canonical
exports (including post-return/callback functions), table references and ctor
registration remain roots. Replacing one export body with a stub cannot remove
code reachable through those other roots. Automatically deriving a safe graph
for arbitrary component instances/aliases is not implemented.

## Performance and CI

```sh
matrix=tmp/component-size-report/release-profile-matrix
node test-components/size-analysis/benchmark-core.mjs \
  $matrix/baseline-s/report/core-0/module.wasm $matrix/strip/report/core-0/module.wasm \
  $matrix/cgu1/report/core-0/module.wasm $matrix/z/report/core-0/module.wasm \
  > tmp/allocator-performance.json
```

This isolates core instantiation and 100,000 allocate/grow/free cycles (two warmups,
ten measured samples, alternating variant order), checks preserved bytes, and
throws on any host call. It does not initialize agents or measure Golem latency,
replay, JIT compilation or application throughput. Run the repository latency,
throughput and cold-start suites before changing code-generating defaults broadly.

The report is intentionally not a CI job: it publishes no trend and has no meaningful
regression threshold. Run it locally when changing SDK code generation, guest bridge
generation, canonical ABI wrappers, runtime retention, or release profiles, and compare
results produced with the same toolchain, lockfile, Cargo configuration, and environment.

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
- `cargo test -p golem-worker-executor-tests --test integration --
  worker_initialization::partial_creation_reloads_identity_and_original_initialization
  --report-time`: 1 passed with the stripped counters fixture.
- `cargo test -p golem-worker-executor-tests --test integration --
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
