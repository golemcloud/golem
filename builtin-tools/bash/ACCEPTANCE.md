# Bash acceptance evidence

Version 0.2.0 changes the contract to `run(cwd, script, timeout)`: a call stops its script at a
time limit (600 s by default, at most 3600 s) with TERM, then KILL, and exit code 124, and the
shell gains GNU's `timeout` command. The artifact rows below are for
0.2.0.

Verified on 2026-10-01 on the `builtin-bash-tool` branch, based on Golem `c2fc5d83c`, with the
fork revisions and the artifact below. The conformance matrix runs the same shell library as the
standalone `cooperative` example, built from the same source and pins; the other checks were run on
that source too. Provisioning Bash as a built-in tool, with its registry tests and the real-server
CLI scenarios, is a separate change.
Optional Rust argument encoding remains a separate requirement for the full 1.6 surface.

## Revisions and artifact

| Item | Value |
|---|---|
| Port source | Clank `2b4024a25bab5b9e046090152f87b13ea666cc69` |
| Brush | `9549479d688721d6ae10a0b5fdc413d617fe5f03` (upstream `737dd57e`) |
| Coreutils | `3b0f34b05e7ba4accb51bb9e9a0c1e502bd04620` (upstream `406e5a8bd`) |
| sed | `794cb2bc9b00ba17432fda74f1bc74d0965e46dc` (uutils/sed `c46dd6d`) |
| jaq-json, jaq-core, jaq-std | `c26f142e19128acda39f0c6668b38a581193b388` (jaq-json 2.0.3, `4229c5f`) |
| diffutils | uutils/diffutils `3f7a9a6ff3ee584d1cedbe013ebdebb437ad33b8` |
| Runtime, CLI and Rust SDK | Golem baseline above, rebuilt with this change |
| Rust compiler | `1.98.0 (88d9e12ae 2026-08-18)` |
| Standalone WASM runner | Wasmtime `46.0.1` |
| Guest artifact | `builtin-tools/bash.wasm` (built, not committed), release build, `golem:bash` / `bash@0.2.0` |
| Artifact size | 23,356,048 bytes (22.27 MiB) |
| Artifact SHA-256 | `95410bc60350d71ebd63fe69796e6c675b34d79b541e6430dbb42fc4e30df425` |

`cargo make build-bash-tool` built the component in the pinned container the README describes
(Build and verify), wrote it to `builtin-tools/bash.wasm` and passed
`wasm-tools validate --features all`. The build is reproducible: rebuilding from any checkout gives
the same SHA-256. The standalone workspace has
a committed lockfile and immutable Brush/Coreutils git pins, with no local fork overrides.

## Results

| Check | Result |
|---|---|
| Shared argv/help parser's moved tests | 15 passed; parser also checks on `wasm32-wasip2` without host features |
| Standalone workspace library tests | 371 passed: shell 213, component 8, wget 37, curl 87, HTTP transport 26 |
| Unit tests run as WASM | 141 passed under Wasmtime: shell 133, HTTP transport 8 (the component needs Golem's host; the rest need threads or `tempfile`) |
| Shell integration tests | 20 passed |
| Exact pinned Brush library tests | Core 141, builtins 28 and parser 276 passed (parser's YAML snapshot test is ignored upstream) |
| Conformance matrix | 2,253 per-PR cases passed on arm64 and on x86-64; exact shell exit code, stdout and stderr, against goldens recorded from Bash 5 and the GNU tools. Of the 18,008 cases including the sweep tier, the 238 that fail on each architecture are exactly those listed in `conformance/sweep-known-failures.json` |
| Oracle goldens | The goldens `stale` checked on 2026-09-30 matched a fresh run of the oracle image; the 51 cases added since, and one whose script changed, were each recorded from two fresh oracle containers that agreed |
| Feature checklist | 316 of 316 features have at least one matrix case |
| Brush compatibility suite | 2,541 cases run against the tool's shell; the 318 that fail match `conformance/compat/baseline.txt` exactly. One more, whose answer depends on timing in bash itself (`printf … | x=1` and SIGPIPE), may pass or fail |
| Harness self-tests | 21 passed, including missing/unexpected stderr, mismatched shell status, stale goldens and splitting a script into calls |
| Waits and result size, on a real server (by hand, with Bash provisioned as in the separate change) | A script polling a background job's file every 0.1 s, `sleep 15`, `timeout 2 sleep 60` and a call stopped at a 2 s time limit each complete with no suspension of the owner during the call, and a simulated crash after each leaves the owner healthy. A call returning 2 MiB on each stream, its `--lookup` and `golem agent oplog` after three such calls all succeed |
| Lint and format | Standalone workspace native and WASM Clippy, all targets/features, `-D warnings`; format checks passed |
| Dependency check | `cargo deny ... check advisories sources`: passed |

The checked-in `cargo make test-builtin-bash-pipelines` task passed on the pinned build. It builds
the same shell library as the tool, runs the harness self-tests, and compares all 2,253 per-PR matrix
cases in 45 corpora (`conformance/cases/`) with goldens recorded from Bash and GNU coreutils, grep,
sed, jq, diffutils, patch, findutils and file. The goldens were recorded in an environment set up
like a Golem agent's: no home directory, a uid with no passwd entry, and Golem's environment
variables. The task needs no Docker. The corpora cover
agent idioms, every builtin, compound commands, expansions, parameters and arrays, redirections,
options, traps, syntax errors, what does and does not carry across calls, the agent's environment, and `/dev`
paths as operands, besides the per-command cases. The task also fails when a feature on the
checklist (`conformance/features.py`) has no case. The example reports its actual shell status
separately from process termination: WASI's `cli/run` alone would collapse distinct nonzero
statuses. No stderr is globally discarded or normalized. A deliberate difference from Bash is a
per-case fixture with its reason. The same task's harness self-tests (`component/src/tests.rs`)
cover `coproc` and a refused command (`umask`), reached directly, through `eval`, inside a
function, and inside a trap: each fails with shell status 2 and no stdout, before any preceding
script effect.

The slow `cargo make test-builtin-bash-sweep` task runs Brush's own compatibility suite against the
tool's shell in Docker and compares the failing cases with the recorded baseline. It fails on a new
failure, and on a listed case that now passes, so fixes leave the list. It also checks every
matrix case, the sweep tier included, against its golden, and fails the same way against
`conformance/sweep-known-failures.json`. Whether the goldens still match the oracle is checked by
its own workflow (`builtin-bash-oracle-drift.yaml`), when the oracle or the goldens change and
weekly.

The pinned Brush checks cover capacities 1, 8 and 65,536 bytes, oversized writes, partial progress,
reader/writer clone lifetimes, EOF and BrokenPipe wakeups, legacy synchronous-input rejection,
early consumer exit, process status and nested task cleanup. These are tests of the actual pinned
fork, not a copied buffer implementation.

## Known gaps

- A sibling tool that itself waits 10 s or more can still have its owner suspended in the middle of
  a bash call; bash's own waits are bounded below that (see README, Limits).
- Optional Rust argument wire encoding; the tool uses supported non-optional shapes.
- Broader waiter/fairness and arbitrary chunk-boundary cases beyond the preserved cooperative
  regression suite.

The shell intentionally supports its documented embedded command options, not every option of
GNU Bash/Coreutils/curl/wget. Native embedding retains the source port's process-wide stdio/cwd
constraints and must serialize native sessions. The production WASM path uses invocation-owned
I/O and injected task services.

See [README](README.md#build-and-verify) and the [conformance matrix](conformance/README.md) for
reproducible commands. HTTP unit tests need loopback networking; recording goldens and the Brush
compatibility suite need Docker.
