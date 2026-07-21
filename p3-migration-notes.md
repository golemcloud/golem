# WASI p2 → p3 migration notes

## Blocker 0: We must support BOTH p2 and p3 WASI host interfaces

**Decision: the worker executor has to link p2 *and* p3 WASI host
implementations side-by-side. A p3-only linker is not enough.**
This contradicts the current
[golem-worker-executor/src/wasi_host/mod.rs](file:///Users/vigoo/projects/golem/golem/golem-worker-executor/src/wasi_host/mod.rs),
where `create_linker` only calls `wasmtime_wasi::p3::add_to_linker` /
`wasmtime_wasi_http::p3::add_to_linker`.

### Why (empirical finding)

Real guest components — even ones authored for p3 — import a *mix* of p2
and p3 interfaces, because the Rust standard library is not migrated to
p3 yet:

- There is **no stable-toolchain path** to compile a Rust program to a
  pure-p3 component today. `wasm32-wasip3` is a Tier 3 rustc target with
  **no precompiled std** (`rustup target list` offers no wasip3 std), so
  it requires `-Z build-std`, which is nightly-only. The stable
  preview1→component and `wasm32-wasip2` paths only ever emit p2.
- Even when you *do* build for `wasm32-wasip3` on nightly, std still
  imports p2: Rust's `library/std/Cargo.toml` maps **both**
  `target_env = "p2"` and `target_env = "p3"` to the same
  `wasi = "0.14.4"` (package `wasi`, the WASI 0.2 / p2 bindings crate).
  So anything touching `std` (`println!`, env, clocks, fs, …) lowers to
  p2 `wasi:io` / `wasi:cli` imports regardless of the target.

### Reproduction (kept at `tmp4/wasip3-hello-test/`)

A minimal stable build that:
- depends on `wasip3 = "=0.6.0+wasi-0.3.0-rc-2026-03-15"` and
  `wit-bindgen = "=0.57.1"` (`async` feature),
- exports a p3-native `compute: async func(n: u32) -> u32`,
- in the body calls `wasip3::random::random::get_random_u64()` (a p3
  import) and `println!` (std → p2),
- compiles with plain `cargo build --release --target wasm32-wasip2`
  (no nightly).

`wasm-tools validate --features all` passes and `wasm-tools component wit`
shows the imports are split across p2 and p3:

```
world root {
  import wasi:random/random@0.3.0-rc-2026-03-15;   // p3 (wasip3 crate)
  import wasi:io/poll@0.2.6;                        // p2 (std)
  import wasi:io/error@0.2.6;                       // p2 (std)
  import wasi:io/streams@0.2.6;                     // p2 (std)
  import wasi:cli/environment@0.2.6;                // p2 (std)
  import wasi:cli/exit@0.2.6;                       // p2 (std)
  import wasi:cli/stdin@0.2.6;                      // p2 (std)
  import wasi:cli/stdout@0.2.6;                     // p2 (std)
  import wasi:cli/stderr@0.2.6;                     // p2 (std)
  import wasi:cli/terminal-*@0.2.6;                 // p2 (std)
  export test:p3demo/demo@0.1.0;                    // p3 async export
}
```

Note in particular that `wasi:io@0.2.6` (`poll` / `error` / `streams`) —
which "does not exist in p3 at all" per the notes below — is imported by
ordinary std I/O. The executor must therefore keep providing the p2
`wasi:io` host, the p2 `wasi:cli` host, etc., *and* the p3 interfaces our
own WIT and the `wasip3` crate pull in.

### Consequences

- `wasi_host::create_linker` must register p2 **and** p3 WASI/HTTP host
  trees. Re-adding the p2 `add_to_linker` calls removed in
  "Blocker 2 (resolved)" / "Blocker 3" below is now required, not
  optional — they were deleted on the false assumption that p3-only is
  sufficient.
- The durability story spans both ABIs: std-driven side effects come
  through p2 host fns (sync, pollable-based) while p3-native guest code
  comes through p3 async host fns. Durable wrappers are needed on both
  until std itself moves to p3 upstream.
- This is effectively a long-lived "p2 + p3 coexistence" requirement,
  not a transition we can flip in one cut. It only ends when Rust std
  (and other languages' stdlibs) ship p3-native WASI.

---

## Known blockers (deferred, must be solved before merge)

### Blocker 1: Durability layer is not concurrency-aware

The p3 component model lets the guest issue multiple async host calls
concurrently (e.g. parallel RPC, parallel HTTP). With the Accessor-based
host method shape, no `&mut store` is held across awaits, so concurrent
host futures actively *do* overlap.

Our current durability/oplog layer assumes a strictly linear sequence of
durable operations per worker:
- `Begin … End` markers matched positionally on replay
- Single in-flight remote write at a time
- Idempotency / future-invocation slots are singletons

We **cannot** simply serialize durable ops per worker, because the
parallelism that p2 expressed via pollables (parallel RPC, parallel HTTP,
parallel keyvalue ops, etc.) is now expressed as concurrently-awaited
async host calls. Serializing them would regress real functionality.

Required design change (original sketch — **superseded, see
"Implemented design" below**):
- New oplog schema with per-call `call_id`, split into
  `DurableOpStart { call_id, … }` and `DurableOpEnd { call_id, result }`
- Per-worker monotonic `call_id` allocator, incremented synchronously
  when the guest enters a durable host method (initiation order is
  deterministic; completion order is not)
- Replay engine resolves recorded results by `call_id` in recorded
  completion order (driving suspended replay futures via oneshot
  channels), reproducing the original observable completion order
- New `Durability` API: `start_call(...) -> CallHandle`, `CallHandle::finish(result)`
- Idempotency keys / Begin-End nesting re-expressed per `call_id`
- Oplog format version bump + migration of old entries
  (treat each existing entry as Start immediately followed by matching End)

Proposed migration order once we pick this back up:
1. Land oplog schema change (Start/End with call_id), still strictly
   sequential at runtime — semantic no-op, unblocks the rest.
2. Rewrite `Durability` API to start_call/finish shape, port all callers.
3. Implement p3 accessor-based host stubs against the new `Durability`.
4. Switch replay engine to concurrent (resolve futures by call_id in
   recorded completion order). Add tests for overlapping durable RPC.
5. SDK regen + end-to-end p3 component runtime tests.

#### Implemented design (supersedes the `call_id` sketch above)

The concurrent-durability redesign has landed ("Concurrent durability",
#3628, plus the follow-up concurrent resolver work). The authoritative
design document is `concurrent-durability.md`. The implementation kept the
sketch's Start/End split and its resolve-by-identity replay model, but
replaced the identity scheme:

- **No `CallId` type and no allocator. A durable call is identified by
  the `OplogIndex` of its `Start` entry.** The oplog index is already a
  per-worker-monotonic, append-assigned identifier, so a separate
  `call_id` counter would have duplicated it while adding failure modes
  the index doesn't have: recovery seeding after a crash, re-seeding on
  worker fork, and keeping counter and oplog in sync across commits. The
  `Start` stores no id field at all — its identity is its own index,
  assigned by the append.
- **Three entries**: `Start { parent_start_index, function_name,
  request: Option<…>, durable_function_type }`, `End { start_index,
  response: Option<…>, forced_commit }`, and the third terminal
  `Cancelled { start_index, partial: Option<…> }` (anticipated by the
  structured-concurrency note below — the loser of a guest `race`/
  `select!` is cancelled, not leaked). `End`/`Cancelled` reference their
  `Start` via `start_index`; nesting uses `parent_start_index` on
  `Start` (scopes such as batched writes are request-less `Start`s their
  child calls point back at).
- **No oplog format version bump, and no entry migration was
  implemented.** The schema swap replaced `HostCall`/`BeginRemoteWrite`/
  `EndRemoteWrite` with the three new entries in one step, deliberately
  with **no compatibility shim for the deleted variants** (see
  `concurrent-durability.md`, decision 2): oplogs persisted by builds
  older than the swap that contain those variants are *not* decodable
  by post-swap builds. Every other entry's `desert` encoding stayed
  stable (the `keep_existing`/`keep_preexisting` binary-tag tests in
  `golem-common` pin this). The compatibility that *is* guaranteed —
  and tested — begins at the swap: it shipped together with a
  sequential legacy adapter that wrote completed host calls as an
  *adjacent* `Start`+`End` pair appended atomically (`Oplog::add_pair`,
  preserving the old single-entry crash window), and oplogs recorded by
  those sequential builds replay through the concurrent resolver
  unchanged. Covered by the compatibility test
  `pre_migration_adjacent_pair_oplog_replays_through_concurrent_resolver`
  in `golem-worker-executor/src/durable_host/replay_state.rs`.
- **Replay** resolves by identity, as sketched, but keyed by the `Start`
  index: each pending `CallHandle` registers a oneshot awaiter with the
  concurrent resolver, and the replay cursor routes each `End`/
  `Cancelled` to the awaiter registered for its `start_index`, so
  overlapping calls' terminals replay in recorded completion order
  regardless of claim order. Scope `End`s are folded into the same
  resolver instead of positional reads.

While the blocker was unsolved, the following host methods were
intentionally left as `unimplemented!()` `TODO(p3)` stubs (workspace
builds, runtime not ported). All of them have since been implemented on
top of the landed design; their durability status is tracked in the
"Durability audit" section at the end of this document:
- `HostFutureInvokeResultWithStore::get`
- `HostGetPromiseResultWithStore::get`
- keyvalue cache `Future*ResultWithStore::get`
- blobstore async stream-returning methods
- keyvalue async stream-returning methods

Concrete follow-up: before calling the p3 host migration runtime-complete, replace
`HostFutureInvokeResultWithStore::get` and `HostGetPromiseResultWithStore::get`
with real accessor-based durable implementations. These are guest-visible p3
async APIs (`invoke_result.get().await`, `promise.get().await`); leaving them as
`unimplemented!()` is acceptable only for a build-only intermediate state and
will trap any p3 component that awaits an RPC result or promise.

Also deferred:
- `lazy-initialized-pollable` in `golem-durability` — removed from active
  WIT and host impl deleted. Since resolved (T26): removed permanently with
  no replacement; see "Resolved (T26)" section below.

---

### Blocker 3: WASI durability is regressed (no longer p2-wrapped)

This blocker is the consequence of resolving Blocker 2 the simplest way. We
removed every `wasmtime_wasi::p2::*` durable wrapper from the executor and
replaced the entire wiring with bulk `wasmtime_wasi::p3::add_to_linker(...)`
plus `wasmtime_wasi_http::p3::add_to_linker(...)`. The whole standard WASI
surface is now served by **wasmtime-wasi's default p3 host implementations**,
which means:

- WASI calls are no longer recorded in the oplog: clocks (`now`,
  `wait_until`, `wait_for`), random, filesystem read/write, sockets, HTTP,
  cli env/exit/stdin/stdout/stderr.
- Replay is no longer deterministic for any workload that observes a WASI
  side-effecting result (time, random bytes, file contents at a moment,
  HTTP response, env vars at a moment, etc.).
- Per-worker stdio capture is replaced with a "raw async stdio" placeholder
  in [`DurableWorkerCtx::create`](file:///Users/vigoo/projects/golem/golem/golem-worker-executor/src/durable_host/mod.rs); previously this went through
  `ManagedStdIn/Out/Err` from `durable_host/io/` (now deleted). Worker
  output is currently piped to the executor process's stdio.
- `WasiHttpHooks` (custom outgoing-request interception that backed durable
  HTTP) is gone; `as_wasi_http_view` returns the wasmtime-default
  `default_hooks()`. The `HttpRequestState::outgoing_request_config()`
  helper was removed.
- `WebSocketConnectionEntry`, `FutureInvokeResultEntry`,
  `GetPromiseResultEntry` no longer have `wasmtime_wasi::p2::Pollable`
  impls — these need to be re-exposed via the p3 accessor pattern as part
  of Blocker 1.
- All `From<...>` conversions between `SerializableXxx` types in
  [`golem-common/src/model/oplog/payload/types.rs`](file:///Users/vigoo/projects/golem/golem/golem-common/src/model/oplog/payload/types.rs) and the wasmtime
  HTTP/socket/filesystem types were removed; the `Serializable*` types
  themselves are kept for oplog format compatibility.

What remains the same:
- Golem-owned WIT durability (oplog API, durability API, agents, retry,
  context, RPC, websocket) is still wired through our bindgen at
  [`preview2/mod.rs`](file:///Users/vigoo/projects/golem/golem/golem-worker-executor/src/preview2/mod.rs); host trait impls live under
  `durable_host/{golem,blobstore,keyvalue,logging,config,rdbms,websocket,quota,wasm_rpc}/`.
  These are unaffected by the WASI removal.
- `wasm_component_model_async(true)` and
  `wasm_component_model_error_context(true)` are now set on every wasmtime
  `Config` site (worker-executor, compilation service, agent extraction).
- The two engine `Config` sites used at agent type extraction
  ([`golem-common/src/model/agent/extraction.rs`](file:///Users/vigoo/projects/golem/golem/golem-common/src/model/agent/extraction.rs)
  and [`cli/golem-cli/src/model/agent/extraction.rs`](file:///Users/vigoo/projects/golem/golem/cli/golem-cli/src/model/agent/extraction.rs))
  use `wasmtime_wasi::p3::add_to_linker(...)`. The CLI variant uses an
  in-house `MemoryWriter` instead of `wasmtime_wasi::p2::pipe` so it has
  no p2 surface dependency.

Required follow-up work to restore durability under p3:
1. Re-introduce per-interface durable wrappers under
   `golem-worker-executor/src/durable_host/wasi/` (or similar). For each:
   - sync p3 `Host` impl (most clocks/random/cli/sockets/filesystem methods),
   - async `HostWithStore` impl using `Accessor` (monotonic `wait_*`,
     async sockets bind/listen/send/receive, etc.).
   - In each impl, surround the underlying `WasiCtxView` call with
     start/persist/replay logic from the new concurrent-aware durability
     API to be designed under Blocker 1.
2. Override the wasmtime defaults via `linker.allow_shadowing(true)` and
   re-register our durable variants after the bulk `add_to_linker`.
3. Restore `ManagedStd{In,Out,Err}` (or a p3 equivalent) so worker stdio
   is captured per-worker, not piped to the executor process.
4. Restore custom `WasiHttpHooks` against the p3 hooks API for HTTP
   request/response oplog persistence. **Done — implemented and
   runtime-verified (checklist item #8 is `done`; see the resolved
   cross-cutting decision 1 below for the authoritative status).**
   `as_wasi_http_view_p3` exposes `DurableHttpHooks` instead of
   `wasmtime_wasi_http::p3::default_hooks()`, and the p3 `client::send` /
   `HostResponseWithStore::consume_body` wrappers
   (`golem-worker-executor/src/durable_host/p3/http/`) record the request,
   response head, body chunks, and trailers to the oplog and replay them
   without network I/O. On replay `send` consumes the request by mirroring
   the live path minus the network so it does not leak, streaming-body
   guests do not block, the recorded response head is returned without
   waiting for the upload to finish (matching live `WasiHttp::send`,
   which spawns its body-I/O future), and deterministic body-transmission
   errors (e.g. `HttpRequestBodySize`) replay correctly. The
   runtime-harness gap that initially kept row #8 blocked was closed by
   T40 (`test-components/http-tests` migrated to P3 `wasi:http`; the full
   `http_tests`-tagged worker-executor suite passes against the durable
   P3 path). The request-body transmission *result* is also recorded and
   replayed durably: the durable `request::new` interposes on the
   guest-facing transmission future
   (`pending_p3_http_request_transmissions` in `durable_host/mod.rs`) and
   `client::send` records/replays the result via the demand-gated
   transmission recorder (`start_transmission_recording` in
   `durable_host/p3/http/send.rs`), so non-deterministic mid-body upload
   errors no longer replay as `Ok(())`.
5. Re-add `From<...>` conversions in
   `golem-common/src/model/oplog/payload/types.rs` against the p3
   wasmtime types where round-trip with wasmtime is needed. Audit signed p3
   `system-clock.instant` conversions explicitly: p3 instants allow negative
   seconds, so pre-epoch values must not silently clamp to the Unix epoch, wrap at
   p2 boundaries, or panic when converting through `SystemTime`.

Until done, the workspace builds and any `wasi:cli/command` style p3
component should be runnable by the executor — but there is no replay
determinism for WASI side effects.

---

### Blocker 2 (resolved): Whole WASI host implementation was p2

All `wasmtime_wasi::p2::*` and `wasmtime_wasi_http::p2::*` host wiring has
been removed from the workspace. The only remaining mentions are TODO
comments documenting where p2 used to live for future re-implementation.
This was resolved by Blocker 3 above (we accepted the WASI durability
regression to land the structural migration).

---

### Original Blocker 2 detail (kept for reference)

Although the WIT was migrated to p3 and new bindgen-generated p3 host traits
exist (with `unimplemented!()` stubs for the async ones from Blocker 1),
**the engine is still being linked with the entire `wasmtime_wasi::p2::*`
host tree**. The build is green only because wasmtime v45 retains both
`p2` and `p3` modules side-by-side, but at runtime the executor is wired
to p2 host implementations that no longer match the p3 WIT we hand out
to guests.

Concretely, every entry under [golem-worker-executor/src/wasi_host/mod.rs](file:///Users/vigoo/projects/golem/golem/golem-worker-executor/src/wasi_host/mod.rs)
calls `wasmtime_wasi::p2::bindings::<x>::add_to_linker::<...>` for:
- `cli::{environment, exit, stderr, stdin, stdout, terminal_input,
  terminal_output, terminal_stderr, terminal_stdin, terminal_stdout}`
- `clocks::{monotonic_clock, wall_clock}`
- `filesystem::{preopens, types}`
- `io::{error, poll, streams}` (these *don't exist in p3 at all*)
- `random::{random, insecure, insecure_seed}`
- `sockets::{instance_network, ip_name_lookup, network, tcp,
  tcp_create_socket, udp, udp_create_socket}`
- plus `wasmtime_wasi_http::p2::bindings::http::{outgoing_handler, types}`

And the corresponding durable wrappers under
`golem-worker-executor/src/durable_host/{cli,clocks,sockets,...}` are
all still implementing `wasmtime_wasi::p2::bindings::*::Host` traits.

Other p2-shaped pieces still live:
- `golem-common/src/model/agent/extraction.rs` builds an `Engine` with
  `wasmtime_wasi::p2::add_to_linker_with_options_async` and a
  `WasiCtx`/`IoCtx`/`IoView` based on p2. It runs at component upload
  time to discover agent types and will fail to instantiate p3 guests.
- `golem-debugging-service/src/debug_context.rs` implements
  `wasmtime_wasi::p2::bindings::cli::environment::Host` for its context.
- `golem-worker-executor/src/durable_host/mod.rs` imports `FsResult`,
  `Descriptor`, `HostFutureIncomingResponse`, `OutgoingRequestConfig`,
  and other `wasmtime_wasi::p2::*` / `wasmtime_wasi_http::p2::*` types.
- `golem-worker-executor/src/durable_host/golem/v1x.rs` implements
  `wasmtime_wasi::p2::Pollable for GetPromiseResultEntry` (no analogue
  in p3 — see Blocker 1).
- `golem-wasm/src/lib.rs` re-exports `wasmtime_wasi::p2::DynPollable` and
  implements `wasmtime_wasi::p2::Pollable for FutureInvokeResultEntry`.
  Used by host RPC plumbing for what was previously a pollable-driven
  invoke result.
- `golem-common/src/model/oplog/payload/tests.rs` still depends on p2
  socket and http types for some serialization fixtures.

This is the actual reason the workspace builds despite the WIT move: we
haven't deleted any of the p2 wiring, we just added p3 stubs alongside.

Required work:
1. Replace `wasi_host::create_linker` so it uses
   `wasmtime_wasi::p3::add_to_linker` (and `wasmtime_wasi_http::p3::*`
   equivalents) for everything WASI provides natively. Delete per-interface
   add_to_linker calls that duplicate that.
2. Decide for each currently-durabilized WASI interface whether we still
   need a custom durable wrapper (clocks, sockets, filesystem,
   stdin/stdout, http) and, if so, port it from
   `impl wasmtime_wasi::p2::bindings::<x>::Host for DurableWorkerCtx<Ctx>`
   to `impl wasmtime_wasi::p3::bindings::<x>::Host for DurableWorkerCtx<Ctx>`.
   Note that p3 host trait shapes differ (no pollables; some methods are
   `async` and use the Accessor pattern from Blocker 1).
3. Port `extract_agent_types_with_streams` to p3:
   `wasmtime_wasi::p3::add_to_linker`, p3 `WasiCtx`/`WasiCtxView`/`WasiView`
   replacements, and add `wasm_component_model_async(true)` /
   `wasm_component_model_error_context(true)` to its `Config`.
4. Port `golem-debugging-service` debug context off p2 cli::environment.
5. Replace p2 `Pollable` implementations (`FutureInvokeResultEntry`,
   `GetPromiseResultEntry`) with the p3-native async-host shape — this
   is essentially the same work as Blocker 1. As part of this, remove stale p2
   `subscribe`/child-pollable bookkeeping once the p3 `get` implementations are
   real, so future conflict resolutions do not accidentally reason in p2 terms.
6. Replace p2 imports in `oplog/payload/tests.rs` fixtures with p3
   equivalents (likely just type renames for sockets/http addresses).

Until done, anything that actually instantiates a guest under our
runtime will either:
- hit duplicate/conflicting linker entries (p2 `wasi:io/*`, p2
  `wasi:clocks/wall-clock`, etc. that don't exist in our WIT), or
- fail to provide the new p3 interfaces that the WIT now imports
  (`wasi:clocks/system-clock`, async types/streams in cli/sockets/http).

---

## Decisions / facts settled so far

- Wasmtime upgraded to v45.
- WASI p3 version in use: `0.3.0-rc-2026-03-15`.
- Root cargo features set: `wasmtime` enables `component-model`,
  `component-model-async`, `component-model-async-bytes`, `anyhow`;
  `wasmtime-wasi` and `wasmtime-wasi-http` enable `p3`.
- `wasi:io` no longer exists in p3. Streams/futures are component-model
  built-ins (`stream<T>`, `future<T>`, `async func`); `pollable` is gone.
- Clocks renamed: `wasi:clocks/wall-clock` → `wasi:clocks/system-clock`,
  `Datetime` → `Instant`, `seconds: u64` → `seconds: i64`.
  `monotonic-clock.duration` now lives in `wasi:clocks/types.duration`.
- For Golem-owned WIT: converted `subscribe()+get()` patterns to p3-native
  `get: async func() -> T`; removed websocket `subscribe`.
- Vendored third-party WASI WIT (`wasi:keyvalue`, `wasi:blobstore`,
  `wasi:logging`, `wasi:config`): no upstream p3 versions exist.
  Patched locally — keyvalue/blobstore stream APIs rewritten from
  `wasi:io` resources to built-in `stream<u8>`; cache future resources
  converted to async `get`; blobstore `stream-object-names` resource
  removed in favor of `list-objects: stream<object-name>`.
- Bindgen caveat: wasmtime v45 p3 bindgen mishandles top-level WIT
  type aliases like `type incoming-value-async-body = stream<u8>` (emits
  invalid Rust identifiers with `-`). Workaround: inline `stream<u8>` /
  `list<u8>` at use sites instead of aliasing them. Do not revert.
- `wasm-tools component wit ./wit` requires `--all-features` because p3
  WIT includes unstable `timezone`. This is fine.
- `cargo make wit` works (Makefile no longer references deleted
  `wit/deps/io`).
- Whole workspace builds: `cargo build --workspace --all-targets` is
  green (warnings only). Build success ≠ runtime correctness.

## Accessor-based pattern (reference)

p3 async host methods are generated as:
```
async fn get<T>(accessor: &Accessor<T, Self>, self_: Resource<...>) -> ...
```
not `async fn(&mut self, ...)`. `Accessor<T, D>` is `Send + Sync`, holds a
`StoreToken` + projection fn, and gives short synchronous access to the
store via `accessor.with(|access| { ... })`. The `with` closure cannot
`.await` and cannot return borrows into store data. This is required so
multiple concurrent host futures in the same store don't deadlock each
other. See [concurrent.rs](file:///Users/vigoo/projects/golem/wasmtime/crates/wasmtime/src/runtime/component/concurrent.rs#L338-L470).

## Rust SDK migration (sdks/rust/golem-rust)

Status: SDK builds against p3 WIT (`cargo build -p golem-rust --all-features`).

Changes made:
- Cargo.toml:
  - `wasip2 = "1.0.2"` → `wasip3 = "0.6.0"`
  - `wstd = "=0.6.5"` removed (WASI 0.2 only, no p3 analogue)
  - `wit-bindgen = "=0.53.1"` → `wit-bindgen = "=0.57.1"` with
    `["async", "async-spawn"]` features (required by p3 async + spawn)
- src/lib.rs:
  - Dropped `pub use wasip2;` and `pub use wstd;`; added `pub use wasip3;`.
  - Removed all `"wasi:io/poll@0.2.3"` and `"wasi:clocks/wall-clock@0.2.3"`
    `with:` mappings from every `wit_bindgen::generate!` block (those
    interfaces no longer exist in p3 WIT).
  - Added `with:` mappings for `wasi:clocks/system-clock@0.3.0-rc-...` and
    `wasi:clocks/types@0.3.0-rc-...` to `wasip3::clocks::system_clock` /
    `wasip3::clocks::types` so the SDK shares the wasip3 types with hand-
    written `IntoValue`/`FromValueAndType` impls (otherwise wit-bindgen
    generates a duplicate `Instant` type).
  - `blocking_await_promise` now delegates to `wit_bindgen::block_on` of
    the async version. Async version uses `promise.get().await` directly
    (no more `subscribe()/poll`).
- src/json.rs: `blocking_await_promise_json` reuses
  `crate::blocking_await_promise`.
- src/agentic/async_utils.rs: stripped down — `await_invoke_result` is now
  just `invoke_result.get().await`; `await_pollable` removed (no pollables).
- src/agentic/agent_registry.rs:
  - `wstd::runtime::block_on` → `wit_bindgen::block_on`.
  - `wasip2::cli::environment::get_environment()` → `wasip3::cli::environment::...`.
  - `with_agent_instance_async` and `with_agent_initiator` got `Fut: 'static,
    R: 'static` bounds because `wit_bindgen::block_on` requires the future
    to be `'static`.
- src/value_and_type/wasi.rs:
  - `wasip2::clocks::wall_clock::Datetime` → `wasip3::clocks::system_clock::Instant`
    (re-aliased as `Datetime` for callers; the `seconds` field is now `s64`
    instead of `u64`, type builder updated accordingly).
  - Removed `IntoValue`/`FromValueAndType` impls for
    `wasip2::io::error::Error` (`wasi:io` is gone in p3 entirely).
- src/websocket.rs: async `receive()` / `receive_with_timeout()` are
  implemented (T27): the websocket WIT marks `receive` /
  `receive-with-timeout` as `async func`, so the generated bindings are
  natively async; `blocking_*` wrappers use `wit_bindgen::block_on`.

Still to do:
- Rebuild Rust test components (anything in `test-components/*/Cargo.toml`
  that depends on `golem-rust`) against the new SDK + p3 bindings.
- The transitive dep `wasip2` still appears in `cargo tree` because
  `golem-wasm` still depends on it for its own bindings. Out of scope for
  the SDK migration; may need a separate pass on `golem-wasm`.
- `tests/agent.rs` still calls `wstd::runtime::block_on` directly in two
  places — needs to be updated when we run the SDK test suite. Untouched
  for now to keep this commit focused on making the SDK lib compile.

## Structured concurrency under p3 (research note)

Confirmed: guest-side `race` / `select` / `join` still work under p3 with
`wit_bindgen::block_on`, and the loser of a `race` is actually cancelled —
not leaked.

Mechanism (wit-bindgen 0.57.1 `rt/async_support/subtask.rs`):
- Each async host import is a `WaitableOperation<SubtaskOps<...>>`.
- Dropping the corresponding Rust `Future` (which is exactly what
  `futures::select` / `futures_concurrency::Race` do to the losing branch)
  hits `in_progress_cancel`, which calls the canonical ABI builtin
  `[subtask-cancel]`.
- The runtime distinguishes `STATUS_STARTED_CANCELLED` and
  `STATUS_RETURNED_CANCELLED` and cleans up params/results via owned drop.

Caveats:
- wasmtime issue [#12766](https://github.com/bytecodealliance/wasmtime/issues/12766)
  (filed by alexcrichton, marked After-P3): on cancel the host async fn just
  has its Rust future dropped — there is no API to gracefully return a value
  on cancel. So whether a host op actually stops depends on whether its impl
  honours `Future::drop`.
- wit-bindgen issue [#1495](https://github.com/bytecodealliance/wit-bindgen/issues/1495):
  `cancel-import` test currently hangs against wasmtime main. So the wiring
  works but has sharp edges.

Consequence for our durability redesign (Blocker 1 addendum):
The concurrent-durability oplog model needs a third terminal state per
call_id: not just `End { result }` but also `Cancelled { partial? }`.
Per-call host impls must do all durable cleanup in their `Drop`
implementation — wasmtime won't give us a graceful "cancel with value" path.
Replay must reproduce both completions and cancellations in the recorded
order.

## wasm-rquickjs migration

The TS SDK depends on `wasm-rquickjs` (https://github.com/golemcloud/wasm-rquickjs)
to embed JS into WASM components. We own this repo, so the migration plan
treats it as a target of porting work, not as a third-party blocker.

### Surface inventory (as of current `main`)

| Area | Today | Notes for p3 port |
|---|---|---|
| Async runtime | `wstd = "=0.6.5"` (`block_on`, `AsyncPollable`) | Replace with `wit_bindgen::block_on` (same approach as Rust SDK port) |
| WASI bindings | `wasip2 = "1.0"` everywhere | Switch to `wasip3` |
| `wit-bindgen` | 0.42.1 / 0.51.0 / 0.53.1 mix | Bump all to 0.57.x with `async` feature |
| HTTP (fetch path) | `golem-wasi-http = "0.2.0"` | Port to `wasi:http` p3 (drop `golem-wasi-http` or fork it) |
| HTTP (`node:http` path) | Raw `wasip2::http::outgoing_handler` + manual `subscribe()` | Rewrite as direct `.await` on `wasi:http` p3 async APIs |
| Sockets (`net.rs` / `dgram.rs` / `dns.rs`) | `wasip2::sockets` + `subscribe()` | `wasi:sockets` p3 — note this API genuinely reshapes, not just async polish |
| Timers | `wstd::task::sleep` | `wasi:clocks/monotonic-clock` p3 async sleep |
| WebSocket | `golem-websocket` crate uses `wasi:io/poll@0.2.3` pollable | Drop pollable from WIT, regenerate p3-style |
| Env / argv | `wasip2::cli::environment` | `wasip3::cli::environment` |
| Codegen for user imports (`crates/wasm-rquickjs/src/imports.rs`) | Emits `AsyncPollable::new(x.subscribe()).wait_for().await` | Emit direct async `.await` on the host import |
| C deps (`rquickjs-sys`, `libsqlite3-sys`) | wasi-sdk targeting `wasm32-wasip2` | Same C code, just need wasi-sdk that produces `wasm32-wasip3` objects. SQLite WASI VFS (custom `golemcloud/rusqlite` fork) needs a p3 audit. |

No upstream p3 work exists in `golemcloud/wasm-rquickjs`. The `wasip3` entry
in its Cargo.lock is only a transitive `getrandom` dep, not intentional.

### The "wstd disappears" insight

Earlier I framed `wstd` as a hard blocker. That was wrong: under p3, async
host calls are direct `async fn`s, so the entire "subscribe + AsyncPollable"
pattern simply doesn't exist. We replace `wstd::runtime::block_on` with
`wit_bindgen::block_on` (already validated in the Rust SDK port) and every
`AsyncPollable::new(x.subscribe()).wait_for().await` becomes `x.foo().await`
on the p3 import.

So `wasm-rquickjs` is "do the SDK-style refresh + rewrite each builtin to
p3-native + port the codegen in `imports.rs`", not "wait for someone to
ship a p3 wstd".

## Wizer + p3 components — experimental finding

ComponentizeJS-style flows (which wasm-rquickjs uses) pre-initialize the
JS bundle into the QuickJS heap at build time via Wizer. We need to know
whether Wizer can produce/consume p3-style components.

### Setup

- Wasmtime CLI 44.0.1 (which now ships the `wasmtime wizer` subcommand —
  the standalone `bytecodealliance/wizer` repo is now a thin shim that
  re-exports `wasmtime_wizer::*`; development moved into the wasmtime
  monorepo at `crates/wizer`).
- `wasm-tools` 1.248.0 (older versions can't even decode the
  component-type custom section produced by wit-bindgen 0.57's async
  generator — symptom: `invalid leading byte (0x43) for component
  defined type`).
- `wit-bindgen` 0.57.1.

### Two Rust crates built (kept under `tmp4/wizer-p3-experiment/`)

| Test | WIT export | Comment |
|---|---|---|
| A (sync) | `greet: func(name: string) -> string` | control |
| B (async) | `greet: async func(name: string) -> string` | true p3 async export — uses `task.return`, `waitable.join`, etc. |

Both also export `wizer-initialize: func()` at the world level (Wizer
needs `wizer-initialize` as a component-level export, not as an inner
core-module export).

Both built with `cargo build --target wasm32-wasip1 --release` then
`wasm-tools component new ... --adapt wasi_snapshot_preview1.reactor.wasm`
(adapter taken from wasmtime v44.0.0 release).

### Result matrix

| Test | Wizer command | Outcome |
|---|---|---|
| A (sync) | `wasmtime wizer -S cli --keep-init-func=true ...` | ✅ wizered, validates clean |
| A (sync) | default `--keep-init-func=false` | ⚠️ wizered but produces invalid component — hits wasmtime issue [#13168](https://github.com/bytecodealliance/wasmtime/issues/13168) (`core instance has no export named wizer-initialize`) |
| **B (async)** | `wasmtime wizer -S cli -W component-model-async --keep-init-func=true ...` | **✅ wizered, validates clean, async WIT export preserved** |
| B (async) | same but no `-W component-model-async` | ❌ `waitable.join requires the component model async feature` |

### Conclusions

1. **Wizer is NOT a hard blocker for p3 components.** Earlier claim retracted.
2. The init function itself runs synchronously (it's a sync `func()` in WIT),
   which is fine — JS heap setup doesn't need async I/O during build.
3. The component-model async ABI (`task.return`, `waitable.join`, etc.) is
   preserved end-to-end through the snapshot-and-rewrite pipeline.
4. Required configuration for our use case:
   - depend on `wasmtime-wizer 44+` from the wasmtime monorepo (not the
     v11.0.3 shim, which is still pinned to wasmtime 42)
   - pass `-W component-model-async` (and likely `-W component-model-async-builtins`,
     `-W component-model-async-stackful` defensively)
   - pass `--keep-init-func=true` until #13168 is fixed
5. Separately: wasm-tools must be 1.248.0+ (older versions cannot even read
   wit-bindgen-0.57-generated async component-type custom sections).

Reproducer artifacts kept at `tmp4/wizer-p3-experiment/` (Cargo crates,
componentized output, wizered output, and the WASI preview1 reactor
adapter).

## Resolved (T26): no p3 replacement for `lazy-initialized-pollable`

`golem:durability/durability@1.5.0` used to expose a host resource
`lazy-initialized-pollable` (constructor / `set(pollable)` / `subscribe()`)
that let a guest hand out a "wakeup token" during durable replay, then
later attach it to a real pollable when replay transitioned to live mode.

**Decision: the resource is removed with no replacement.** What it provided
was a *level-triggered, reusable, rebindable readiness handle* — a concept
that only existed because p2 separated readiness (pollables, host-mintable
only) from data. p3 `future`/`stream` handles are one-shot, linear, and
guest-creatable, so:

- no future-based design (host- or guest-side) could reproduce the
  ready→pending→ready rebinding contract anyway;
- none of the audited consumers actually needs it (see below): the pollable
  never crossed any caller-facing WIT boundary, it only implemented internal
  `blocking-get-next` loops;
- the replay→live transition is expressed in p3 by resolving replayed
  results immediately and directly awaiting the live source afterwards
  (exactly what golem-ai's Bedrock `nopoll` build already does on p2).

The commented-out resource and TODOs were removed from the WIT and the host;
no SDK primitive was added (a `LazyInitializedFuture` would promise
rebindable semantics the ABI cannot deliver). The `lazy_pollable`
worker-executor test and its test-component code were deleted: it existed to
test the removed feature itself, and its remaining coverage axes are already
held by `durability::custom_durability_1` (custom durability + PersistNothing
+ restart/replay) and `wasi::oplog_replay_after_streaming_http_read`
(streaming chunked HTTP + restart + full replay).

### Where it is actually used (audit)

- **Local repo**: zero callers in code. Only WIT defs and auto-generated
  FFI bindings reference it.
- **`golemcloud/wasm-rquickjs`**: zero active usage. Only appears in a
  vendored WIT in a compile-test example and a generated `.d.ts` golden
  file fixture.
- **`golemcloud/golem-ai`**: **actively used** in three production crates,
  all gated on `#[cfg(feature = "durability")]`:
  - `llm/llm/src/durability.rs` — `DurableChatStream` (OpenAI, Anthropic,
    Grok, Ollama, OpenRouter; Bedrock opts out via `nopoll`)
  - `search/search/src/durability.rs` — `DurableSearchStream`
  - `tts/tts/src/durability.rs` — `DurableVoiceConversionStream`

The pattern in each: during replay, `subscribe()` returns a pollable
backed by `LazyInitializedPollable::new()`. When replay completes and a
real provider stream is created, every accumulated lazy pollable is
`set()` to the real stream's pollable so any caller blocked on it
unblocks.

A deeper audit of golem-ai (done as part of resolving T26) narrowed this
further:

- The lazy pollable is **never exposed** through the caller-facing WIT of
  `chat-stream`/`search-stream` (`get-next`/`blocking-get-next` only, no
  `subscribe`, no pollable return values). It exists solely so the internal
  `blocking_get_next` loop has something to `block()` on while the wrapper
  is still in replay state.
- The deferred attachment happens exactly once, at the single replay→live
  transition, when the continuation provider stream is created.
- The TTS `DurableVoiceConversionStream` never actually wires it up
  (`#[allow(dead_code)]`, no constructor/subscribe/set) — dead code.
- Bedrock's `nopoll` feature compiles out all pollable machinery and the
  durable replay/continuation logic works by directly awaiting
  `poll_next()` in a loop — proving the durability state machine does not
  need a readiness bridge, only the p2 blocking adapters did.

## Open task: migrate `golemcloud/golem-ai` to p3

Independent follow-on work, can only start once:
- `golem-rust` SDK p3 bindings are released (already done in `sdks/rust/`).

There is no lazy-initialized-pollable replacement to wait for (see the T26
decision above): the migration removes the readiness bridge instead of
porting it. Required changes per provider crate:
- Bump `golem-rust` to the p3-aware version.
- Delete the `LazyInitializedPollable` vectors, the cached `subscription`
  pollables, and the internal `subscribe()` bridge from the durable stream
  state machines; make `get_next` await the provider directly (replayed
  results resolve immediately; at the replay→live transition create the
  continuation stream once and await it in place — the shape the Bedrock
  `nopoll` build already has).
- Underlying provider reads move from p2 pollable-backed body streams to
  p3 `stream<u8>` mapped in-guest to event batches (guest-created
  `stream`/`future` handles via `wit_stream`/`wit_future`; no host support
  needed). Streams and their mapping tasks must be fully consumed within a
  single invocation.
- Re-test each provider (openai, anthropic, grok, ollama, openrouter,
  search providers, tts providers, etc.).
- The `nopoll` feature flag becomes meaningless once every provider is on
  the direct-await path — remove it.
- The TTS `DurableVoiceConversionStream` state machine is dead code today;
  either finish it p3-style or delete it during the migration.

This is a separate repo, separate release cadence, but should be
tracked alongside the main migration so the SDK→library→provider
chain is consistent.

## Wasmtime fork customizations — p3 readiness audit

Our wasmtime fork (`golem-wasmtime-v45.0.0` branch at
`/Users/vigoo/projects/golem/wasmtime`) carries five Golem-specific
commits on top of upstream v45:

| Commit | Subject |
|---|---|
| `bba239940c` | Apply Golem fork wasi-io + wasi foundation to v45 baseline |
| `e3cf0300e5` | Apply Golem fork wasi-http customizations to v45 baseline |
| `10c4ba5d9b` | Port additional Golem fork changes for v45 compatibility |
| `ab5e4b9a49` | Port Golem fork CLI run/serve commands to v45 wasi shape |
| `106dc8ea89` | Yield fix |

All of these were ported from the v42 fork and are **p2-only** today —
none of the additions live under any `p3/` directory. Below is what
each addition does and what it means for the p3 cutover.

### 1. Suspend-on-long-sleep mechanism (wasi-io / wasi p2)

**Files:** `crates/wasi-io/src/{lib,impls,poll}.rs`,
`crates/wasi/src/ctx.rs`, `crates/wasi/src/p2/host/clocks.rs`.

**Surface:**
- `IoCtx { suspend_signal: Box<dyn Fn(Duration) -> wasmtime::Error + Send + Sync> }`
  — a host-configurable closure that returns a special trap.
- `Pollable { supports_suspend: Option<Instant> }` — pollables can
  declare a deadline at which they will become ready.
- `WasiCtxBuilder::set_suspend(...)` — host registers the closure.
- In `wasi:io/poll::poll()` (p2 impl), if every pollable in the wait
  set advertises a `supports_suspend` deadline, the implementation
  computes the longest sleep and calls `suspend_signal(duration)`,
  returning the resulting trap.
- `subscribe-duration` in `wasi:clocks/monotonic-clock` populates
  `supports_suspend` with the wakeup instant.

**Why we have it:** durability. A worker that would block on a long
`sleep`, a webhook wait, or a scheduled send should not stay resident
in memory. The trap unwinds the wasm stack so the host can evict the
worker and re-instantiate it from the oplog when the deadline elapses
or the external event arrives.

**p3 picture:**
- `wasi:io/poll` is gone. There is no central "wait set" for the
  host to inspect; instead each async host import is a separate
  `async fn` returning a future.
- The natural p3 hook is **at the host import boundary**: the
  `monotonic-clock.subscribe-duration` (or its p3 equivalent
  `wait-until` / `wait-for` async fn) is a host-implemented `async fn`,
  and the host can choose to *not* drive that future to completion,
  instead unwinding the calling task.
- For a `poll()`-style scenario where the guest is awaiting *several*
  futures together (`select!`/race), we lose the "central poll set
  inspector" perspective. Instead, the host needs a notion of "if all
  currently-pending host futures for this task are long-deadline
  things, suspend the whole task". That requires either:
  - tracking per-task pending host futures and their deadlines, or
  - moving the suspension contract one level up — into the wasmtime
    canon-async ABI itself, so the host can request "yield this
    task back with a deadline" — a proper upstream wasmtime
    extension, not a fork-only hack.

**Direction:** medium-large piece of work. Need to design a
"task-suspension" extension on top of canon-async, possibly upstream
to wasmtime, otherwise re-implement as a fork addition layered on the
component-async runtime.

### 2. HTTP connection pool (wasi-http p2)

**Files:** `crates/wasi-http/src/p2/connection_pool.rs` (~680 lines,
new module), `crates/wasi-http/src/p2/{body,http_impl,types,types_impl,mod}.rs`,
`crates/wasi-http/src/{lib,ctx}.rs`.

**Surface:**
- `HttpConnectionPool` + `HttpConnectionPoolConfig`.
- `WasiHttpCtx::connection_pool` and `WasiHttpHooks::connection_pool()`
  override hook.
- `pooled_send_request_handler` / `default_send_request_with_pool`.
- `HostIncomingResponse::pooled_connection` field +
  `poison_pooled_connection()`.
- `ConnectionPermits` type.
- `HostIncomingBody::retain_connection_permits` / lifecycle hooks
  that release the permit only when the response body is fully drained
  or aborted.

**Why we have it:** TCP + TLS handshake cost is huge per outgoing
request. With many short-lived workers each making external HTTP
calls, sharing keep-alive connections across workers is a major
performance win.

**p3 picture:**
- `wasi-http` p3 is built on async `request: handler(request) -> result<response, error-code>`
  (no manual outgoing-handler shim, no `future-incoming-response` resource).
- The pool itself is a **host-side concern** — independent of WIT —
  so the *implementation* can in principle be carried over wholesale.
  The integration points change:
  - The fork hooks `WasiHttpHooks::send_request` to consult the pool;
    p3 has a different host trait shape (probably `WasiHttp::handler`
    with an `Accessor`-style host method).
  - The body lifecycle hooks (`retain_connection_permits`,
    `poison_pooled_connection`) need to attach to whatever resource
    represents the response body in p3 (likely a `stream<u8>`
    completion, plus a host-side ownership token).
- Hyper / hyper-util sides don't change.

**Direction:** medium. Mostly a port-and-rewire of the existing module
into the p3 host impl shape. The pool data structures and TLS handling
stay; only the lift/lower glue and lifecycle hooks change.

### 3. Deferred send during replay (wasi-http p2)

**Files:** `crates/wasi-http/src/p2/types.rs`,
`crates/wasi-http/src/p2/http_impl.rs`.

**Surface:**
- `HostFutureIncomingResponse::Deferred` variant — stores the live
  `hyper::Request` and triggers the send on first `get()` instead of
  immediately on `handle()`.
- The pollable for `Deferred` reports "ready" so the durable `get()`
  is reached during replay and serializes the response from the oplog.

**Why we have it:** during replay, the guest calls `outgoing-handler.handle`
and expects a `future-incoming-response`. We don't want to actually
send the HTTP request during replay (the oplog has the canned response).
The deferred variant lets `handle()` return a "future" that does
nothing until `get()`, which then either replays from oplog (replay
mode) or actually fires the request (live mode).

**p3 picture:**
- `wasi-http` p3 collapses `outgoing-handler.handle` + `future-incoming-response.get`
  into a single async `request(...)` call. There is no equivalent of
  the "two-phase handle/get split" to defer between.
- The deferral story therefore moves entirely inside our durability
  wrapper around the single async call: in replay mode the wrapped
  async fn yields the oplog response immediately; in live mode it
  actually invokes the host. No new wasmtime fork support needed.
- This is conceptually simpler under p3, modulo the concurrent-durability
  redesign (Blocker 1) which is the precondition for any durable async
  host fn.

**Direction:** small. This customization mostly *disappears* under
p3 because the p3 API doesn't have the two-phase split that motivated
it.

### 4. Body completion signalling + worker error propagation (wasi-http p2)

**Files:** `crates/wasi-http/src/p2/body.rs`,
`crates/wasi-http/src/p2/types.rs`.

**Surface:**
- `HostOutgoingBody::{completion_sender, set_completion_sender}` —
  signal when the outgoing body is finished or aborted.
- `HostOutgoingRequest::body_completion` — receiver counterpart.
- `BodyWithTimeout`/`record_frame` send `Trailers(None)` on EOF so
  `HostFutureTrailers` resolves correctly.
- `ConnWorkerErrorReceiver` / `retain_worker(worker, error_receiver)` —
  the body holds a reference to the underlying connection worker
  task, surfaces its errors to the guest at end-of-stream, and
  releases it when the body is consumed.
- `FailingStream` — a stream that always returns `LastOperationFailed`,
  used during replay to construct response bodies whose chunks come
  from the oplog.

**Why we have it:** correct lifecycle of long-lived hyper connection
worker tasks, error propagation from the network layer to the guest,
and replay-time response body reconstruction.

**p3 picture:**
- Outgoing body in p3 is a `stream<u8>` (or `tuple<stream<u8>, ...>`)
  passed into the request future. The host knows when it ends because
  the stream closes; no separate "completion signal" needed for that
  signaling. The connection-worker lifetime tracking and error
  propagation still need explicit code, since dropping a `stream<u8>`
  is the abort signal.
- `FailingStream` becomes "construct a `stream<u8>` whose host side
  immediately yields the oplog chunks then closes" — a normal p3
  stream impl.
- All of this collapses into the durable wrapper: no `Deferred`
  variant, no `set_completion_sender` plumbing, just an async fn that
  knows how to construct or replay the response stream.

**Direction:** small-medium. Conceptual collapse into the durability
layer, but the connection-worker error propagation logic still needs
to be carried over and wired to the p3 stream lifecycle.

### 5. DynamicPollable / `dynamic_subscribe` (wasi-io p2)

**Files:** `crates/wasi-io/src/{lib,impls,poll}.rs`.

**Surface:**
- `DynamicPollable` trait — host objects implementing this can be
  exposed as a `wasi:io/poll::pollable` whose readiness is dynamically
  computed by the host.
- `dynamic_subscribe()` helper.

**Why we have it:** lets host-side resources synthesize pollables on
demand without going through the fixed wasi-io plumbing — used by
`lazy-initialized-pollable` and similar Golem-defined resources.

**p3 picture:** dies with `wasi:io/poll`. No replacement needed —
per the T26 decision, the lazy-initialized-pollable consumer pattern
is expressed with plain one-shot p3 `future`/`stream` handles, so no
dynamic-readiness host capability survives into p3.

### 6. Async bindings expansion (wasi p2 + wasi-http p2)

**Files:** `crates/wasi-http/src/p2/bindings.rs`,
`crates/wasi/src/p2/bindings.rs`.

**Surface:** the upstream `bindgen!` `async: { only_imports: [...] }`
list is extended to include extra functions that Golem wants to be
host-async (so we can `await` durably): `outgoing-handler.handle`,
`future-incoming-response.get`, `future-trailers.get`,
`incoming-body.{finish,drop}`, `incoming-response.drop`,
`future-incoming-response.drop`, plus async-ified clocks/env/fs/random/dns/io
host fns in `crates/wasi`.

**Why we have it:** under p2, host fns are sync-by-default and the
durability layer must yield from them. Adding them to the async
binding list lets them be `async fn` host impls.

**p3 picture:** *every* WIT export/import is naturally async-capable
under canon-async; the `only_imports` list goes away. So this
customization disappears entirely once we move to the p3 add_to_linker.

**Direction:** none — already gone in our p3 wiring (which uses
`wasmtime_wasi::p3::add_to_linker` and `wasmtime_wasi_http::p3::add_to_linker`).

### 7. Resource & view extensions (wasi)

**Files:** `crates/wasi/src/lib.rs`, `crates/wasi/src/view.rs`,
`crates/wasi/src/ctx.rs`,
`crates/wasmtime/src/runtime/component/resource_table.rs`.

**Surface:**
- `IoCtx`, `IoView::io_ctx/io_data`, `WasiCtxView { io_ctx, ... }`,
  `WasiCtxBuilder::build()` returns `(WasiCtx, IoCtx)`.
- `ResourceTable::get_any()` immutable accessor (added on the
  wasmtime side).
- `as_any()` exposed on `OutputStream` impls (wasi-tls, wasi-pipe,
  wasi-write_stream) so the durability layer can downcast streams
  to apply per-stream durability handling (see
  `is_incoming_http_body_stream` in our `durable_host`).
- File/Dir path tracking in `wasi/src/cli/file.rs` /
  `crates/wasi/src/filesystem.rs`.

**Why we have it:** the durability layer needs to inspect host
resources beyond what upstream exposes (downcasting, path tracking,
shared IoCtx).

**p3 picture:**
- `as_any()`-based downcasting of `OutputStream` impls is moot —
  outputs in p3 are component-model `stream<u8>` built-ins, not host
  trait objects. The downcast pattern needs to be replaced with
  whatever p3 host-side mechanism we use to recognize "this stream
  is a wasi-http body stream and needs durable wrapping".
- `IoCtx` itself is mostly tied to the suspend-signal mechanism
  (item 1), so it goes away unless we re-introduce a similar carrier
  for the p3 task-suspension machinery.
- `ResourceTable::get_any()` and file/dir path tracking are general
  utilities and should carry over unchanged.

**Direction:** medium. Mostly tied to whatever shape the p3
durability wrapping takes. Decide once Blocker 1 design is settled.

### 8. CLI run / serve patches (wasmtime CLI)

**Files:** `src/commands/run.rs`, `src/commands/serve.rs`.

These are minor wiring changes so the wasmtime CLI honors our
extended `WasiCtxBuilder` shape (3-arg subscribe, `(WasiCtx, IoCtx)`
build return). Pure plumbing, no semantic content. Drops out
entirely under p3 since the p3 add_to_linker has its own
non-customized wiring.

**Direction:** none — already not relevant to the p3 path.

### 9. Misc (`Yield fix`, hyper-rustls 0.27 bump)

The `Yield fix` (`106dc8ea89`) is a small wasi-io / clocks tweak
giving the right `Pending`/`Ready` behavior for zero-duration sleeps.
Needs review under p3 but is a one-liner-ish concern, not architectural.

The `hyper-rustls 0.27` bump is unrelated to p2/p3.

---

### Summary table — fork customizations under p3

| # | Customization | Effort to replicate under p3 | Notes |
|---|---|---|---|
| 1 | Suspend-on-long-sleep | **Large** | Needs new task-suspension contract on top of canon-async, possibly upstream |
| 2 | HTTP connection pool | Medium | Module logic carries over; lift/lower glue rewires |
| 3 | Deferred send during replay | Small / disappears | Collapses into durable wrapper around single async `request()` |
| 4 | Body completion + worker error propagation | Small-medium | Lifecycle ties to `stream<u8>` close, but worker-error path needs porting |
| 5 | DynamicPollable | None | Dies with `wasi:io/poll`; replaced by rebindable-future design |
| 6 | Async bindings expansion | None | Disappears — canon-async makes everything async-capable |
| 7 | Resource & view extensions | Medium | Mostly tied to durability-wrap shape; decide post-Blocker-1 |
| 8 | CLI run / serve patches | None | Not relevant on p3 path |
| 9 | Yield fix / dep bumps | Trivial | Needs review only |

The two architectural items are **(1) suspend-on-long-sleep** and
**(2) HTTP connection pool**. Both block production-quality p3 worker
execution and must be designed before we can claim p3 parity with the
current p2 fork.

## De-risking plan: suspend-on-long-sleep + suspend-on-promise-wait under p3

This expands fork-customization item (1) into an actionable plan,
scoped enough to decide whether to schedule the migration.
Earlier draft of this section was rewritten after an oracle review;
key corrections noted inline.

### Goal

A worker that has nothing to do *except* wait for time and/or
external events must be **evicted from memory** and **resumed later**
with the same observable state. Coverage:

1. Long sleeps (`monotonic-clock` waits beyond a configurable threshold).
2. **Golem promise waits** (a guest awaiting a host-side promise
   resolved by an external completion — webhook, scheduled invocation,
   inbox arrival, RPC reply from a different worker, etc.).
3. `select!` / `race` / `join!` combinations of (1), (2), and ordinary
   in-flight host calls.
4. **Concurrent durability**: the store may have other in-flight host
   futures that are *not* suspendable (real HTTP/RPC/DB). Suspension
   must wait for those to drain or model their abort+reissue.

### Why the p2 mechanism does not port directly

p2 relied on `wasi:io/poll::poll()` being a single host fn with full
visibility of the whole wait set. Under p3 there is no central wait
set; each async host import is its own future, and a guest task can
hold several pending futures simultaneously via `waitable.join`. So
the design moves from **trap-on-poll** to **per-future suspendability
metadata + scheduler-level quiescence detection + drop-store-and-
reinstantiate, with deterministic replay**.

### Existing wasmtime building block, and why it is *not* enough alone

Wasmtime v45 (PR #13246) added `Accessor::poll_no_interesting_tasks`
plus an `interesting_tasks` counter per store. It tracks *task
liveness*, not *task blocked-on-suspendable-waits*. A guest task
blocked on `waitable.join` is still "interesting", so this hook stays
pending exactly when we need it to fire.

**Conclusion:** `poll_no_interesting_tasks` answers "is the store
fully drained?", not "is the store quiescent on suspendable waits?".
We need a different hook.

### Primary path: small wasmtime-fork hook in `concurrent.rs`

We treat the **wasmtime-fork hook as the primary path**, not a
fallback. Reasoning:

- We already maintain a fork.
- The public v45 API does not expose what we need.
- A small targeted hook is simpler than building a fragile
  executor-side inference layer over the wrong primitive.

Concretely, in `crates/wasmtime/src/runtime/component/concurrent.rs`,
at the event-loop point where the runtime is about to return
`Poll::Pending` because there are no ready work items but there are
pending futures, expose:

```rust
pub struct SuspensionCandidate<'a> {
    pub pending_unsuspendable: usize,
    pub suspendable: &'a [SuspendableEntry],
}

pub struct SuspendableEntry {
    pub call_id: CallId,            // ties to Blocker 1 oplog model
    pub deadline: Option<Instant>,  // None = wait indefinitely
    pub wakeup_key: Option<WakeupKey>, // promise id, etc.
}
```

Each Golem-owned host async fn registers its future with optional
suspendability metadata via a new fork API (`Accessor::register_suspendable`
or similar). Untagged futures default to **unsuspendable**.

Wasmtime exposes a function the executor can call when it has no
ready guest work:
```rust
fn classify_suspension(&mut self, store: &mut Store<...>) -> SuspensionState
```
returning `Running` / `Blocked(SuspensionCandidate)` / `Idle`.

This is the only fork-side change. Maintenance cost: small, and aligned
with the upstream-anticipated direction in #13246's commit message.

### Mechanism

```diagram
                   ╭──────────────────────────╮
                   │  Worker scheduler        │
                   │  (golem-worker-executor) │
                   ╰────────────┬─────────────╯
                                │ run_until_quiescent_or_done()
                                ▼
                   ╭──────────────────────────╮
                   │  wasmtime store          │
                   │  ┌────────────────────╮  │
                   │  │ guest task         │  │
                   │  │  await join(       │  │
                   │  │    sleep(24h),     │←── future tagged Suspendable {
                   │  │                    │      deadline=now+24h, key=None }
                   │  │    promise.wait(p) │←── future tagged Suspendable {
                   │  │  )                 │      deadline=None, key=Some(p) }
                   │  ╰────────────────────╯  │
                   ╰──────────────────────────╯
                                │ classify_suspension()
                                ▼
       Idle: every interesting task is blocked AND
             pending_unsuspendable == 0
                                │
                                ▼
       persist SuspendedAt { suspension_id, candidates: [...] } to oplog
       drop store
       scheduler.register_resume(worker_id, deadlines, wakeup_keys)
                                │
       (deadline elapses or wakeup arrives)
                                │
                                ▼
       reinstantiate worker → replay oplog → reach same await point →
       on resume the actually-fired wake source produces a result;
       persist ResumedFrom { suspension_id, winner: call_id, partial? }
       so future replays pick the same winner deterministically
```

### Critical correctness points

These were under-specified in the earlier draft and are non-negotiable:

1. **Suspendable waits are real futures, not "Pending forever"
   sentinels.** A `select!(sleep(5s), http_call(30m))` where HTTP is
   unsuspendable must let the sleep fire locally at 5s and cancel the
   HTTP. A forever-pending sleep would deadlock correctness. The
   suspendability tag is metadata for the scheduler; the future
   itself behaves normally and resolves locally if its source ever
   becomes ready while the store is still resident.

2. **Persist the wake winner, not only wake candidates.** If both a
   promise and a deadline are ready by the time resume happens, the
   replay must pick the same winner the original execution did. Oplog
   records `ResumedFrom { suspension_id, winner: call_id, ... }`.
   `select!` losers must be recorded as `Cancelled` per the Blocker 1
   terminal-state model.

3. **Use absolute / logical durable time** in the oplog, never
   process-local `Instant`.

4. **No `Accessor` in `Drop`.** Wasmtime's own docs say `Accessor`
   is not guaranteed to work in `Drop`. Suspendable-future host impls
   must do their cleanup (unregister wakeup-key listeners, free
   resources) **without** going through `Accessor`. This means the
   cleanup state must be held in the future's own captured state, not
   reachable only via the store. This constraint applies to any
   cancellation under Blocker 1 too.

5. **Cancellation is first-class, not a footnote.** Drop = cancellation.
   Promise-wait, sleep, RPC-with-deadline, and any other suspendable
   primitive must each have a documented `Drop` contract.

6. **Default-deny suspendability.** Any host async import that does
   not explicitly carry suspendability metadata is treated as
   unsuspendable. A workspace-level lint / debug assertion checks
   that all our durable wrappers tag their futures explicitly.

7. **Post-return / background tasks.** A guest task that has called
   `task.return` may still hold pending host work. This counts as
   unsuspendable until those tasks finish. Confirm via prototype 0.

### Suspendable-future protocol

Each Golem-owned suspendable host async fn implements:

```rust
trait SuspendableHostFuture {
    fn metadata(&self) -> Suspendable;       // deadline?, wakeup_key?
    fn poll(&mut self, ...) -> Poll<Result>; // normal future drive
    // Drop impl handles unregistration without Accessor.
}
```

When created, the wrapper:
- registers the future in the store's `SuspendabilityTable`,
- registers any `wakeup_key` with the worker scheduler so external
  events can find the right worker to resume.

When dropped:
- unregisters from `SuspendabilityTable`,
- unregisters `wakeup_key` listener if applicable,
- writes a `Cancelled { call_id, partial? }` oplog record per Blocker 1.

### Concurrent-durability rule (v1)

> **Suspend only when `pending_unsuspendable == 0`.**

Trade-offs accepted for v1:
- A 30-minute HTTP request keeps the worker resident for 30 minutes.
- A hung unsuspendable call keeps it resident indefinitely.

Mandatory guardrails for v1:
- **Hard per-import timeouts** on all unsuspendable host calls
  (configurable; default sensible).
- **Metric**: `worker_resident_due_to_unsuspendable_seconds` per
  worker, alert threshold.
- **Metric**: count of evictions vs. count of forced-resident workers.

A v2 could add an "abort and re-issue on resume" contract per host
fn, where `Drop` cleanly aborts the in-flight work and the replay
re-issues. That is explicitly out of v1 scope.

### Validation experiments — corrected ordering

The previous draft's prototypes were good but ordered wrong. Run **0,
0.5, 0.75 first** — they decide the shape of everything else.

| # | Prototype | Pass criterion | Decides |
|---|---|---|---|
| 0 | Tiny guest blocked on a single host async import; check whether the wasmtime fork hook (or `poll_no_interesting_tasks` if we attempt Option A) ever surfaces "blocked on suspendable" | Hook fires correctly | Confirms primary path; disconfirms Option A |
| 0.5 | `select!(sleep(100ms), unsuspendable_http(30s))` | Sleep wins locally without eviction; HTTP is cancelled cleanly; oplog records loser as `Cancelled` | Catches the "Pending forever" bug; validates cancellation path |
| 0.75 | Promise wait registered externally, then store dropped before completion | No leak; no duplicate delivery on resume; listener cleanly unregistered without `Accessor` use in `Drop` | Validates the Drop contract |
| 1 | Single guest task `sleep(24h)` only | Store evicted; resume after fast-forwarded clock yields the right post-sleep state | Validates basic eviction + replay |
| 2 | Single guest task awaits a Golem promise | Store evicted; external completion event triggers resume | Validates wakeup-key path |
| 3 | `select!(sleep(24h), promise.wait())` — only one wakes | Either branch can fire on resume; oplog records winner | Validates `select!` over suspendables |
| 3.5 | Same as 3, but **both** wake sources become ready *while evicted* (the deadline elapsed AND the promise completed before resume runs) | Replay picks the original winner deterministically (i.e. the one we recorded), not whichever the host happens to surface first | Validates wake-winner persistence |
| 4 | `join!(sleep(24h), HTTP fetch)` (HTTP non-suspendable) | Store does NOT suspend until HTTP resolves; then suspends for the remaining sleep | Validates the v1 quiescence rule |
| 5 | `select!(sleep(24h), HTTP fetch)` | Whichever finishes first wins; loser cancelled correctly; works with Blocker 1's `call_id` model | Validates race semantics under concurrent durability |
| 6 | Two concurrent durable HTTPs + a sleep | Both HTTPs durably complete, then suspend on the sleep | Validates multi-call durability |
| 7 | Two suspendable waits; resume at earliest deadline; immediately resuspend for the later one | No spurious work on the inner resume; second suspension is clean | Validates resuspension |
| 8 | Guest task that has `task.return`-ed but still has background host work pending | Store NOT classified as suspendable until background work finishes | Validates post-return task handling |

If 0, 0.5, 0.75 pass, the architecture is sound. If 0 falsifies the
fork hook design, redesign before continuing.

### Effort estimate (corrected)

| Item | Estimate |
|---|---|
| Spike to falsify hook (prototypes 0 / 0.5 / 0.75) | ~1 week |
| Fork hook in `concurrent.rs` (`SuspensionCandidate`, `classify_suspension`, `register_suspendable`) | 1–2 weeks |
| Production-quality executor-side mechanism (suspendability table, oplog records, scheduler wiring, replay) | 3–5 weeks |
| Suspendable wrappers for sleep + promise wait + scheduled-invocation wait + RPC-with-deadline | 1 week |
| Guardrails (hard timeouts on unsuspendable host calls, metrics, alerts) | 0.5 week |
| Cross-cutting: Drop contracts for every suspendable + cancellable host fn | 1 week (overlaps with Blocker 1) |
| **Total before Blocker 1 work is complete** | **~6–10 weeks** |
| **Plus Blocker 1 (concurrent durability)** | separate, prerequisite |
| **Plus externally-completable wait-source primitive** | promise waits only, ~1 week |

The original "~2 weeks" estimate was a spike-only number for a single
happy-path scenario, not a production-ready mechanism.

### Shared primitive scoping clarification

An earlier draft suggested unifying the suspend mechanism with a
`lazy-initialized-pollable` replacement under a single "rebindable
future" abstraction. That is obsolete on both counts: the T26 decision
removed the lazy-initialized-pollable with no replacement (see the
"Resolved (T26)" section), and one-shot p3 futures cannot express a
rebindable wait source anyway. What remains for the suspend mechanism:

- **Low-level primitive:** a *durable replayable oneshot* /
  externally-completable wait source, used by promise waits
  (suspend mechanism). It is host-internal — not a guest-facing WIT
  resource.
- **Higher-level concept:** `SuspendableWait` carries
  `Suspendable { deadline?, wakeup_key? }` metadata, participates in
  the scheduler's quiescence check, is created by Golem-owned host fns.

Long sleeps don't need either abstraction — they have a real timer
source; just the suspendability metadata.

### Risks / unknowns we have NOT closed

- **Wasmtime cancellation maturity (issues #12766, wit-bindgen #1495).**
  Dropping the store mid-flight relies on host-side `Drop` doing
  meaningful cleanup. Brittle today.
- **`waitable.join` internals.** The mechanism assumes a guest task
  blocked on `waitable.join` is reflected by the future-set state we
  inspect in `concurrent.rs`. Prototype 0 must confirm this.
- **Replay non-determinism beyond `select!` winner.** `FuturesUnordered`
  completion order, scheduler interleaving across wasmtime versions,
  multi-task background work — all can introduce hidden non-determinism
  the replay model needs to either suppress or persist explicitly.
- **Promise resource shape under p3.** Currently tied to p2 pollables.
  Re-modelling them as p3 async fns is itself a separate design item
  and a precondition for prototype 2.

### Decision criteria for scheduling the migration

Schedule the migration once **all** of these hold:

1. Prototype 0 confirms the fork hook surfaces blocked-on-suspendable
   correctly (or definitively falsifies a chosen alternative).
2. Prototype 0.5 demonstrates suspendable + unsuspendable mixed
   correctness without eviction.
3. Prototype 0.75 demonstrates `Drop`-based cleanup without `Accessor`.
4. Written design exists for:
   - the `SuspensionCandidate` / `classify_suspension` fork API,
   - Drop contracts for every suspendable & cancellable host fn,
   - the absolute-time oplog representation for deadlines,
   - the wake-winner persistence record and how replay consults it,
   - alignment with the Blocker 1 `call_id` / `Cancelled` terminal-state model.
5. Effort and ownership for the **3–5 week production mechanism** is
   allocated, on top of the Blocker 1 redesign.

Until those are done, the suspend story is the highest single risk
in the migration.

---

## Durability audit — p3 host function decisions

A per-function durability audit of every new WASI Preview 3 host wrapper
(`golem-worker-executor/src/durable_host/p3/*.rs`) and every Golem-owned
custom WIT function whose `subscribe()+get()` / pollable pattern was
converted to p3 `async func` / `stream<T>` / `future<T>`. The p2 wrappers
under `golem-worker-executor/src/durable_host/{clocks,cli,random,filesystem,
sockets,http,io,…}/` were inspected to decide whether each function was
durable (recorded into the oplog) and what `DurableFunctionType` it used
(`ReadLocal`, `ReadRemote`, `WriteRemote`, `WriteRemoteBatched`). The p3
wrappers were inspected for their current state: `delegates` (passes
through to the wasmtime default with no oplog), `durable` (already wired
through `run_read_access` / `CallHandle`), or `unimpl`
(`unimplemented!()`).

### A. p3 WASI wrappers — make durable (currently `delegates`)

- **`clocks`** (`durable_host/p3/clocks.rs`)
  - `system_clock::now`, `system_clock::get_resolution` (ReadLocal —
    wall-clock reads return host-observable values that must replay)
  - `monotonic_clock::now`, `monotonic_clock::get_resolution` (ReadLocal)
  - `monotonic_clock::wait_until` (ReadLocal, suspend-coupled — the
    long-sleep / eviction path)
  - `monotonic_clock::wait_for` — **already durable ✅** via
    `P3MonotonicClockWaitFor` / `run_read_access`
- **`random`** (`durable_host/p3/random.rs`) — all five functions
  (`random::get_random_bytes`, `random::get_random_u64`,
  `insecure::get_insecure_random_bytes`, `insecure::get_insecure_random_u64`,
  `insecure_seed::get_insecure_seed`), all `ReadLocal`
- **`cli`** (`durable_host/p3/cli.rs`)
  - `stdin::read_via_stream`, `stdout::write_via_stream`,
    `stderr::write_via_stream` (the p3 streams replace the p2
    `io/streams::{read,write}` durability; `ManagedStd{In,Out,Err}` must
    be restored at this boundary, not in a separate io module)
- **`filesystem`** (`durable_host/p3/filesystem.rs`)
  - `read_via_stream`, `write_via_stream`, `append_via_stream`,
    `read_directory` (stream contents must replay)
  - `stat`, `stat_at` (ReadLocal — the only durable fs ops in p2; p2
    explicitly overrides `status_change_timestamp = None`)
  - All other `DescriptorWithStore` methods (`advise`, `sync_data`,
    `sync`, `get_flags`, `get_type`, `set_size`, `set_times`,
    `set_times_at`, `create_directory_at`, `link_at`, `open_at`,
    `readlink_at`, `remove_directory_at`, `rename_at`, `symlink_at`,
    `unlink_file_at`, `is_same_object`, `metadata_hash`,
    `metadata_hash_at`): **no** — p2 deliberately does not durable fs
    mutations/lookups (local fs is ephemeral per worker instance)
- **`sockets`** (`durable_host/p3/sockets.rs`)
  - `HostTcpSocketWithStore::send` (takes `StreamReader<u8>`),
    `HostTcpSocketWithStore::receive` (returns `StreamReader<u8>`) —
    outgoing/incoming bytes must replay (p2 durable via `io/streams`)
  - `HostUdpSocketWithStore::send`, `HostUdpSocketWithStore::receive` —
    **newly durable (p2 skipped these)**; this is the right moment to
    durabilize UDP datagrams
  - `ip_name_lookup::resolve_addresses` — **already durable ✅** via
    `P3SocketsIpNameLookupResolveAddresses` / `run_read_access`
  - All sync `HostTcpSocket`/`HostUdpSocket` setup (`bind`, `create`,
    `connect`, `disconnect`, `get_*`, `set_*`, `drop`) and
    `HostTcpSocketWithStore::connect`/`listen`: **no** — matches p2;
    durability lives on the data streams
- **`http`** (`durable_host/p3/http.rs`)
  - `client::send` (WriteRemoteBatched — the big one; collapses the p2
    `outgoing_handler.handle` + `future-incoming-response.get` two-phase
    into a single async call; **durable**: `DurableHttpHooks` is restored on
    the p3 view and the wrapper records/replays the request + response
    status/headers from the oplog)
  - `HostResponseWithStore::consume_body` (returns `StreamReader<u8>` +
    trailers `FutureReader`) — **durable**: response body chunks and trailers
    replay from the oplog chunk-by-chunk
  - `HostFields`, `HostRequest`, `HostRequestOptions`, `HostResponse`
    getters/setters, `HostRequestWithStore::{new, consume_body, drop}`,
    `HostResponseWithStore::{new, drop}`, error converters: **no** —
    in-memory manipulation; the recorded response status/headers are
    replayed by `client::send`'s durability

### B. Golem-owned custom functions — implement (currently `unimplemented!()`)

- `wasm_rpc::HostFutureInvokeResultWithStore::get`
  (`durable_host/wasm_rpc/mod.rs`) — the async RPC completion result;
  was p2 Pollable-based `FutureInvokeResultEntry`
- `golem::HostGetPromiseResultWithStore::get`
  (`durable_host/golem/v1x.rs`) — the promise-completion payload; was p2
  Pollable-based `GetPromiseResultEntry`
- `wasi:keyvalue/cache` full future-resource interface
  (`durable_host/keyvalue/caching.rs`) — initiation
  (`get`/`exists`/`set`/`get_or_set`/`delete`) records a `Start`, the
  matching `HostFuture*ResultWithStore::get` records the `End` with the
  result, and `HostFuture*::drop` records a `Cancelled` terminal (per
  the Blocker 1 `call_id` model — not a plain oplog entry like p2).
  `HostVacancy::{vacancy_fill, drop}` participates in the durable
  get-or-set state machine.
- `keyvalue::incoming_value_consume_async`
  (`durable_host/keyvalue/types.rs`) — returns the value as `stream<u8>`;
  bytes must equal the replayed durable `get` result
- `blobstore::incoming_value_consume_async`
  (`durable_host/blobstore/types.rs`) — same as keyvalue
- `blobstore::container::list_objects`
  (`durable_host/blobstore/container.rs`) — `ReadRemote`; listed object
  names must replay (the rest of `blobstore::container` — `get_object`,
  `put_object`/`write_data`, `delete_object`, `delete_objects`,
  `has_object`, `object_info`, `clear`, `delete_container`,
  `rename_object` — is already durable in the p2 style and kept)

### C. Follow-up (p3 WIT rework first, then durability)

- `websocket::{receive, receive_with_timeout}`
  (`durable_host/websocket/client.rs`) — resolved by T27: the
  `golem:websocket` WIT now marks both as `async func` and the host
  implements them on the accessor-based (`HostWebsocketConnectionWithStore`)
  path, so a parked receive does not hold the store. The already-durable
  `connect` / `send` / `close` stay as-is. A `stream<message>` redesign
  remains a possible separate follow-up.

### No durability needed (matches p2 or pure plumbing)

- `cli::environment::{get_environment, get_arguments, get_initial_cwd}`,
  `cli::exit::{exit, exit_with_code}`, all `cli::terminal_*`
- `http` header / request / response / request-options getters and
  setters, `HostFields::*`, error-code converters
- `filesystem` mutations and lookups except `stat` / `stat_at`
- `sockets` sync Tcp/Udp setup, `tcp::{connect, listen}`, UDP datagram
  metadata
- `keyvalue::outgoing_value_write_body_async`,
  `blobstore::outgoing_value_write_body` — guest writes body bytes into
  the stream; bytes are captured by the consuming durable `set` / `put`
  call (matches p2, where the body buffer is owned by
  `OutgoingValueEntry` and recorded by the set)
- Plain resource `drop`s (except future-resource `drop`s, which need
  `Cancelled`)
- `types::convert_error_code` / `preopens::get_directories` /
  `HostDescriptor::drop` (resource release; path tracking to restore
  separately per fork-notes item 7)

### Cross-cutting decisions surfaced by the audit

1. **HTTP collapses the p2 two-phase into `client::send`.** p2 had
   `outgoing_handler.handle` (durable) + `future-incoming-response.get`
   (durable) + body streams (durable via `io/streams`). Under p3 the
   single `client::send` must record the request + response
   status/headers, and the response body stream returned by
   `consume_body` must replay its chunks from the oplog. **Implemented and
   runtime-verified (checklist item #8 is `done`):** `WasiHttpHooks` is
   restored as `DurableHttpHooks` on the p3 view and both `client::send` and
   `consume_body` record/replay through the oplog. Initially verified by code
   review + host-side/unit tests only (the `http3.md` Step 8 runtime tests 3–8
   were blocked on a missing wasip3 HTTP component/runtime harness); the
   harness gap was closed by T40 — `test-components/http-tests` was migrated
   to P3 `wasi:http`, and the full `http_tests`-tagged worker-executor suite
   passes against the durable P3 path (see T40/T42 status in
   `p3-gaps-tasks.md`).
2. **Future-resource interfaces (`keyvalue::cache`) need both a `Start`
   on initiation and an `End`/`Cancelled` on `.get()`/`drop`**, per the
   Blocker 1 `call_id` model — not just one durable call like p2.
3. **Stdio durability is now at the `read_via_stream`/`write_via_stream`
   boundary** (p3 has no separate `io/streams`), so `ManagedStd{In,Out,
   Err}` must be restored here, not in a separate io module.
4. **UDP durability is a deliberate upgrade over p2**, not a port: p2
   skipped it, but the audit adds `HostUdpSocketWithStore::{send,
   receive}` to the durable set.
5. **`websocket::receive` keeps its `func` (sync) WIT shape** today;
   rather than wrap it, the WIT should be moved to p3 `async func` /
   streams first, then durabilized as a follow-up step.
