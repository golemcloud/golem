# Bash tool conformance matrix

The matrix runs each case in the bash tool's shell and compares the exact exit status, stdout and
stderr with Bash 5 and the GNU tools. The shell side is the standalone `cooperative` example: the
same shell crate, with deterministic tool probes, under Wasmtime. It does not call a Golem server.
It runs as a WASI 0.3 command (`wasi:cli/run@0.3.0`, which `wasmtime run -Sp3` calls) on the
component's own task runtime, so `curl` and `wget` reach the network as the tool's do; their cases
use a port that refuses and a name under `.invalid`, which fail the same way on both sides.
WASI's exit interface reduces every nonzero code to failure, so the example writes its real exit
code to a separate report that the harness reads.

Both sides run in the environment an agent has: no HOME, a uid with no passwd entry, cwd `/`,
Golem's `GOLEM_*` variables, and a fresh `/tmp`. Bash's answers are recorded once into `goldens/`,
so checking needs no Docker; the goldens carry a fingerprint of the oracle image and environment,
and `check` refuses stale ones.

```sh
cargo build --manifest-path plugins/builtin-tools/bash/Cargo.toml \
  -p bash-shell --example cooperative --features test-support --release --target wasm32-wasip2
W=plugins/builtin-tools/bash/target/wasm32-wasip2/release/examples/cooperative.wasm
python3 plugins/builtin-tools/bash/conformance/conformance.py check --wasm $W     # against goldens
python3 plugins/builtin-tools/bash/conformance/conformance.py coverage --wasm $W  # checklist gate
python3 plugins/builtin-tools/bash/conformance/test_conformance.py              # harness tests
# After adding or changing cases (needs Docker; builds oracle.Dockerfile):
python3 plugins/builtin-tools/bash/conformance/conformance.py record --prefix '<text>'
```

`live` compares with the oracle directly and `stale` re-runs it to find goldens that no longer
match. Each case gets a fresh filesystem and a 15-second timeout; cases run in parallel, one per
CPU by default (`--jobs N`). `--case '<name>'` selects one case and `--prefix '<text>'` every case
whose name starts with it; repeat either to select more. `--report FILE` writes the failures as
JSON. The matrix needs Wasmtime with WASI 0.3/component-model async support.

`record` runs the oracle **twice** per case by default, each run in its own fresh container, and
refuses to write a golden the two runs disagree on -- printing which case and which field (status,
stdout or stderr) differed. A case whose answer depends on this run's own randomness (a PID,
`$RANDOM`, an untrimmed wall-clock time) would otherwise poison the golden with a snapshot that
stops matching the very next time the oracle happens to run it, which is exactly the kind of thing
`stale` exists to catch later -- catching it at record time instead means a case has to be made
deterministic (or given a fixture) before it is ever recorded, and `stale` only ever has to explain
a real change in the oracle. `--runs 1` records with a single pass, for quick local iteration when
you are not yet ready to commit a golden (it skips the determinism check entirely, so a case
recorded this way is not proven deterministic).

The oracle image pins its base by digest: Bash 5 on Alpine with the GNU tools installed over
BusyBox (currently coreutils 9.11, grep 3.12, sed 4.9, jq 1.8.2, diffutils 3.12, patch 2.8,
findutils 4.10.0, file 5.47, curl 8.22.0 and wget 1.25.0), run with `LC_ALL=C.UTF-8`. The packages
come from Alpine's index when the image is built, so a rebuild can pick up newer versions; the
drift workflow below is what catches that. `oracle_fingerprint()` hashes the whole Dockerfile, so
changing it (for instance to pin those package versions too) invalidates every recorded golden and
forces a re-record.

Each corpus is a module in `cases/` that defines `CASES` as `(name, script)` or
`(name, script, tags)`. Where the tool deliberately differs from bash, a corpus adds a fixture
with its reason: `EXPECTED` (status, stdout, stderr, reason) or `EXPECTED_STDERR` (stderr only);
`check` flags a fixture that has come to match bash, so fixtures cannot outlive their reason. A
script with `#--call--` lines runs as separate calls, each a fresh shell as each `run` of the tool
is: our side starts every call in the directory the previous one ended in, as a caller passing back
the returned `cwd` does, and the oracle runs each call as its own `bash -c` in the directory the
marker names (`#--call-- /tmp/w`; `/` when it names none). `features.py` lists the features of the bash manual and every registered command;
`coverage` fails when one has no case, or no `n/a` reason.

## Brush's compatibility suite

`compat/` runs Brush's own suite (about 2,500 YAML cases, each compared with a real bash) against
the tool's shell, as the `compat` example (a bash-like command line over the same shell) under
Wasmtime, inside a Linux image with bash 5 and GNU tools:

```sh
cargo build --manifest-path plugins/builtin-tools/bash/Cargo.toml \
  -p bash-shell --example compat --release --target wasm32-wasip2
plugins/builtin-tools/bash/conformance/compat/run-in-docker.sh <brush checkout> <compat.wasm> > out.txt
python3 plugins/builtin-tools/bash/conformance/compat/baseline.py check out.txt
```

`compat/baseline.txt` lists the cases that fail today; `baseline.py check` fails on a new failure
and on a listed case that now passes, and `baseline.py update` rewrites the list. Many listed
cases run a script from standard input or a file, or interactively, where bash behaves as it does
not under `-c`; the tool always runs a script as `bash -c` would.

| Corpus | Cases |
|---|---|
| `cases/agent_idioms.py` (commands agents write) | 119 |
| `cases/builtins.py` (each Bash builtin) | 87 |
| `cases/command_errors.py` (utility and HTTP usage errors) | 16 |
| `cases/compound.py` (compound commands, `select`, `for ((;;))`) | 31 |
| `cases/core.py` (pipes, SIGPIPE, traps, jobs, tool probes) | 118 |
| `cases/coreutils_errors.py` | 47 |
| `cases/coreutils_extra.py` | 71 |
| `cases/coreutils_edge.py` (arithmetic, format directives, locale, huge widths, non-regular inputs) | 92 |
| `cases/diagnostics.py` | 19 |
| `cases/diff.py` (diff, cmp, patch) | 76 |
| `cases/env_facts.py` (what an agent's environment looks like) | 30 |
| `cases/expansions.py` (expansions, globbing, process substitution) | 70 |
| `cases/find.py` | 67 |
| `cases/grep.py` | 66 |
| `cases/inspect.py` (`file`, `stat`, `which`, `man`) | 15 |
| `cases/jobs.py` (jobs, traps and signals in the process model) | 143 |
| `cases/jq.py` | 95 |
| `cases/options.py` (`set` and `shopt` options) | 18 |
| `cases/params.py` (parameters and arrays) | 33 |
| `cases/parsing.py` (parser, tokenizer, expansion syntax; jq's parsing) | 231 |
| `cases/redirect.py` (redirections, `{var}>`, descriptor moves) | 26 |
| `cases/redirections.py` (`exec`) | 7 |
| `cases/refusals.py` (refusals and the builtins at their edge) | 45 |
| `cases/runtime.py` (shell runtime semantics) | 148 |
| `cases/sed.py` | 74 |
| `cases/sh.py` (`sh -c`, `bash -c`) | 13 |
| `cases/siblings.py` (concurrent sibling calls) | 3 |
| `cases/special_paths.py` (`/dev/null`, `/dev/stdin`, `-` and `/dev/stdout` as operands (generated)) | 124 |
| `cases/state.py` (separate calls: only the working directory carries (`#--call--`)) | 15 |
| `cases/syntax.py` (syntax and syntax errors) | 20 |
| `cases/text_tools.py` (grep, diff, sed, jq, patch, cmp) | 69 |
| `cases/tilde.py` (`~` with and without a home directory, in assignment-like words) | 12 |
| `cases/tool_layer.py` (resource bounds, `/dev`, the utilities' streams and environment) | 151 |
| `cases/traps.py` (EXIT, ERR, DEBUG, RETURN) | 18 |
| `cases/xargs.py` | 32 |
| **Total, per PR** | **2201** |
| `cases/sweep_options_*.py` (sweep tier: every option of every registered command; 7 modules) | 4104 |
| `cases/sweep_grammar_*.py` (sweep tier: Bash grammar, generated from fixed seeds; 10 modules) | 4231 |
| `cases/sweep_realworld_*.py` (sweep tier: tldr-pages examples and coding-agent command shapes, see `cases/NOTICE`; 5 modules) | 3495 |
| `cases/sweep_edge_*.py` (sweep tier: Unicode, invalid UTF-8, binary, empty and large input, `/dev` paths, separate calls, error paths; 7 modules) | 3944 |
| **Total, sweep job** | **17975** |

## Two tiers

A corpus that sets `TIER = "sweep"` is checked only with `--tier all`; `check`'s default (`--tier
pr`) skips it. The sweep-tier corpora carry no fixtures. The sweep cases where the tool still
differs from Bash are listed in `sweep-known-failures.json`; the slow job passes
`--known-failures` with it, so it fails on a case that fails but is not listed, and on a listed
case that now passes (remove it from the list, or rewrite the list from a full run with
`--update-known-failures`). A per-PR case never goes in that list.

| Tier | Runs against goldens in | Job |
|---|---|---|
| `pr` (default) | every PR and push (`cooperative-pipelines`) | `test-builtin-bash-pipelines` |
| `all` (`pr` + `sweep`) | every PR and push, alongside Brush's compat suite (`conformance-sweep`) | `test-builtin-bash-sweep` |

Neither job re-runs the oracle: both compare the shell's own output with what is already recorded
in `goldens/`, so a PR needs no Docker to pass. Checking against a *live* oracle (`stale`) is a
third, separate concern below.

## Oracle drift

`stale` re-runs the live oracle for every case (both tiers) and reports one whose answer no longer
matches its recorded golden -- catching either a genuine oracle change (the Dockerfile was edited
on purpose) or a case that slipped past `record`'s determinism check some other way (recorded with
`--runs 1`, say, or before that check existed). It is deliberately **not** run on every push: it
is the slowest thing in this directory (re-running ~17,000 cases against Docker containers, not
just comparing already-recorded JSON), and its failures are about the oracle's own answer, not
about anything a given PR touched, so running it per-push would fail unrelated PRs whenever the
oracle happens to drift.

It runs instead in its own workflow, `.github/workflows/builtin-bash-oracle-drift.yaml` (job
"Oracle drift (stale goldens)", `cargo make test-builtin-bash-oracle-drift`), triggered only by:
- a push or PR touching `oracle.Dockerfile`, `goldens/**` or `conformance.py` itself (a `paths:`
  filter on that workflow's own `on:`, not a job-level condition inside the shared
  `builtin-bash.yaml` -- the other jobs there keep running on every bash change regardless);
- a weekly `schedule:`, for the oracle drifting on its own with nothing in this repo changing
  (Alpine's index publishing a newer package, say);
- `workflow_dispatch:`, to run it by hand.
