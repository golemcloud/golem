# WASI P3 + `main` merge resolution plan

This document tracks the merge of `main` (`3380a0579`) into `wasi-p3`
(`e2057c2ba`) from merge base `ce6c49492`.

## Definition of done

- [x] No unmerged index entries remain.
- [x] No conflict markers remain in tracked source files.
- [x] Every incoming `main` commit is either cleanly retained or explicitly
      ported onto the P3 implementation.
- [x] P3 async WIT, mixed P2/P3 linking, concurrent durability, stream
      recording, cancellation, and replay invariants remain intact.
- [x] All generated/copied WIT and SDK artifacts are regenerated from the
      merged sources of truth.
- [x] All affected test components and the committed OTLP plugin are rebuilt.
- [ ] Formatting, clippy, WIT drift, builds, focused tests, and the affected
      full suites pass.
- [ ] The merge commit is completed and the post-commit WIT drift check passes.

## Progress summary

| Stage | Status | Notes |
|---|---|---|
| Conflict and incoming-commit inventory | done | 228 unresolved paths; 45 incoming non-merge commits |
| Historical/design research | done | Unblocked, local migration/task/design docs, and per-file three-way stages inspected |
| Canonical manifests and WIT | done | Canonical sources staged; workspace metadata and dir-mirror tests pass; synchronized copies will be regenerated after SDK sources |
| Oplog schemas and common models | done | Schemas, payloads, conversions, matching, rendering, status folding, and append-only P3 binary compatibility covered |
| Executor runtime integration | done | Replay/storage, durable hosts, RPC lifecycle, worker status and split tests integrated; focused executor compile/tests pass |
| CLI/debug/build tooling | done | Handwritten conflicts and workflows resolved; WIT dependency copies mirrored from canonical sources |
| SDK handwritten integration | done | Rust, TS, Scala, and MoonBit merged on native P3 async APIs |
| Generated artifacts and lockfiles | done | Regenerated from merged sources; no hand-spliced artifacts remain |
| Test components and plugin | done | Affected components and OTLP plugin rebuilt with the merged CLI |
| Formatting/build/focused tests | done | Root build and focused common/executor/SDK checks pass |
| Affected full suites and final audit | in progress | Root unit and worker-executor suites pass; integration/CLI rerun and final merge-state audit remain |

## Conflict inventory

Initial index state: 228 unresolved paths (`UU` 196, `DU` 27, `UD` 5).

| Area | Count | Resolution class |
|---|---:|---|
| `.github` | 5 | manual workflow union |
| root manifests/build | 3 | manual manifests; regenerate lockfile |
| `cli` | 5 | manual sources; regenerate mirrored WIT |
| `dev-tools` | 1 | preserve `main`'s tested dir-mirror entry point |
| `golem-api-grpc` | 2 | manual canonical protobuf schemas |
| `golem-common` | 6 | manual models/conversions; regenerate mirrored WIT |
| debugging/test framework | 2 | manual exhaustive rendering/test union |
| worker executor | 14 | manual semantic integration |
| integration tests | 1 | dependency union |
| plugin | 2 | regenerate lockfile and committed WASM |
| MoonBit SDK | 132 | semantic merge of 15 sources; regenerate the rest |
| Rust SDK | 9 | semantic merge of 5 sources; regenerate lock/WIT copies |
| Scala SDK | 7 | semantic merge of 2 sources; regenerate WIT/DTS |
| TypeScript SDK | 13 | semantic merge of 5 sources, keep 3 deletions, regenerate WIT/DTS |
| test components | 23 | semantic merge of 6 sources; regenerate 16 locks and plugin binary |
| root WIT | 3 | manual canonical schemas |

## Non-negotiable integration invariants

1. The executor continues to provide P2 and P3 WASI side by side. Rust std and
   QuickJS compatibility imports do not justify reverting Golem-owned P3 async
   APIs to pollables.
2. Durable calls remain identified by the oplog index of `Start`; `End` and
   `Cancelled` resolve by `start_index`. Do not introduce a separate call-id.
3. Replay resolver effects occur only after cursor consumption is committed.
   Prefetching and speculative reads must not publish terminals or side effects.
4. The ordered oplog/state actors remain the synchronization boundary. Do not
   restore host-call paths that hold async status/oplog locks across awaits.
5. `HostStreamFrame` and `CompletionDiscarded` remain durable replay semantics,
   not optional diagnostics.
6. P3 invocation/snapshot guest calls remain async and are driven to a settled
   component-task state.
7. Replay never eagerly reactivates RPC targets. `KnownFresh` is only for the
   first trusted ephemeral delivery; replay and retries use `MayExist`.
8. Opaque secrets/quota handles and the redesigned tools/schema APIs from
   `main` must not be replaced with the branch's older serializable carriers.
9. Root `wit/` and protobuf files are sources of truth. Per-crate `wit/deps`,
   DTS, MoonBit FFI/MBTI, lockfiles, and WASM binaries are regenerated.
10. Existing persisted enum/protobuf tags are append-only. Never reuse a tag
    for a different semantic variant.

## Phase 1 — Canonical workspace and WIT sources

- [x] Resolve `Cargo.toml` as a semantic union:
  - retain Wasmtime/WASI 46.0.1 and P3/component-model features;
  - retain the P3-compatible pinned `wasm-rquickjs` revision;
  - retain removal of `wac-graph` and `wit-bindgen-rt`;
  - add `main`'s `dev-tools` exclusion, `wat`, `desert-rust`, and `test-r`
    updates;
  - use Golem's `wit-bindgen` 0.58 fork while keeping the P3-required 0.58
    bindgen/runtime shape.
- [x] Resolve canonical `wit/deps/golem-1.x/golem-host.wit`:
  - retain P3 async promise `get` and async snapshot save/load;
  - add cards, installation, and current `main` host APIs.
- [x] Resolve canonical `wit/deps/golem-1.x/golem-oplog.wit`:
  - retain P3 clock imports and concurrent/P3 oplog variants;
  - add all card queue/install/failure/revocation/expiration variants.
- [x] Resolve canonical `wit/deps/golem-quota/types.wit` using `main`'s opaque
      `golem:core/types@2.0.0` quota token and operations; do not restore
      `quota-token-record` serialization.
- [x] Audit the auto-merged `wit/host.wit` for P3 worlds plus tools, secrets,
      quota, and cards.
- [x] Resolve `Makefile.toml` with `main`'s idempotent `dir-mirror` design and
      P3's final dependency set/component tasks.
- [x] Resolve `dev-tools/dir-mirror/src/main.rs` to the thin tested-library
      wrapper from `main`.
- [x] Run dev-tools tests before using `cargo make wit`.

Progress note: canonical manifests and WIT sources are resolved and staged.
`cargo metadata --no-deps` succeeds. All 11 dir-mirror tests, its formatting
check, and clippy pass. The first `cargo make wit` refreshed synchronized WIT
directories but downstream SDK entrypoints still require their handwritten
semantic merges before the generated copies are staged.

## Phase 2 — Oplog wire format and common models

- [x] Resolve public protobuf oneof tags:
  - preserve `main`: cards at 48–51;
  - assign P3-only `HostStreamFrame` and `CompletionDiscarded` 52 and 53;
  - add comments/reservations if an abandoned experimental tag requires it.
- [x] Resolve raw protobuf oneof tags:
  - preserve `main`: cards at 47–50;
  - assign P3-only host stream/discard variants 51 and 52.
- [x] Merge all corresponding protobuf messages and conversion directions.
- [x] Merge `golem-common` oplog payload registry as a union of:
  - P3 clocks/random/filesystem/sockets/keyvalue/HTTP/stream payloads;
  - cards, secret audit/reveal, and `main` filesystem stream payloads.
- [x] Merge matcher support for cards, opaque values, stream frames, and
      completion-discarded entries.
- [x] Merge public/raw protobuf conversions and preserve fallible P3 typed
      payload encoding plus secret/quota redaction/rejection.
- [x] Merge public oplog model/WIT conversion and CLI/debug renderers
      exhaustively over the final enum.
- [x] Ensure worker status folding treats stream/discard/card records with the
      intended no-op or state update semantics.
- [x] Add/retain binary/protobuf roundtrip and compatibility tests for every
      new variant and tag.

Progress note: the first full unit run found that `main`'s new card, secret,
and filesystem payload cases had been inserted before existing P3 cases in
`desert-rust` enums. The incoming-only cases are now appended after every P3
case in `HostRequest`, `HostResponse`, and `HostFunctionName`, preserving the
branch's persisted tags. A regression test pins late P3 response and host
function encodings.

## Phase 3 — Executor runtime semantic integration

### Replay and batch reading

- [x] Keep the P3 transactional cursor/concurrent resolver architecture in
      `replay_state.rs`.
- [x] Port `main`'s 1,024-entry replay buffer beneath committed consumption.
- [x] Preserve rollback push-back, sparse-read single-entry fallback,
      backward-target invalidation, skipped regions, and jump behavior.
- [x] Route card/snapshot/fork/log/resolver effects only through committed
      consumption.
- [x] Preserve incomplete `Start`, orphan terminal, abandoned-call drain,
      nested parent, stream frame, and completion-discard handling.
- [x] Port batch replay/archive transfer fencing and pointer-identity cleanup.

### Durable host and snapshot recovery

- [x] Keep accessor-based P3 host calls, ordered recorder, worker-state actor,
      settled guest calls, and pending-tail accounting.
- [x] Port `snapshotting_mode` behavior from #3679 across all durability APIs:
      snapshot loading restores state without recording duplicate oplog writes.
- [x] Port `CardEventBoundaryScan` incremental scanning and synchronization from
      #3701.
- [x] Merge cards, opaque secrets, tools, quota, and current schema-value APIs
      into the P3 host implementations.
- [x] Merge main's filesystem read/skip/check-write optimizations into the P3
      direct-stream implementation without restoring pollable assumptions.

### RPC

- [x] Retain the P3 `Active`/`Baked`/`Cancelled`/`Consumed` state machine,
      one-terminal ownership, cancellation token, delivery token, and
      `CompletionDiscarded` behavior.
- [x] Port `RpcTargetActivation::{Activated, ReplayPending}` and lazy activation
      from #3669; replay must not reactivate targets.
- [x] Port agent config, invocation freshness, classified failures, and
      resolver-aware schema conversion from `main`.
- [x] Preserve idempotency/span ordering and safe incomplete-call recovery.

### Ephemeral oplog and worker lifecycle

- [x] Keep the branch oplog actor and ordered append/upload APIs.
- [x] Add `main`'s `TransferFiber`/`MultiLayerOplogService` lifecycle and cleanup.
- [x] Integrate `KnownFresh`/`MayExist`, one-shot ephemeral identity, config,
      results, and cleanup onto the branch's `ArcSwap`/worker-state actor.
- [x] Port resolver-aware invocation decode, then retain expected-output schema
      validation and settled P3 guest calls.
- [x] Merge agent card interest/revocation/expiration state and status folding.

### Tests reorganized on main

- [x] Port branch additions from deleted `tests/agent_sdk_ts.rs` into `main`'s
      split `tests/agent_sdk_ts/` modules, then keep the old file deleted.
- [x] Port branch additions from deleted `tests/in_function_retry.rs` into
      `main`'s split `tests/in_function_retry/` modules, then keep the old file
      deleted.
- [x] Merge durability/snapshot/card tests.

Progress note: `cargo check -p golem-worker-executor --lib` and lib-test
compilation pass. The eight replay-activation/freshness RPC tests and the
public-oplog auto-injected-field conversion test pass. Broader executor suites
remain in the final verification phase.

Progress note: protobuf card tags occupy public 48–51/raw 47–50, with P3
`HostStreamFrame` and `CompletionDiscarded` appended at public 52–53/raw
51–52. `cargo check -p golem-common` passes and the three queued-card protobuf
invariant tests pass.

Progress note: the first full executor run exposed eight integration failures.
The HTTP idempotency assertion had assumed a fixed oplog position; it now
derives the UUID from the persisted physical `Start` index of the send, and the
focused replay test passes. Three P2-only retry tests depended on pollable
`subscribe`, trailer, and skip bookkeeping that does not exist in the native P3
stream implementation; they were removed rather than reintroducing P2 state,
because the merged P3 request/response streaming and replay suites cover the
same retry boundaries. The P3 WASI sleep test is focused-green; its one broad-
suite timeout was load-sensitive and is not being used to weaken suspension.

Progress note: failed asynchronous memory growth safely traps and restarts the
Wasm instance, but the normal immediate restart retained the old generation's
memory permits. Three 768 MiB workers under a 768 MiB pool could therefore
livelock while each waited for permits held by the others. Permit-failure
restarts now release and reacquire permits, and a worker-lifetime interrupt
state carries that internal restart across replacement `RunningWorker`
generations. Terminal external interrupts supersede the internal restart, are
claimed once before success can be published, and remain authoritative until
that generation stops. The baseline large-dynamic-memory test, delayed-recovery
interrupt test, explicit interrupt/resume test, and the new pressure/interrupt
regression pass; the regression also passed five consecutive repetitions.

Progress note: the large-initial-memory fixture was allocating another 512 MiB
on every `run`, although it models resident initial memory. Retaining that
allocation in agent state prevents its second invocation from growing toward
1 GiB under the 768 MiB eviction-test cap. The exact eviction and readonly
bypass regressions now pass, and the complete worker-executor suite reports 766
passed, 4 ignored, and 0 failed.

## Phase 4 — CLI, extraction, debugging, and workflows

- [x] Keep `cli/golem-cli/src/composition.rs` deleted with WAC removal.
- [x] Merge CLI worker rendering and app test module registration.
- [x] Merge component metadata extraction: `main`'s agent+tool extraction on
      P3 Wasmtime APIs, concurrent async imports, and mixed P2/P3 compatibility.
- [x] Merge debugging playback tests and #3697 timeout.
- [x] Merge debug rendering for all final oplog variants.
- [x] Merge `integration-tests/Cargo.toml`: add `chrono`/`url`, keep
      `wac-graph` removed.
- [x] Merge all workflows:
  - retain the exact P3-compatible `wasm-rquickjs` revision;
  - retain `main`'s dev-tools/check-wit, MoonBit, Java/SBT, timeout, Scala,
    and TS package/publish changes.

## Phase 5 — SDK handwritten sources

### TypeScript

- [x] Keep the redesigned fluent API and tools/secrets/quota behavior.
- [x] Keep `baseAgent.ts`, old `internal/clientGeneration.ts`, and obsolete
      decorator HTTP tests deleted; do not resurrect decorator-era exports.
- [x] Merge P3 async guest/snapshot/RPC behavior into the redesigned `index.ts`.
- [x] Keep the real strict UTF-8 decoder implementation.
- [x] Merge broad host externalization and P3 template generation/stale-WIT
      checks into rollup/template scripts.
- [x] Reconcile vitest aliases with the final generated P3 bindings only.

Progress note: the pinned `wasm-rquickjs` generator cannot lower nested P3
`stream`/`future` values inside imported or exported result records. The
canonical tool WIT remains unchanged. The TS SDK omits its optional tool guest
export and host import until that generator supports the canonical
`InvocationResult`; this is behaviorally equivalent to the previous TS SDK's
always-empty tool registry. Obsolete generated tool/stream declarations were
removed. DTS generation, template generation, build, lint, formatting, and the
full TS test suite pass.

Progress note: canonical guest discovery/definition exports are synchronous;
retaining `Promise` wrappers from the old SDK changed their component ABI.
TypeScript and Scala now export these calls synchronously while invocation and
snapshot operations remain native P3 async calls. Regenerating DTS, templates,
and the Scala base image before runtime tests was essential: running against a
previously built guest artifact can conceal this source/build-order mismatch.
The full SDK suites pass with the regenerated artifacts.

Progress note: P3 RPC future `get` resolves directly with the method value and
rejects with `RpcError`; it does not synchronously return the old tagged result
union. The fluent client and unit mocks now follow that contract, preserve
non-RPC exceptions, and translate RPC rejections to `RemoteCallError`. The TS
unit suite and executor `ts_abort_after_complete_is_noop` regression pass.

### Rust

- [x] Merge current tools/guest bridge/client generation, cards, secrets,
      schema, and opaque quota handles with P3 async generated calls.
- [x] Keep direct `get().await`; do not restore pollable `subscribe()` helpers.
- [x] Keep P3 bindgen async declarations, clock remaps, and stream support.
- [x] Merge SDK manifest features/dependencies for tools and P3.

### Scala

- [x] Merge tools guest bridge/registry into async P3 `Guest.scala` exports.
- [x] Merge metadata and richer RPC APIs while retaining direct P3 promise
      awaiting/cancellation and no pollable loop.

### MoonBit

- [x] Semantically merge the 15 handwritten package/runtime/WIT/example/tool
      sources identified in the conflict inventory.
- [x] Keep native async RPC/agent/snapshot APIs and add tools/secrets/opaque
      quota/current schema behavior.
- [x] Do not restore legacy `WitValue`, `DataValue`, `DataSchema`, or old
      camelCase generated packages.

Progress note: the MoonBit workspace now resolves the example against the
local SDK, generated async aliases consistently use `@asyncCore`, and the
generator normalizes trailing whitespace and one final newline so regeneration
is idempotent. SDK/tools checks and tests pass; the tool suite reports 253
tests, and the example checks/builds against the P3 SDK.

## Phase 6 — Regenerate copied/generated artifacts

- [x] Run `cargo make wit`; use it to resolve every per-crate `wit/deps` copy.
- [ ] Run `cargo make check-wit` and verify a second sync is clean/idempotent.
- [x] Regenerate root `Cargo.lock` after all root manifests compile.
- [x] Regenerate `sdks/rust/Cargo.lock`.
- [x] Regenerate MoonBit FFI/world/package/MBTI output from final WIT, then
      `moon info` and `moon fmt`.
- [x] Regenerate TS DTS, build the SDK, and rebuild the P3 agent template.
- [x] Regenerate Scala DTS and `agent_guest.wasm` from the final P3 WIT.
- [x] Regenerate all 16 conflicted test-component lockfiles through component
      builds, not textual merging.
- [x] Regenerate `plugins/otlp-exporter/Cargo.lock` and the committed
      `plugins/otlp-exporter.wasm`.
- [x] Confirm no generated conflict path remains and no stale generated file
      survived merely because Git auto-merged it.

Progress note: `cargo make wit` completed twice without producing further
working-tree changes. During the merge, `cargo make check-wit` necessarily
reports the staged mirror changes because its implementation requires those
paths to be clean relative to `HEAD`; rerun it immediately after the merge
commit to perform the intended drift check.

Progress note: `http-tests`, `host-api-tests`, and `oplog-processor` initially
resolved both registry and fork builds of `wit-bindgen` 0.58. Its async spawn
runtime uses process-global identity, so the duplicate package identity could
silently drop spawned futures even though both packages reported version
0.58.0. Each fixture now directly pins the required Golem fork revision, its
lockfile contains one 0.58 identity, and all three components were rebuilt.

Progress note: unrestricted lockfile regeneration had floated 217 packages
even though both merge parents agreed on their versions. The newer AWS graph
could not discover native macOS roots, causing all S3 integration variants to
fail unless an explicit certificate file was supplied. The common-parent
versions, including the complete AWS and coupled `wasm-bindgen` families, were
restored without reverting dependencies on which the parents differed. There
is now zero unexplained common-parent drift; offline locked metadata succeeds,
and the exact S3 test passes without `SSL_CERT_FILE` or any other certificate
override.

## Phase 7 — Test-component semantic sources

- [x] `agent-rpc` Rust: keep P3 future `.get().await` and wasi-fetch; add
      `main`'s current schema encoding/ephemeral result APIs.
- [x] `agent-sdk-rust/Cargo.toml`: merge `chrono`/`url` features with P3
      `wasi-fetch`; remove duplicate or unused `wstd` entries.
- [x] `host-api-tests/golem_host_api.rs`: keep T41 P3 HTTP/async behavior and
      add card/promise APIs from `main`.
- [x] Keep P3 HTTP in `golem_wasi_http.rs`, `invocation_context.rs`, and
      `quota_api.rs`; incorporate independent `main` APIs only.
- [x] `oplog-processor`: keep async P3 ABI and durable Start/End/Cancelled
      enrichment; port `main`'s invocation-name map/removal correctness fix.

## Phase 8 — Verification

### Structural and focused checks

- [x] `git diff --name-only --diff-filter=U` is empty.
- [x] Search tracked files for conflict markers.
- [x] `cargo metadata --no-deps` succeeds.
- [x] Dev-tools fmt/clippy/tests pass.
- [ ] `cargo make check-wit` passes.
- [x] Oplog protobuf/payload/matcher/public rendering tests pass.
- [x] Replay-state, ordered oplog, RPC, snapshot, status, and stream unit tests
      pass.
- [x] Dynamic-memory permit reacquisition, interrupt priority (one run plus five
      repetitions), delayed-recovery interrupt, and explicit resume tests pass.
- [x] Large initial-memory eviction and readonly permit-bypass tests pass with
      resident fixture memory reused across invocations.
- [x] `cargo make build` passes.
- [x] Rust SDK build/tests pass (151 tool tests).
- [x] TS SDK build/tests/template generation pass.
- [x] Scala compile/tests/base-image generation pass (547 tests).
- [x] MoonBit SDK/tools/example check/tests/build pass (253 tool tests).
- [x] All affected test components and OTLP plugin rebuild successfully.

### Runtime acceptance

- [x] P3 concurrent replay tests pass, including interleaved Start/End,
      cancellation, stream frames, completion discard, and restart.
- [x] P3 HTTP request/response streaming, retries, cancellation, and replay
      tests pass.
- [x] Snapshot recovery tests from #3679 pass under P3.
- [x] Batch replay/debug rewind/archive deletion tests from #3697 pass.
- [x] Ephemeral KnownFresh/MayExist and cleanup tests pass.
- [x] RPC replay does not reactivate targets and classified failures replay.
- [x] Card installation/revocation/expiration and opaque secrets/quota tests
      pass.
- [ ] Oplog processor P3 and OTLP smoke tests pass.
- [x] `cargo make unit-tests` passes.
- [x] `cargo make worker-executor-tests` passes (766 passed, 4 ignored).
- [ ] `cargo make integration-tests` passes.
- [ ] `cargo make cli-integration-tests` passes.
- [x] `cargo make fix` completes cleanly, followed by re-running checks affected
      by any automatic edits.

## Incoming commit audit

`[x]` means the commit is retained in the merged tree, including explicit
semantic integration or regeneration where Git could not merge it directly.

- [x] `12b3a2316` forked wit-bindgen (#3651)
- [x] `c02f2cf21` desert-rust update (#3656)
- [x] `53be60035` per-agent revoked card queue (#3632)
- [x] `65721968d` TS SDK optimization (#3660)
- [x] `469cac0f5` bounded component cache/concurrent compilations (#3643)
- [x] `3db667263` MoonBit bridge generator (#3659)
- [x] `2d0162829` Scala bridge (#3655)
- [x] `46f7b25f4` value type refactoring 5 fix (#3657)
- [x] `14cb3cc12` tests and fixes (#3661)
- [x] `2e5b9c5ff` retries and pool size (#3663)
- [x] `c38d5cdf6` CLI structured output (#3633)
- [x] `cf5592170` card installation host function (#3637)
- [x] `f7bfc476a` tools WIT (#3665)
- [x] `b2bc07525` ephemeral cleanup (#3652)
- [x] `2def062bb` value type refactoring 6 (#3662)
- [x] `f3d2e168d` CLI invocation oplog rendering (#3668)
- [x] `63e984324` deployment localServer/subdomain (#3667)
- [x] `b5dfe83ae` idempotent WIT syncing (#3670)
- [x] `54e4bb9c4` CLI manifest skill metadata (#3675)
- [x] `c217a1e4e` ephemeral listing fixes (#3671)
- [x] `1f08bf005` no RPC target reactivation during replay (#3669)
- [x] `3c71ae109` retry executor test cleanup (#3676)
- [x] `cc5c646ca` opaque secrets (#3674)
- [x] `a314cade5` Rust tool definition macros (#3673)
- [x] `1aa898c5e` permissions CLI/API improvements (#3646)
- [x] `5c0081ea5` integration DB dependency matrix (#3682)
- [x] `66c0e1694` Rust tool client generator (#3681)
- [x] `8cda94c1d` permissions cleanup (#3649)
- [x] `29b18e985` card expiration (#3672)
- [x] `012609a4d` Rust guest bridge generation (#3683)
- [x] `820759f3b` Scala tools support (#3687)
- [x] `f8e09af66` permissions card REST/CLI (#3688)
- [x] `7051b7b8e` RAG blog post (#3686)
- [x] `c40e8d824` Rust 1.97 clippy fixes (#3689)
- [x] `8fca30638` app version resolution (#3684)
- [x] `cef29698f` Scala tool bridge (#3693)
- [x] `26ba4d4d1` TS SDK redesign (#3680)
- [x] `e2c0fe298` snapshot recovery (#3679)
- [x] `96cffbc17` MoonBit tool definitions (#3698)
- [x] `dc63acbfe` performance regression fixes (#3701)
- [x] `a6dc3c3c7` ephemeral hot path (#3699)
- [x] `ed7cecfa7` MoonBit guest bridge (#3704)
- [x] `74e4c794b` batch oplog replay (#3697)
- [x] `bc50cf28c` SchemaValue API migration (#3710)
- [x] `3380a0579` website tracking tags (#3706)

## Progress log

| Date | Update |
|---|---|
| 2026-07-21 | Inventoried 228 conflicts and 45 incoming commits; inspected three-way stages and branch task/design docs; researched executor, SDK, WIT-sync, test-component, snapshot, replay, RPC, and ephemeral changes; wrote this plan before resolving conflicts. |
| 2026-07-21 | Resolved all test-component handwritten conflicts, regenerated all 16 standalone conflicted lockfiles, and passed focused wasm32-wasip2 checks for agent-rpc, agent-sdk-rust, host-api-tests, and oplog-processor. |
| 2026-07-21 | Resolved all 228 index conflicts; integrated P3 replay, stream, RPC, and actor invariants with cards, secrets, tools, schemas, batched replay, snapshots, and ephemeral changes from `main`; root build and focused common/executor tests pass. |
| 2026-07-21 | Regenerated WIT mirrors, SDK bindings/templates, test-component lockfiles/artifacts, and the OTLP plugin. Rust tool tests (151), Scala tests (547), MoonBit tool tests (253), and the full TS test/lint/build/template checks pass. |
| 2026-07-21 | Full unit testing exposed shifted persisted P3 `desert-rust` enum tags. Moved all incoming-only host payload/function cases to append-only positions and added regression fixtures; focused payload tests and the full root unit suite pass. |
| 2026-07-21 | `cargo make fix` completed successfully; retained its Rust 1.97 simplifications and removed the remaining unused executor-test import. |
| 2026-07-22 | Corrected synchronous TS/Scala canonical guest exports, fixed P3 RPC future rejection handling and mocks, rebuilt dependent artifacts, and passed the SDK suites plus the abort-after-completion runtime regression. |
| 2026-07-22 | Collapsed duplicate registry/fork `wit-bindgen` 0.58 identities in three async-spawn fixtures, rebuilt them, derived HTTP idempotency from the physical `Start`, and removed three obsolete P2 pollable retry tests superseded by P3 coverage. |
| 2026-07-22 | Diagnosed dynamic-memory permit livelock and integrated permit-reacquiring restarts with a worker-lifetime, terminal-priority interrupt state machine. Baseline allocation, delayed recovery, explicit resume, and the pressure/interrupt regression (one run plus five repeats) pass; compile, formatting, whitespace, and focused bug-finder review are clean. |
| 2026-07-22 | Corrected the large-initial-memory fixture to reuse its resident 512 MiB allocation, passed exact eviction/readonly checks, and completed the full worker-executor suite with 766 passed and 4 ignored. The first integration run passed 41/42 group-1 tests but exposed a P3 adaptation race in card revocation synchronization; a pre-await durable random boundary makes the exact test pass, and the full suite rerun is in progress. |
| 2026-07-22 | Traced the 75 S3 failures to accidental root lockfile drift, not host configuration: restoring every package version on which both parents agreed fixed native root discovery. Offline locked metadata and the exact S3 variants now pass without an explicit certificate file; the no-override full integration rerun remains. |
