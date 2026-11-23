# Bash tool conformance matrix

The matrix compares the tool's shell with recorded Bash 5 and GNU tool answers. It checks the exact shell exit status, stdout and stderr, including invalid UTF-8. It runs the standalone `cooperative` example under Wasmtime, not a Golem server. That example uses the same shell crate and deterministic sibling-tool probes. It runs as a WASI 0.3 command on the component's own task runtime.

## Provenance and harness choice

Golem added this harness and its corpora in [PR #3929](https://github.com/golemcloud/golem/pull/3929), rather than vendoring a third-party Python suite. Some real-world cases adapt public examples and command shapes; [cases/NOTICE](cases/NOTICE) records their sources, revisions, licenses and modifications. The separate Brush compatibility suite is upstream Rust/YAML, fetched at the Brush revision pinned in `../Cargo.toml`.

The harness is a typechecked Effect TypeScript CLI because its work is external-process orchestration: running Wasmtime and Docker, recording oracle answers, checking drift, reporting failures and maintaining baselines. These are developer commands, not Rust unit tests. The process-free harness tests use Bun. Brush keeps its own Rust test harness; it is not converted to test-r.

`cases/*.json` is the editable source of truth. The former Python modules' finite, deterministic generators have been expanded into explicit cases. All 18,009 scripts, names, tags, options, tiers, fixture bytes and reasons were preserved. No oracle answers were regenerated for this conversion. The corpus descriptions and third-party notice remain with the data. New cases do not require a generator or Python.

## Run the matrix

Install Bun, Wasmtime with WASI 0.3/component-model async support (CI uses 50.0.1), and the Rust `wasm32-wasip2` target. From the repository root:

```sh
bun install --cwd builtin-tools/bash/conformance --frozen-lockfile
bun run --cwd builtin-tools/bash/conformance check
bun run --cwd builtin-tools/bash/conformance test
cargo build --manifest-path builtin-tools/bash/Cargo.toml --locked \
  -p bash-shell --example cooperative --features test-support --release --target wasm32-wasip2
W=builtin-tools/bash/target/wasm32-wasip2/release/examples/cooperative.wasm
bun builtin-tools/bash/conformance/conformance.ts check --wasm "$W"
bun builtin-tools/bash/conformance/conformance.ts coverage --wasm "$W"
# After adding or changing cases (needs Docker):
bun builtin-tools/bash/conformance/conformance.ts record --prefix '<text>'
```

The package's `check` script runs strict TypeScript checks, oxlint and dprint. The cargo-make preparation task runs the same checks. `bun run --cwd builtin-tools/bash/conformance format` formats the TypeScript and package configuration, not the corpora or goldens. The pinned native oxlint rules follow Effect's conventions and reject explicit `any`; they do not require the unpublished `@effect/oxc` plugin. Formatting uses two spaces, double quotes, no semicolons or trailing commas, and a 120-column target.

The CLI supports `check` (the default), `coverage`, `record`, `live` and `stale`. `live` compares with the oracle directly, without reading goldens or a recorded fingerprint. `stale` re-runs it to find answers that no longer match the goldens. Only `record` writes goldens. `check` and `coverage` need no Docker.

The harness compiles the shell once before parallel execution, outside the per-case timeout, with a 120-second compilation timeout. Each case has a fresh filesystem and a 15-second execution timeout unless its `options.timeout` overrides it. Both output streams drain concurrently. Effect scopes release temporary directories and subprocesses, including on timeout or interruption. Oracle containers are scoped to workers and reset before each case.

`--jobs N` bounds concurrency. Its default is `BASH_CONFORMANCE_JOBS`, or the available CPU count. `--case '<name>'` selects an exact name; `--prefix '<text>'` selects a prefix. Repeat either to select more. A selection that matches no cases fails. `--report FILE` writes JSON failure records with scripts, tags, expected and actual byte streams, and execution errors. `--wasmtime PATH` and `--oracle-image IMAGE` override the executables/image used for local checks.

## Corpus contract

Each corpus has a `description`, `tier` (`pr` or `sweep`) and `cases` array. Each case has a unique `name`, a `script`, `tags` and `options`. `options.no_tmp` leaves `/tmp` absent. A full deliberate fixture is `expected: {status, stdout, stderr, reason}`. A stderr-only fixture is `expectedStderr: {value, reason}`. Byte streams use either `{"text": "..."}` or `{"base64": "..."}`. Reasons must be nonempty. Full fixtures replace all three outcome fields; stderr-only fixtures preserve the oracle status and stdout. `check` rejects a fixture that now matches the oracle.

`oracleRace` holds the reason an oracle answer depends on timing. `check` still compares that case with its recorded golden; `stale` skips it. `stale` also skips full deliberate fixtures, whose live oracle answer is not the tool's contract. It does not skip stderr-only fixtures.

A line `#--call-- /some/directory` splits a script into separate calls. Each call starts a fresh shell. Our example carries the previous call's final cwd, as a caller passing back the tool's returned cwd does. The oracle starts the next call in the directory named by the marker (`/` if omitted). Outputs concatenate and the last call supplies the status. State other than cwd does not carry between shells.

`features.json` lists language features, builtins, commands, explicit not-applicable reasons, qualifier tags and Unicode-aware ECMAScript syntax detectors. A case earns credit from its tags, syntax and command positions, and its command-prefixed name. `coverage` rejects unknown tags, uncovered applicable features, and differences between the checklist and the shell's actual `compgen` builtin/command registration. `printf` is a command that replaces a Bash builtin; `probe-tool` and `legacy-stdin` are example-only fixtures.

Detectors are coverage heuristics, not a shell parser. They use native ECMAScript `\s` for whitespace and `[\p{L}\p{N}_]` for Unicode names and keyword boundaries. Keep those boundaries Unicode-aware: `café()` is a function, but `whileé` is not the `while` keyword. These expressions do not emulate Python's regular-expression semantics.

## Oracle and goldens

Both sides see no HOME, no passwd entry for their uid, cwd `/`, Golem's fixed `GOLEM_*` variables, and a fresh writable filesystem. The oracle also has a tool PATH and `LC_ALL=C.UTF-8`. Network error cases use a refused port and a `.invalid` name.

`oracle.Dockerfile` pins its Alpine base by multi-architecture digest and installs Bash 5 and GNU tools. Packages come from Alpine's index, so rebuilding can pick up new versions. Goldens include fingerprints of the full Dockerfile and environment profile, plus each script's hash. `check` refuses a changed oracle profile, missing golden or edited script. The status report must contain a u8 and equal the WASI process exit status; a runtime failure after the report is not accepted as a shell result.

`record` runs each selected case twice by default, in fresh worker containers on each pass, and refuses to record an answer whose status, stdout or stderr differs. It reports the differing field. `--runs 1` skips that determinism check for local iteration; it does not establish a deterministic answer. Recording a subset preserves other current goldens and removes cases that no longer exist.

Goldens were recorded on arm64. Bash/GNU answers can differ on x86-64 because of C `char` signedness and Bash `long double`: multibyte IFS after whitespace, rounding ties in `printf`, `xargs -d` with byte 0xff, and `wc --debug` hardware output. The affected PR cases retain their explained fixtures; the sweep omits these inputs.

## Tiers and CI

There are 2,254 `pr` cases and 15,755 additional `sweep` cases. `check` defaults to `pr`; `--tier all` checks both. Other commands select from both tiers. The sweep has no deliberate fixtures. `sweep-known-failures.json` lists its existing differences. `--known-failures FILE` rejects both new failures and selected listed cases that now pass. `--update-known-failures` rewrites that file from the selected run; use a full run to replace the full baseline.

- `cargo make test-builtin-bash-pipelines` builds the example, checks types, lint and formatting, tests the harness, checks PR cases, and runs coverage.
- `cargo make test-builtin-bash-sweep` runs Brush compatibility against its baseline and the full matrix against its known failures.
- `cargo make test-builtin-bash-oracle-drift` runs `stale` against the live oracle. It needs Docker, not a built Wasm shell.

The separate `.github/workflows/builtin-bash-oracle-drift.yaml` workflow runs on oracle/corpus/harness input changes, weekly, or manual dispatch. It is not part of every push's matrix check: an upstream oracle change should not fail unrelated work. No command here dispatches a workflow.

## Brush compatibility

`compat/` runs the pinned Brush checkout's roughly 2,500 YAML cases against the tool's `compat` example and real Bash, inside Linux with GNU tools:

```sh
cargo build --manifest-path builtin-tools/bash/Cargo.toml \
  -p bash-shell --example compat --release --target wasm32-wasip2
builtin-tools/bash/conformance/compat/run-in-docker.sh <brush-checkout> <compat.wasm> > out.txt
bun builtin-tools/bash/conformance/compat/baseline.ts check out.txt
```

`baseline.ts check` requires a completed summary and fails on new failures or listed cases that now pass. `update` rewrites `compat/baseline.txt`. The named upstream SIGPIPE race is excluded. Brush's own YAML `known_failure` handling stays in its harness. Many baseline cases use standard input, a script file or interactive mode, whereas the tool always runs scripts with `bash -c` semantics.
