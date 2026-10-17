# Node tool process boundary probe

This is a feasibility probe, not a `node:child_process` implementation. It checks whether
synchronous JavaScript can wait for an asynchronous host function and a separately scheduled
component without a nested guest executor or an rquickjs fork.

The Rust harness is `golem-common/tests/node_tool_processes.rs`. It uses Golem's shared production
Wasmtime configuration. No test invokes a compiler or an operating-system subprocess.

## Cases

The default test uses a small stackless P3 caller and a fresh child Store. The opt-in test uses
the released wasm-rquickjs compiler to build this JavaScript fixture before testing:

- `run` calls a synchronous native import from the component's main QuickJS runtime.
- `run-job` calls the same import from a fresh `startJavaScript` execution runtime, through
  `runJavaScript`. It rejects an inherited parent global or a Promise-valued native return.
- The direct-host control yields and returns the number plus one. The nested-component case
  yields, starts an independently scheduled child Store, and obtains the same result.
- Five interleaved repetitions cover both entries and both host cases. Each parent has a fresh
  Store. The harness requires twenty host calls and ten child calls.

Only the fixture directory is preopened, read-only, as the guest root. The children do not access
the owner's filesystem. Passing these tests does not establish Golem tool admission, permission,
filesystem lane, attachment, cancellation or replay correctness.

## Reproduce

Run from the Golem repository root with wasm-rquickjs CLI 0.4.13 and the Rust `wasm32-wasip2`
target available. `probe_cli` must point to that compiler; this does not replace an installed CLI.
The wrapper is generated outside the checkout. Encoding support is needed by the fixture runtime.

```sh
probe_cli=/path/to/wasm-rquickjs
probe_dir=$(mktemp -d /private/tmp/node-tool-processes.XXXXXX)
"$probe_cli" generate-wrapper-crate \
  --wit golem-common/tests/fixtures/node-tool-processes/wit \
  --js golem-common/tests/fixtures/node-tool-processes/probe.js \
  --target wasi-p3 --output "$probe_dir/component"
(cd "$probe_dir/component" && cargo build --offline --release \
  --target wasm32-wasip2 --no-default-features --features p3,encoding -j 4)
"$probe_cli" optimize \
  --input "$probe_dir/component/target/wasm32-wasip2/release/probe.wasm" \
  --output "$probe_dir/probe-wizer.wasm"
RUST_MIN_STACK=33554432 \
GOLEM_NODE_TOOL_PROBE_WASM="$probe_dir/probe-wizer.wasm" \
cargo test --locked --offline --profile dev-ci -j 4 \
  -p golem-common --test node_tool_processes -- \
  --include-ignored --nocapture --report-time
```

The opt-in test is ignored unless explicitly selected with `--include-ignored`. It requires a
prebuilt component at `GOLEM_NODE_TOOL_PROBE_WASM`. The default engine test needs no Wasm fixture.

## Host scheduling constraint

A concurrent host callback must not recursively drive a second Store's event loop. An earlier
probe called the child's `TypedFunc::call_async` directly in that callback. LLDB stopped at
Wasmtime's `check_recursive_run` guard: `Recursive StoreContextMut::run_concurrent calls not
supported`. The panic aborted the native process; increasing the native stack did not help.

The probe now schedules the child as a separate Tokio task and aborts it on observer drop. This
is a harness scheduling correction, not a demonstrated QuickJS or Golem defect. Production code
must use Golem's tracked entity tasks, causal admission, explicit tool cancellation and cleanup,
not copy the probe's bare task scheduler.

## Local observations

The [October 17 observations](observations-2026-10-17.json) record the Wizer-enabled run on
Golem main `7a787a6394eee74318d32792d8b2402c5fe2ea22`, with wasm-rquickjs CLI 0.4.13. Both focused
tests passed, including the twenty-case JavaScript matrix and its exact invocation counts.

| Entry | Direct-host median | Nested-component median |
| --- | ---: | ---: |
| Main runtime | 0.355 ms | 0.421 ms |
| Fresh execution job | 76.204 ms | 76.261 ms |

Each value summarizes five interleaved samples, including the first sample. These are scalar
boundary timings with a release-built Wasm fixture and a native `dev-ci` harness. They exclude
component compilation and parent Store instantiation, but include the entry's QuickJS work and
any fresh execution-job startup. They do not measure a Golem tool or establish a production
performance difference. The recorded maximum RSS includes the whole mixed-case native test
process and its component compilation, not an individual invocation.

The generated wrapper, dependency lockfile and Wasm artifacts are machine-local temporary files,
not committed binaries. The observations retain their hashes and sizes. The reproduction commands
generate a new dependency lockfile, so they are not a promise of a bit-identical rebuild.

## Async import lowering experiment

A separate local variant changed `execute` to `async func` in the generated crate's WIT and
added this option to its `wit_bindgen_p3::generate!` invocation:

```rust
async: ["-import:experiment:node-tools/host#execute"],
```

The generated synchronous native JavaScript wrapper was retained for that experiment. It passed
the main-runtime direct-host/nested-component matrix after the scheduling correction. This is a
binding-generation experiment, not an option currently exposed by wasm-rquickjs CLI. A production
generator option must keep Rust bindings, JavaScript wrappers and declarations consistent.

## Production integration boundary

Keep Golem-specific behavior in Golem:

- Discover tools by `lookup-name` through the current caller's tool surface. Reuse the parser in
  `golem-schema/src/schema/tool/argv.rs`; do not add another CLI grammar.
- Reuse Bash's result policy: named `ProcessExitStatus`, declared error statuses and redacted
  structured results. Node's spawn errors must remain distinct from a started command's exit.
- Reuse tool dispatch, permission checks, the owner's causal filesystem lane, attachment limits,
  durable records and explicit cancellation. Do not add a second oplog or independent authority.
- Drain terminal, stdin, stdout and stderr concurrently without requiring JavaScript callbacks
  while a synchronous call is blocked. The current scalar probe does not establish this.
- Preserve local JavaScript and Node-shebang resolution before trying bound tool names. Delegate
  shell syntax to the bound Bash tool, not an operating-system shell.
- Enable tool commands by default, with a per-component disable control inherited by fresh
  runtimes. This compatibility switch does not replace tool bindings or permissions.
- Make registration safe during Wizer initialization. Read tool bindings and caller authority
  lazily during an invocation, not while registering the backend.

wasm-rquickjs should supply only a generic, runtime-local backend/initialization mechanism and,
if needed, configurable synchronous import bindings. A TypeScript-only bundle rewrite does not
cover fresh execution runtimes or all Golem JavaScript SDKs. Avoid new Rust feature flags unless
the measured implementation actually requires them.

The next acceptance gate is a real synchronous JavaScript call into a filesystem-capable Golem
tool, with bounded output, then cancellation and reconstruction. Do not treat the scalar tests
as that acceptance gate or as evidence of production performance.
