# P3 gap-closing task list

Ordered task breakdown for closing **all** gaps in [`p3-gaps.md`](./p3-gaps.md)
(G1–G34). Execute in order; each task is individually implementable and
verifiable. Task descriptions reference the gap sections in `p3-gaps.md` for
full details and file/line evidence — read the referenced gap section before
starting a task.

## Conventions

- Build check: `cargo build -p golem-worker-executor` (or the crate touched).
- Worker-executor integration tests:
  `cargo-test-r run --package golem-worker-executor --test integration -- <name> --exact --report-time`
  (redis must be running; macOS has no `timeout`, use an external wrapper for
  hang-prone tests).
- When a task changes a test component, rebuild it first (see the
  `modifying-test-components` skill); `http-tests` flat artifact is
  `test-components/golem_it_http_tests_release.wasm`.
- Oplog payload changes: never reorder existing `BinaryCodec` enum
  constructors — append only. Roundtrip tests live in
  `golem-common/src/model/oplog/payload/tests.rs`.
- New P3 durable wrappers must follow the reviewed patterns in
  `durable_host/p3/` (`CallHandle` + correct drop policy, `run_read_access`,
  parent-`Cancellable`/child-`NotCancellable` for streamed results).
- After each task: `cargo make fix` before committing.

## Status tracker

Statuses: `todo` → `in-progress` → `done` / `blocked`.

| #   | Task | Gaps | Status |
|-----|------|------|--------|
| T01 | Drive the P3 request-body upload future | G1 | done |
| T02 | P3 send: HTTP call-limit + monthly quota accounting | G4 | done |
| T03 | P3 send: idempotency-key header injection | G3 | done |
| T04 | P3 send: trace-context headers + outgoing HTTP spans | G3 | done |
| T05 | `observe_function_call` in all P3 wrappers | G24 | done |
| T48 | Claim-safe span records for concurrent P3 sends | G35 | todo |
| T06 | Worker-level retry classification for P3 HTTP errors | G6, G17 | done |
| T07 | Generic retryable-error routing helper + P3 DNS retry | G17, G13 | done |
| T08 | Record/replay request-body transmission result | G8 | done |
| T09 | Rebuild in-flight P3 HTTP sends after restart | G2 | done |
| T10 | P3 HTTP cancellation semantics + tests | Part 2 §1 | done |
| T11 | Suspend-on-long-sleep for P3 `wait_until`/`wait_for` | G9 | done |
| T12 | P3 promise-await suspend parity | G9 | done |
| T13 | Design pass: P3 inline HTTP retry | G5 | done |
| T14 | Inline retry: awaiting-response phase | G5 | done |
| T15 | Inline retry: request-body write phase | G5 | done |
| T16 | Inline retry: resuming-response-body phase | G5 | done |
| T17 | Restore P2 worker stdout/stderr capture | G10 | done |
| T18 | Enriched environment for P3 `get_environment` | G11 | done |
| T19 | Filesystem parity holes | G12 | done |
| T20 | Keyvalue leftovers: atomic ops + async body | G16 | done |
| T21 | Signed time conversion hardening | G20 | done |
| T22 | PersistNothing on the live accessor path | G19 | done |
| T23 | Cancellation-drain tests + stale comment | G19 | done |
| T24 | Pre-migration oplog compatibility test + notes update | G18 | done |
| T25 | Un-ignore concurrent/suspendable durable-call tests | G19 | done |
| T26 | P3 replacement for `lazy-initialized-pollable` | G14 | done |
| T27 | Websocket WIT decision | G15 | done |
| T28 | Drain spawned guest tasks before invocation completion | G25 | done |
| T29 | Interruption/timeout for parked P3 host futures | G26 | done |
| T30 | Reject `stream`/`future`/`error-context` in agent schemas | G27 | done |
| T31 | Orphan-terminal handling in concurrent replay | G28 | done |
| T32 | Guards for jump/revert/fork cut points | G28 | done |
| T33 | Oplog processor: async ABI + P3-entry coverage | G29 | done |
| T34 | Debugging service concurrent-entry fixes | G30, G22 | done |
| T35 | Suspend heuristics aware of pending P3 work | G31 | done |
| T36 | Embedder follow-ups: OTLP smoke test + library-plugin policy | G32 | done ((b) obsolete) |
| T37 | Memory/backpressure hardening for P3 host tasks | G34 | done |
| T38 | Agent extraction: P3 `wasi:http` imports | G21 | done |
| T39 | Public oplog rendering tests for P3 entries | G23 | done |
| T40 | Runtime verification for P3 HTTP durability (checklist #8) | Part 2 | done |
| T41 | Migrate `host-api-tests` HTTP to P3 | G7 | done |
| T42 | Full-suite sweep + documentation close-out | all | done |
| T43 | Scala SDK: runtime port off pollables | G33 | done |
| T44 | Scala SDK: base image regeneration + integration tests | G33 | done |
| T45 | MoonBit SDK: bindings regeneration + API port | G33 | todo |
| T46 | MoonBit SDK: example build + verification | G33 | todo |
| T47 | TS SDK migration | G33 | blocked |

---

## Phase 0 — HTTP quick wins

### T01 — Drive the P3 request-body upload future (G1)

The P3 `WasiHttpHooks::send_request` in
`golem-worker-executor/src/durable_host/mod.rs` (~line 249) discards the
request-body I/O future (`_ = fut;`), so POST bodies are never uploaded. Drive
that future on the live path (spawn it alongside the returned response-body
I/O future, matching how live `WasiHttp::send` behaves), making sure the
replay path is unaffected.

**Verify:**
- `cargo build -p golem-worker-executor`
- These previously-hanging tests pass:
  `http::http_client`, `http::http_client_using_reqwest`,
  `http::http_client_using_reqwest_async`,
  `http::http_client_using_reqwest_async_parallel`,
  `wasi::http_client_response_persisted_between_invocations`.

### T02 — P3 send: HTTP call-limit + monthly quota accounting (G4)

Mirror P2 (`http/outgoing_http.rs:226-237`): call
`check_and_increment_http_call_count()` and `record_monthly_http_call()`
before the live P3 send in `durable_host/p3/http.rs` (live path only, never on
replay).

**Verify:** `resource_limits::http_call_limit_exceeded_traps_invocation`
passes; the passing `resource_limits::concurrent_agent_*` tests stay green.

### T03 — P3 send: idempotency-key header injection (G3)

Mirror P2 (`http/outgoing_http.rs:312-330`): when
`set_outgoing_http_idempotency_key` is enabled and the guest did not set the
header, inject `idempotency-key` derived via `derive_idempotency_key`
(`durable_host/mod.rs:387-395`) before serializing the request head, so the
injected header is part of the recorded durable request.

**Verify:** `http::outgoing_http_contains_idempotency_key` passes; add a
replay assertion (restart worker, confirm same key replays) if not covered.

### T04 — P3 send: trace-context headers + outgoing HTTP spans (G3, Part 2 §1)

Mirror P2 (`http/outgoing_http.rs:278-309`): start an invocation-context span
for the outgoing request, inject `traceparent`/`tracestate` (honoring
`forward_trace_context_headers`), and finish the span when the send completes
(both success and error paths; on replay, spans must be reconstructed the same
way P2 does). Injected headers must be recorded in the durable request head.

**Verify:**
- Build + existing http tests stay green.
- Add a worker-executor test (extend `http-tests` component/test server) that
  asserts the test server received a `traceparent` header from a P3 request.
- Full trace-propagation verification comes later with T41
  (`observability::invocation_context_test`).

**Note:** the shipped implementation records the span as positional
`StartSpan`/`FinishSpan` entries, which is unsound for *concurrent* sends —
tracked as T48 (G35), to be fixed before T09/T40's parallel-replay
acceptance runs.

### T05 — `observe_function_call` in all P3 wrappers (G24)

Every P2 wrapper calls `observe_function_call(interface, function)` for
metrics + debug tracing; no P3 wrapper does. Add calls to every host method in
`durable_host/p3/*.rs` (or centralize at the `run_read_access` /
`CallHandle::start_access` entry points plus explicit calls in pass-through
methods — pick one approach and apply it uniformly).

**Verify:** `cargo build -p golem-worker-executor`;
`rg -c "observe_function_call" golem-worker-executor/src/durable_host/p3/`
shows coverage in every file; spot-check `golem_host_function_call_total`
metric increments in one integration test run with logs.

### T48 — Claim-safe span records for concurrent P3 sends (G35)

The T04 implementation records the `outgoing-http-request` span as positional
`StartSpan`/`FinishSpan` entries, but the accessor-based send path runs host
calls concurrently and its [scope `Start`] → `StartSpan` → [host-call `Start`]
appends have await points between them — overlapping sends can interleave
these entries, and positional replay then delivers the wrong `StartSpan` to a
send (wrong span id) or fails with an unexpected-oplog-entry error. Same for
the `FinishSpan` written by the consume-body task and the deferred
`DropEvent::FinishSpan` drain. See G35 for the three candidate fixes (atomic
batch append; identity-keyed span entries resolved via the concurrent
resolver; spans derived from the claimed `Start` with no separate entries) —
do a short design pass, pick one, and implement it for the p3 HTTP send path.
Align the RPC `create_invocation_span` path if the chosen mechanism
generalizes (it is currently safe only because `&mut self` host calls hold
the store exclusively).

Must land **before** T09/T40 use the parallel streaming replay tests as
acceptance gates.

**Verify:** new regression test: two (or more) concurrent P3 sends in one
invocation, executor restart, full replay + a fresh invocation succeed
(extend `http::http_client_using_reqwest_async_parallel` with a restart or
add a dedicated test); `wasi::oplog_replay_after_parallel_streaming_http_reads`
and `..._raw_...` no longer blocked by span entries once their G2 fixes land;
existing `http::` tests stay green.

---

## Phase 1 — retry & replay semantics

### T06 — Worker-level retry classification for P3 HTTP errors (G6, G17)

Port `classify_http_error_code` semantics (P2: `http/types.rs:1555-1614`) to
the P3 send/consume-body paths: transient transport/protocol `ErrorCode`s must
route through the host-retry machinery (worker `Retrying` state per retry
policy) instead of being returned as guest-visible error values. Permanent
errors keep the current record-and-return behavior. Interaction with T14
(inline retry) is expected — keep the classification function shared.

**Verify:** `api::interrupt_worker_during_delayed_recovery_retry`,
`api::delete_worker_during_delayed_recovery_retry`,
`api::long_running_poll_loop_http_failures_are_retried` pass (workers reach
`Retrying`, not `Failed`).

**Note (shipped):** implemented for the **send** path via the shared
`classify_serializable_http_error_code` (P2's `classify_http_error_code`
delegates to it) and a new accessor-path
`CallHandle::try_trigger_retry_access` (concurrent.rs) that snapshots the
worker's retry-policy tiers into a `TaskRetryContext` and routes through
`try_trigger_host_trap_retry`. The **consume-body** terminal error is
deliberately *not* retry-routed yet: the send's `End` is already recorded, so
a retry would replay the response from recorded headers with an empty body
(the request is not re-issued) and re-execute the consume-body scope against
that empty body — silent wrong data. Wire consume-body retry classification in
**T09** together with the mid-flight send rebuild (see the comment in
`HttpConsumeBodyTask::run`).

### T07 — Generic retryable-error routing helper + P3 DNS retry (G17, G13)

Make the pattern from T06 reusable: a documented helper (or extension to
`run_read_access`) that lets any P3 wrapper classify an error value as
retryable-via-host vs guest-visible, so future wrappers cannot silently bypass
retry classification. Apply it to P3 `resolve_addresses`
(`p3/sockets.rs:1659-1691`) mirroring P2's transient resolver-failure retry
(`sockets/ip_name_lookup.rs:63-119`).

**Verify:** build; unit test for the helper; a DNS-failure worker-executor
test (add one if none exists) showing transient resolution failure triggers
retry instead of guest error.

**Note (shipped):** the T06 pattern is now the shared helper
`run_read_access_classified` (p3/mod.rs): it runs the standard
live/replay/incomplete flow and, before persisting, classifies a guest-visible
error value in the response payload (`classify: Fn(&Resp) ->
Option<ClassifiedHostError>`), routing transient failures through
`CallHandle::try_trigger_retry_access`. `run_read_access` delegates to it with
a `|_| None` classifier and documents that wrappers whose response carries an
error value must use the classified variant. Applied to P3 `resolve_addresses`
with `classify_p3_ip_name_lookup_error` (permanent: `AccessDenied`,
`InvalidArgument`, `NameUnresolvable`, `PermanentResolverFailure`; transient:
`TemporaryResolverFailure`, `Other`) and `RetryContext::dns` properties.
Covered by classification unit tests (`durable_host::p3::sockets::tests`) and
worker-executor tests `wasi::p3_ip_address_resolve` (durable success + replay,
via a new `resolve_p3` method on the host-api-tests Networking agent) and
`wasi::p3_ip_address_resolve_permanent_failure_is_guest_visible_without_retry`.
A *transient* resolution failure cannot be induced end-to-end: wasmtime's
resolver (P2 and P3 alike) maps every lookup failure to `NameUnresolvable`
("If/when we use `getaddrinfo` directly, map the error properly"), so the
transient branch is exercised only at the unit level — same limitation as P2's
retry, which is equally unreachable live today.

### T08 — Record/replay request-body transmission result (G8)

Wrap the request-body transmission `FutureReader` at
`HostRequestWithStore::new` (mirroring `HttpTrailersFutureProducer`) so the
transmission result is recorded on the live path and replayed deterministically
— a mid-body network failure must not replay as `Ok(())`.

**Verify:** payload roundtrip test in
`cargo test -p golem-common -- model::oplog::payload::tests::p3_http`;
host-side unit tests in
`cargo test -p golem-worker-executor --lib -- durable_host::p3::http::tests`;
regression test: record a transmission error, replay, assert the guest sees
the same error.

**Note (shipped):** the durable `request::new` interposes on the transmission
`FutureReader` (built-in future piped into a raw channel; the guest gets a
producer awaiting a resolution channel), with the wiring registered in
`pending_p3_http_request_transmissions` keyed by request rep and detached at
the *start* of the consuming host call (reps are reused after table deletion).
`client::send` spawns `HttpRequestBodyTransmissionTask` at a deterministic
point right after the send terminal (+`FinishSpan` on the error arm); the
recording is **demand-gated**: the task parks on a plain demand oneshot fired
by the guest's first real poll of the transmission future, and only then
appends/claims the `body-transmission` `Start` (`ReadRemote` +
`LeaveIncompleteOnDrop`, new payload pair
`P3HttpClientRequestBodyTransmission`) and records/replays the result. A guest
that never observes the result writes **no** oplog entries — required because
`run_concurrent` does not drain spawned tasks (G25/T28): an undemanded task
parked on replay-cursor machinery after the invocation's event loop exits
would strand the fair cursor mutex and deadlock the worker (observed and fixed
during this task). Guest-side `consume-body`/`drop` forward the deterministic
raw value with no recording. The `consume_replayed_request` drain result is
now only the fallback input for incomplete-`Start` re-execution. Residual
edge (guest demands, cancels the read, then immediately finishes the
invocation → task parked on the cursor past `run_concurrent` exit) is
documented on the task and deferred to T28. Regression tests:
`http::outgoing_http_request_body_transmission_error_is_recorded_and_replayed`
(new `post_with_short_body_transmission_error` method on the http-tests
`HttpClient4` agent: `content-length` mismatch → recorded
`HttpRequestBodySize` error replays identically) and the previously-passing
`http::outgoing_http_response_dropped_without_consuming_body_replays`
(fire-and-forget send, no transmission entries).

### T09 — Rebuild in-flight P3 HTTP sends after restart (G2)

Implement the P3 equivalent of P2's deferred-send + rebuild machinery
(`HostFutureIncomingResponse::deferred`, `rebuild_request_after_replay`,
`http/types.rs:1353-1444`): when replay reaches an incomplete P3 send
(recorded `Start`, no terminal), reconstruct and re-issue the request instead
of failing with "Non-idempotent remote write operation was not completed".
Use the request head + options already recorded in
`SerializableP3HttpClientSend` (this closes the per-request timeout/options
rebuild parity note in Part 2 §1). Respect idempotence rules: rebuild is
allowed for idempotent sends and for non-idempotent sends only under
`assume_idempotence` / recorded-safe conditions — match P2 semantics exactly.

**Verify:** `wasi::oplog_replay_after_streaming_http_read`,
`wasi::oplog_replay_after_raw_streaming_http_read`,
`wasi::oplog_replay_after_parallel_streaming_http_reads`,
`wasi::oplog_replay_after_parallel_raw_streaming_http_reads`,
`wasi::http_connection_pool_contention_with_restart`,
`wasi::http_client_interrupting_response_stream`,
`wasi::http_client_interrupting_response_stream_async` pass.
(`api::invocation_queue_is_persistent` and the `…then_sleep…` test also need
T11.)

Also wire the deferred **consume-body** worker-level retry classification from
T06 here: once a mid-flight send can be rebuilt/re-issued, a transient
body-transfer terminal in `HttpConsumeBodyTask::run` can safely route through
`CallHandle::try_trigger_retry_access` instead of being recorded and returned
(see the deferral comment at the parent finalize point in `p3/http.rs`).

**Note (shipped):** the incomplete-*send* rebuild already worked through the
generic scope machinery (incomplete `WriteRemote` re-executes via
`CallReplayOutcome::Incomplete`; incomplete `WriteRemoteBatched(None)` jumps
under `assume_idempotence`, else fails — P2-identical rules). What was missing
was the *consume-body* side: a replayed response carries an empty placeholder
body, so an incomplete consume-body scope jumped to live and silently
delivered a truncated body. Shipped fix: `open_p3_http_response_spans`
generalized to `open_p3_http_responses` (`OpenP3HttpResponseState`: span,
method/uri retry properties, and — for replayed responses — a
`P3HttpSendRebuild` with the recorded head + options, the re-derived
Golem-managed headers from the replayed span + send `Start` index (same
idempotency key on the wire, asserted by
`http_client_interrupting_response_stream`), and the recorded status for
divergence logging). `HttpConsumeBodyTask` re-issues the recorded request via
`reissue_recorded_request` on the **first live upstream read** — so a cleanly
replaying scope or a dropped stream never re-sends — using the built-in
`into_http_with_getter` conversion and the shared connection pool, with no new
oplog entries / call-count / span (recovery, not a new send). The fresh
response head is discarded; the recorded head stays authoritative. Rebuild is
**refused** (permanent, retry-exempt guest-visible `InternalError`) when the
recorded head declares a request body (`content-length` > 0 or unparseable,
any `transfer-encoding`) since P3 does not record request-body bytes; a
streamed upload without those headers is undetectable from the head — hard
no-body guarantees need a recorded body-present bit. This residual is
tracked in **T13** (design: record request-body state) and **T15** (extend
the rebuild to reconstruct recorded bodies / refuse unrecorded ones).
The deferred consume-body retry classification landed at the
**error-frame production point** (not the parent finalize point): before the
`End` child is persisted and before EOF reaches the guest, so a guest that
never reads trailers cannot race the retry trap and finish on truncated data;
on the trap the child is abandoned, the pending read gets a `Failed` reply,
and no `FinishSpan` is written. Verified: all listed tests pass **except**
the two parallel-streaming ones, which still fail on the pre-existing
concurrent `Start`-claim interleaving (T48 must land first, as noted above);
`api::invocation_queue_is_persistent` and
`oplog_replay_streaming_http_then_sleep_future_trailers_bug` pass already
without T11. New regression test:
`wasi::http_client_transient_mid_stream_failure_is_retried_and_reissued`
(server aborts the chunked body mid-stream once → worker retries, re-issues,
guest sees exactly the complete second body, server hit exactly twice); unit
tests `recorded_head_body_detection` and
`rebuilt_request_matches_recorded_head`. Known follow-up (applies to the T06
send path too): `enrich_retry_properties` sets `is-idempotent` from
`assume_idempotence` only — P3 HTTP retry properties are not method-aware the
way P2's `inline_retry.rs` override is.

### T10 — P3 HTTP cancellation semantics + tests (Part 2 §1)

Decide and document the intended behavior when a guest drops the P3 response
future or body `StreamReader` mid-flight (today: generic `CallHandle` drop
policies only; P2 had HTTP-specific cleanup). Implement any needed
HTTP-specific cleanup (aborting the in-flight hyper request, closing pooled
connections) and add tests: drop-before-response, drop-mid-body-stream, and
replay of both.

**Verify:** new worker-executor tests (extend `http-tests` component) pass;
no connection/socket leak (assert via pool state or test-server connection
count).

---

## Phase 2 — suspend

### T11 — Suspend-on-long-sleep for P3 `wait_until`/`wait_for` (G9)

Port the P2 suspend-on-long-sleep behavior (threshold hook
`durable_host/mod.rs:547-558`, suspend + scheduled wakeup
`io/poll.rs:237-274`) to `p3/clocks.rs:142-179`: a sleep beyond the threshold
must suspend the worker with a scheduled wakeup instead of parking the store.
Keep the existing `ReadLocal` durability of the wait. This is checklist item
#15 in `p3-durability-checklist.md` — flip its status when done.

**Verify:** existing P2 sleep/suspend tests stay green; add a P3 sleep test
(guest `wait_for` > threshold → worker suspends, wakes on schedule, replays
correctly); `wasi::oplog_replay_streaming_http_then_sleep_future_trailers_bug`
and `api::invocation_queue_is_persistent` pass (both also need T09).

### T12 — P3 promise-await suspend parity (G9)

P2 suspends the worker when `poll` waits only on promise-backed pollables
(`io/poll.rs:111-138`); the P3 `HostGetPromiseResultWithStore::get` awaits
in-process. Implement the equivalent: when a P3 guest is blocked solely on
promise `get`, suspend the worker and resume on promise completion.

**Verify:** existing promise tests stay green; add a test asserting a P3
worker awaiting an uncompleted promise transitions to suspended and resumes
when the promise is completed externally.

---

## Phase 3 — inline (in-function) HTTP retry port (G5)

### T13 — Design pass: P3 inline HTTP retry (G5)

Read `http/inline_retry.rs` (all phases: `AwaitingResponse`,
`WritingRequestBody`, `ResumingResponseBody`) and produce a short design doc
(`p3-inline-retry.md`) mapping each P2 mechanism to the P3 model: where the
hooks live (send wrapper, body-stream producer, consume-body task), how
recorded body chunks are reconstructed into a P3 `stream<u8>`, how
`InFunctionRetryState`/retry-point interacts with `CallHandle`, and how pooled
connections are poisoned. Deliverable is the doc plus skeleton hook points
(no behavior change).

The design must also decide how P3 records outgoing **request-body** state
(P2 recorded every body write; P3 records nothing), because that closes the
T09 rebuild gate: the shipped T09 restart-rebuild
(`reissue_recorded_request` in `p3/http.rs`) refuses to re-issue any send
whose recorded head declares a request body (`content-length` > 0 or
unparseable, any `transfer-encoding`), and a streamed upload *without* those
headers is undetectable from the head alone and is silently re-issued with an
empty body. At minimum the design must add a recorded body-present bit at the
send boundary (turning the undetectable case into a refusal); recording the
body chunks themselves (needed by T15's full-body resend anyway) also lets
the T09 rebuild re-issue body-bearing sends faithfully instead of refusing.

**Verify:** doc reviewed; `cargo build -p golem-worker-executor`; all
currently-passing tests stay green.

### T14 — Inline retry: awaiting-response phase (G5)

Implement on P3: status-code retry policies (opt-in `status-code` +
`is-idempotent` properties), transient-connection retry while awaiting the
response, idempotent-method rules (GET/HEAD/PUT/DELETE retry even with
idempotence off; POST fails permanently), zone-1 delay/trap thresholds, and
pooled-connection poisoning before resend. Reuse the T06 classification.

**Verify:** `in_function_retry::http_status_retry_policy_retries_matching_status`,
`http_zone1_inline_retry_on_transient_connection_failure`,
`http_zone1_falls_back_to_trap_when_delay_exceeds_threshold`,
`http_get_retried_inline_even_when_idempotence_disabled`,
`http_post_fails_permanently_when_idempotence_disabled` pass.

### T15 — Inline retry: request-body write phase (G5)

Implement on P3: retry on body-write failure with full-body resend from
recorded chunks, write-zeroes body reconstruction, trailers-present disables
retry, and the "no retry when subscribe used" equivalent for the P3 stream
model.

Once request-body recording exists (per the T13 design), also extend the T09
restart-rebuild (`reissue_recorded_request` in `p3/http.rs`): reconstruct the
recorded body instead of refusing body-bearing heads, and refuse (rather than
silently re-issue with an empty body) any send whose body was not recorded —
removing the header-only detection limitation documented in the T09 note.

**Verify:** `in_function_retry::http_output_stream_inline_retry_on_body_write_failure`,
`http_write_zeroes_body_reconstruction`,
`http_no_retry_when_trailers_present`,
`http_no_output_stream_retry_when_subscribe_used`,
`http_awaiting_response_retry_resends_full_body_after_output_stream_retry`
pass; a new restart-rebuild test for a body-bearing send (POST with body,
interrupt mid-response-stream, resume → faithful re-issue or clean refusal,
never an empty-body re-issue).

### T16 — Inline retry: resuming-response-body phase (G5)

Implement on P3: resume a failed response-body read via `Range` requests
(206/416/matching-full-response handling, prefix skip), and the
"no retry when body skip used" rule.

**Verify:** `in_function_retry::http_resuming_response_body_inline_retry_on_body_read_failure`,
`http_resuming_response_body_inline_retry_accepts_matching_non_partial_success_status`,
`http_no_resuming_response_body_retry_when_body_skip_used` pass. All 43
`http_tests`-tagged tests except G7-dependent ones should now pass — run the
full tag as a checkpoint.

---

## Phase 4 — stdio, env, small parity holes

### T17 — Restore P2 worker stdout/stderr capture (G10)

`ManagedStdOut`/`ManagedStdErr` (`durable_host/io/mod.rs:96-186`) pipe worker
output to the executor process's stdio. Route them through the same
worker-event/oplog emission as the P3 path (`p3/cli.rs:170-235`):
`InternalWorkerEvent::stdout/stderr` + log-behavior handling. Since Rust std
lowers `println!` to P2, this affects every component.

**Verify:** existing log/stdout worker-executor tests pass (search
`golem-worker-executor/tests` for stdout/log-event assertions); a `println!`
from a test component appears as a worker log event, not on executor stdout.

**Note (verified, no code change):** G10 was stale — the capture interception
was removed during the migration experiment (1e1829a57) but re-added in
"Readded wasi p2 host functions" (15882f76d). The current P2 path is
byte-identical in behavior to the pre-migration tree: `wasi:io/streams` links
through `DurableWorkerCtx`, whose `HostOutputStream::write` downcasts to
`ManagedStdOut`/`ManagedStdErr`, calls `emit_log_event`
(`InternalWorkerEvent::stdout/stderr` + oplog `Log` entry + `seen_log` dedup —
same semantics as the P3 `emit_log_event_access`) and returns without touching
executor stdio. `blocking-write-and-flush` decomposes into that `write`, and
the P1 `fd_write` path reaches it via wasmtime's P2 `get-stdout`. Verified by
passing `wasi::write_stdout` / `wasi::write_stderr` (which use
`println!`/`eprintln!`). Residual pre-existing (also pre-migration) holes, out
of parity scope: `write-zeroes` and `splice`/`blocking-splice` to console
streams bypass capture and leak to executor stdio, and console `splice`
wrongly goes through filesystem storage reservation — file separately if
worth fixing.

### T18 — Enriched environment for P3 `get_environment` (G11)

Port the P2 environment construction (`cli/environment.rs:25-55`: worker
metadata env + agent defaults + Golem config) to `p3/cli.rs:303-306`. Same for
`get_arguments`/`get_initial_cwd` if P2 diverges from pass-through.

**Verify:** existing env tests stay green; add a test using a P3-native env
read (`wasip3` env import, not std) asserting the enriched variables are
visible.

**Note (done):** Extracted the P2 enriched-env construction into
`DurableWorkerCtx::build_enriched_environment` (`durable_host/cli/environment.rs`)
and routed the P3 `environment::Host::get_environment` (`p3/cli.rs`) through
it, so both previews share the exact same code (worker metadata env + agent
default env merge + `GOLEM_*` enrichment with phantom-id handling).
`get_arguments`/`get_initial_cwd` are pass-through in both P2 and P3 (no
Golem enrichment to port). Added `get_environment_p3` to the
`host-api-tests` Environment agent using the native
`wasi:cli/environment@0.3` import (`golem_rust::wasip3`) and extended the
`environment_variables` worker-executor test to assert the P3 result equals
the P2 result (which is checked against the full enriched variable list).
Verified: rebuilt `golem_it_host_api_tests_release.wasm`; the component
imports both `wasi:cli/environment@0.2.12` and `@0.3.0`; test passes.

### T19 — Filesystem parity holes (G12)

In `p3/filesystem.rs`: route `metadata_hash`/`metadata_hash_at` through the
durable `stat`/`stat_at` results; add `fail_if_read_only` enforcement to
`set_times`, `set_times_at`, `rename_at`, `symlink_at`, `unlink_file_at`; mask
the write bit in `get_flags` for read-only workers (match P2
`filesystem/types.rs` exactly).

**Verify:** existing filesystem worker-executor tests stay green; add unit or
integration tests for read-only enforcement and metadata-hash replay
determinism (live vs replay hash equality).

**Note (done):** P3 `metadata_hash`/`metadata_hash_at` now compute the hash
from the durable `stat`/`stat_at` results via a shared
`calculate_metadata_hash_parts` helper (extracted from P2
`filesystem/types.rs`), so live and replay hashes are deterministic and
byte-identical to P2 (including the `ErrorCode::Overflow` behavior for
pre-epoch timestamps, covered by unit tests). Added read-only worker
enforcement (`fail_if_read_only`, checked before durable observation) to
`set_times`, `set_times_at`, `rename_at`, `symlink_at`, `unlink_file_at`, and
`get_flags` masks the write bit for read-only descriptors, matching P2. Added
`p3_parity` module to the `initial-file-system` test component (native
`wasi:filesystem@0.3` imports) and the `initial_file_p3_parity`
worker-executor test asserting P2/P3 stat + metadata-hash parity both on first
invocation and after crash/restart replay. All existing filesystem tests
remain green.

### T20 — Keyvalue leftovers: atomic ops + async body (G16)

Decide and implement: either implement `keyvalue::atomic::increment` /
`compare_and_swap` (durable, `WriteRemote`) and
`outgoing_value_write_body_async` (buffered like the blobstore
`OutgoingValueWriteConsumer`), or remove them from the vendored WIT. Traps via
`unimplemented!()` are not acceptable guest-visible behavior.

**Verify:** build; if implemented: payload roundtrip tests + a worker-executor
test per operation; if removed: WIT diff reviewed, `cargo make wit` green,
SDK WIT copies synced.

**Note (done):** Split decision per API. (1) Removed `wasi:keyvalue/atomic@0.1.0`
from the exposed WIT surface (`wit/deps/keyvalue/atomic.wit`, the `world.wit`
and `host.wit` imports, `durable_host/keyvalue/atomic.rs`, and the linker
registration) — it had never been implemented and correct atomics need
separately designed cross-backend semantics. (2) Implemented
`outgoing-value-write-body-async` as a P3 stream-consumer API: the WIT
signature is now `func(data: stream<u8>) -> result<_, error>` and the host
pipes the guest-provided readable stream into the outgoing value's body buffer
(`OutgoingValueWriteConsumer`, mirroring the blobstore pattern). The consumer
does no oplog recording; durability stays owned by the durable consumers of the
outgoing value (`eventual::set`, batch set, cache vacancy fill). Both changes
are same-version (`@0.1.0`) breaking changes: precompiled components importing
atomics or the old no-argument async-body method must be rebuilt. `cargo make
wit` synced all SDK WIT copies; the hand-maintained MoonBit worlds no longer
import atomics. Stale *generated* MoonBit/TS/Scala bindings (which still show
the even older `output-stream`-based form) are intentionally deferred to the
SDK regeneration workflows. Verified: `cargo check -p golem-worker-executor
--tests` and `-p golem-debugging-service` clean; rebuilt
`golem_it_host_api_tests_release.wasm` with a new `set_using_async_body` agent
method (chunked `wit_stream` writes); all 15 `keyvalue::` worker-executor tests
pass including the new
`readwrite_get_returns_the_value_that_was_set_using_async_body` test with
crash/replay coverage; docs `next/develop/additional.mdx` updated (v1.5 left
frozen).

### T21 — Signed time conversion hardening (G20)

In `golem-common/src/model/oplog/payload/types.rs`: replace the panicking
`SystemTime → SerializableDateTime` (`duration_since(UNIX_EPOCH).unwrap()`,
~lines 143-149) with signed-safe conversion; make an explicit, documented
decision about negative-seconds clamping in `SerializableDateTime →
SystemTime`/P2 `Datetime` (keep clamping only where the target type cannot
represent the value, and document it).

**Verify:** `cargo test -p golem-common` including new unit tests with
pre-epoch instants (negative seconds) through every conversion direction.

**Note (done):** Documented `SerializableDateTime` semantics (floored seconds
since epoch, negative pre-epoch; nanoseconds in `[0, 1e9)` on top — matching
WASI P3 `system-clock` `instant`, so P3 conversions stay lossless field
copies). `SystemTime → SerializableDateTime` no longer panics: pre-epoch
instants convert to floored negative seconds + nano offset (computed in i128 so
the `i64::MIN` boundary is exact), with saturation to `{i64::MIN,0}` /
`{i64::MAX,999_999_999}` only outside the representable i64 range.
`SerializableDateTime → SystemTime` preserves pre-epoch instants via
`checked_sub`, clamping to the epoch only when the platform `SystemTime` cannot
represent the instant (e.g. pre-1601 on Windows). Clamping decisions are
documented at each conversion: P2 `wall-clock` `datetime` (unsigned seconds)
clamps pre-epoch to `{0,0}` (previously it kept nanoseconds, drifting
post-epoch); its reverse direction saturates `u64 → i64` instead of wrapping.
Chrono conversions are now direct field-based (no SystemTime detour):
out-of-chrono-range values clamp to `MIN_UTC`/`MAX_UTC`, leap-second nanos
clamp to `999_999_999`. `wall_clock.rs` in the worker executor now uses the
shared `From` impl instead of a manual `as i64` cast. Tests: extended
`systemtime_strat` to cover pre-epoch instants (proptest roundtrip both sides
of the epoch, nanos range fixed to include `999_999_999`), plus deterministic
boundary tests (epoch±1ns/1s, exact and fractional `i64::MIN`, saturation,
chrono bounds, P2 clamp, P3 lossless copy). Verified: full
`cargo test -p golem-common` green (824 lib tests + doctests, 38 payload
tests), `cargo check -p golem-worker-executor` clean. Oracle-reviewed twice
(initial review found i64-boundary panics; all findings fixed and re-approved).

---

## Phase 5 — durability core hardening

### T22 — PersistNothing on the live accessor path (G19)

Audit `execute_access_start` (`durable_host/concurrent.rs`): prove or fix that
a live P3 durable call under `PersistenceLevel::PersistNothing` writes no
`Start`/`End` entries (only snapshotting is visibly handled today). Align with
the legacy `Durability` behavior and the replay guard at
`concurrent.rs:1513-1522`.

**Verify:** unit test in `concurrent.rs` tests: live call under PersistNothing
produces zero oplog entries; existing
`transactions::golem_rust_persist_nothing*` tests stay green (fully verified
post-T41 when those move to P3).

**Note (done):** Audited and aligned the live accessor path with the legacy
`Durability` PersistNothing behavior. Covered by the new
`persist_nothing_zone_suppresses_durable_commits_of_live_host_call_entries`
test in `golem-worker-executor/src/services/oplog/tests.rs` (a persist-nothing
zone keeps live host-call entries out of the durably committed oplog).
Oracle-reviewed.

### T23 — Cancellation-drain tests + stale comment (G19)

Fix the stale "no recorder actor in production yet" comment
(`concurrent.rs:352-355`) to describe the wired drain model. Add tests:
(a) guest drops a `Cancellable` P3 call mid-flight live → `Cancelled` recorded
at next drain point; (b) worker interrupt mid-call → incomplete `Start`,
consistent replay; (c) replay of a recorded `Cancelled` with and without
partial result.

**Verify:** new tests in `concurrent.rs`/`replay_state.rs` test modules pass;
`cargo test -p golem-worker-executor --lib -- durable_host::concurrent`.

**Note (done):** Stale comment rewritten to describe the wired drain model.
New tests: `dropped_cancellable_call_records_cancelled_at_next_drain_point`
and `dropped_unfinished_call_keeps_live_permit_until_drop_event_is_consumed`
(`concurrent.rs`), `interrupted_call_reports_incomplete_while_sibling_completes`,
`replay_resolves_cancelled_without_partial`, and
`replay_resolves_cancelled_with_partial_result` (`replay_state.rs`).
`durable_host::concurrent` lib tests green. Oracle-reviewed.

### T24 — Pre-migration oplog compatibility test + notes update (G18)

Add a test that replays a representative pre-migration (P2-era, adjacent
entry) oplog through the concurrent resolver — either a fixture captured from
a released build or synthesized entries using the old shapes. Update
`p3-migration-notes.md` to describe the implemented design (Start-index as
call identity, no format bump) and why it replaced the `call_id` sketch.

**Verify:** compatibility test passes; notes updated; existing
`keep_existing`/`keep_preexisting` decode tests in `golem-common` stay green.

**Note (done):** Added
`pre_migration_adjacent_pair_oplog_replays_through_concurrent_resolver`
(`replay_state.rs` tests) replaying synthesized P2-era adjacent
`Start`/`End`-style entries through the concurrent resolver.
`p3-migration-notes.md` gained the "Implemented design (supersedes the
`call_id` sketch above)" section describing Start-index-as-call-identity with
no oplog format bump. Oracle-reviewed.

### T25 — Un-ignore concurrent/suspendable durable-call tests (G19)

Remove `#[ignore]` from the `TODO(p3)` tests
(`golem-worker-executor/tests/wasi.rs:1864,1929,1993,3338,3426`) and fix
whatever they surface — they need concurrent + suspendable durable async host
calls, so T09/T11 are prerequisites. (`tests/durability.rs:139` stays ignored
until T26.)

**Verify:** all five un-ignored tests pass individually via `cargo-test-r`.

**Note (done):** All five `#[ignore]`s removed; `tests/durability.rs:139`
stays ignored until T26. Two fixes were needed:

1. Replay of a recorded `Cancelled { partial: None }` terminal
   (`CallHandle::replay_access`): the future now parks on
   `std::future::pending()` (marked via `parked_cancelled_replay`) so the
   deterministic guest drops it at the same point it did live; all durable
   cleanup (scope close) is deferred to the handle's `Drop` via the idempotent
   `CloseDurableScope` event, keeping the path cancellation-safe (no await
   between terminal resolution and the park).
2. Replay-cursor lock wedge: `run_concurrent` returns as soon as the root
   future resolves, and a store-spawned durable task could stay frozen inside
   an open replay-cursor transaction, deadlocking completion-path cursor reads
   (e.g. `AgentInvocationFinished`) outside the event loop. Added
   `ReplayState::has_open_cursor_transaction()` and a `settle_replay_cursor`
   step at the top of `finish_invocation_and_get_fuel_consumption`
   (`worker/invocation.rs`) that re-enters the event loop until no cursor
   transaction is open (1ms-sleep probe loop; deliberately not a full
   spawned-task drain — that is T28).

Verified: all five tests pass individually via `cargo-test-r`;
`durable_host::concurrent` lib tests green; `cargo check --all-targets`
clean. Oracle-reviewed twice (initial review found a cancellation window in
the `Cancelled(None)` park and a busy-spin in the settle loop; both fixed and
re-approved).

**Follow-up (deferred, ~M):** integration regression covering the
scope-opening (`WriteRemote` non-idempotent / `WriteRemoteBatched(None)`)
`Cancelled(None)` chain: guest drops the race loser while the scope-terminal
oplog read is pending → parked drop → `CloseDurableScope` drain → verify the
scope `End` and `AgentInvocationFinished` both replay. None of the five
un-ignored tests exercises a scope-opening call through the parked path.

---

## Phase 6 — WIT design decisions

### T26 — P3 replacement for `lazy-initialized-pollable` (G14)

Design and implement a P3-native replacement for the removed
`lazy-initialized-pollable` in `golem:durability`
(`wit/deps/golem-durability/golem-durability.wit:79-88`, host TODO at
`durable_host/durability.rs:958`). Likely shape: a host-created
`future<T>`/deferred-result resource the guest can complete/await. Update WIT,
host impl, `golem-rust` SDK, and the `custom_durability.rs` test component
TODOs. Sync WIT copies (`cargo make wit`, SDK wit dirs).

**Verify:** `durability::custom_durability_1` stays green;
`wasi::oplog_replay_after_streaming_http_read` covers the async-streaming +
restart/replay axis; the feature-specific `durability::lazy_pollable` test is
deleted (no API to test); WIT drift checks green.

**Note (done):** Resolved as a design decision: **removed with no
replacement** (Oracle-reviewed; golem-ai usage audited via Librarian). The
p2 resource provided a level-triggered, reusable, *rebindable* readiness
handle — p3 `future`/`stream` handles are one-shot and linear, so no
future-based design (host- or guest-side) can reproduce that contract, and
no consumer needs it: golem-ai never exposes the pollable through its
caller-facing WIT (it only feeds internal `blocking-get-next` loops), and
its Bedrock `nopoll` build already runs the same durable replay/continuation
state machine with all pollable machinery compiled out. In p3 the
replay→live transition is expressed by resolving replayed results
immediately and directly awaiting the live source (host `stream<u8>` mapped
in-guest to event batches via guest-created `wit_stream`/`wit_future`
handles — no host support needed). Changes: commented-out resource + TODO
replaced by a decision note in `golem-durability.wit` (synced via `cargo
make wit`), host TODO removed, commented `lazy_pollable_*` test-component
code deleted, ignored `durability::lazy_pollable` test **deleted** (not
rewritten — its coverage axes are already held by
`durability::custom_durability_1` (custom durability + PersistNothing +
restart/replay) and `wasi::oplog_replay_after_streaming_http_read`
(streaming chunked HTTP + restart + full replay)), and
`p3-migration-notes.md` updated (decision record, golem-ai migration recipe
without a replacement API, suspend-mechanism scoping corrected). No SDK
primitive added: a `LazyInitializedFuture` would promise rebindable
semantics the ABI cannot deliver.

### T27 — Websocket WIT decision (G15) — DONE

Decision: keep the existing request/response API shape but mark `receive` and
`receive-with-timeout` as `async func` in `golem:websocket` WIT, and move
their host implementations to the accessor-based (`*WithStore`) path so they
do not hold the Wasmtime store while parked — other guest tasks progress
concurrently while a receive waits. A full P3-native `stream<message>`
redesign (TCP-receive durable-stream pattern) was explicitly deferred as a
possible separate follow-up.

Note this **is** an ABI change for the two functions (`async func` is a
distinct component function type): guest components built against the old
sync WIT must be rebuilt against the updated WIT to link. The durable record
shape is unchanged (`WriteRemote` host calls with the same request/response
payloads), so existing oplogs replay unchanged; `CallHandle` replay/reconnect
behavior is preserved. Handles use `LeaveIncompleteOnDrop` so a guest-side
cancellation mid-receive leaves the `Start` incomplete for live re-execution
on replay, matching interrupt semantics. Rust SDK's async `receive()` /
`receive_with_timeout()` implemented; `blocking_*` wrappers use
`wit_bindgen::block_on`. Checklist item #16 flipped.

**Verify:** existing websocket worker-executor tests pass (extend them if the
API changed); WIT sync checks green. `websocket_echo_ts` stays red until T47
(TS SDK migration) regenerates the TS websocket types/bindings and rebuilds
`agent-sdk-ts` — its wasm is stale (pre `golem:core@2.0.0`), unrelated to this
change.

---

## Phase 7 — Part 3 gaps (execution model, tooling, safety)

### T28 — Drain spawned guest tasks before invocation completion (G25)

Switch the four `run_concurrent` call sites in `worker/invocation.rs`
(~160-260) to drain tail work (wasmtime `run_concurrent_and_drain` or a
`poll_no_interesting_tasks` loop) before
`AgentInvocationFinished` is written. Define and document semantics for tasks
that never finish (bounded drain + trap/cancel decision). Ensure durable calls
made by tail tasks land before the invocation-finished entry.

**Verify:** new test component + worker-executor test: guest `spawn`s a task
that performs a durable host call after the export returns; assert oplog
ordering (`Start`/`End` before `AgentInvocationFinished`) and successful
replay after restart. Existing invocation tests stay green.

Known reproducers (observed during the T16 checkpoint sweep):
`wasi::http_timeout_and_restart` (the durable consume-body task spawned by a
sync `consume-body` host call never runs before the invocation completes live,
so its `Start` is missing on replay — "no matching Start" error) and
`wasi::oplog_replay_after_parallel_streaming_http_reads` /
`..._raw_...` (a durable call dropped unfinished at invocation end records
`Cancelled` without partial, which replay rejects). A naive switch of the four
`run_concurrent` call sites to `run_concurrent_and_drain` makes these pass but
hangs `http::outgoing_http_response_future_cancel_aborts_request_and_replays`
in replay drain — the bounded-drain semantics called for above are required.

**Status: done.** Shipped as an internal-settlement design (oracle-approved,
two review rounds):

- Patched wasmtime gained a public `StoreContextMut::run_concurrent_and_settle`
  (`crates/wasmtime/src/runtime/component/concurrent.rs`): after the root
  future completes, the event loop keeps running until an idle observation
  point (all runnable host futures polled, no queued work, no remaining
  `interesting_tasks`) where an embedder settlement predicate holds; parked
  host futures may be left behind. Covered by 5 new scenarios in
  `crates/misc/component-async-tests/tests/scenario/settle.rs` (85/85 suite
  green).
- Golem side: `durable_host/tail_work.rs` adds `TailWorkTracker` with a
  non-clone RAII `TailActivity` permit and explicit `TailActivity::park()`
  safe-park points; every store/accessor spawn site under `durable_host`
  (p3 cli/filesystem/sockets/http replay/request- and response-body,
  blobstore, keyvalue) is instrumented.
- All guest export calls in `worker/invocation.rs` (initialize, invoke,
  save/load snapshot, and the non-accessor `call_process` via a no-op root)
  go through `run_guest_call_settled`, whose settlement predicate requires
  zero active tail tasks and no open replay-cursor transaction (the former
  external 1 ms `settle_replay_cursor` probe loop was removed). The drain
  phase is bounded by `TAIL_WORK_SETTLE_TIMEOUT` (30 s), armed when the root
  future returns; hitting it traps the invocation without writing
  `AgentInvocationFinished`, so normal retry replays it (crash-equivalent).
- Replay-side leak guard (`spawn_replayed_request_leak_guard` in
  `p3/http/replay.rs`, armed/disarmed in `p3/http/send.rs`) reclaims and
  drains a replayed outgoing-request resource when the replay send future is
  cancelled mid-flight, so guest-side cancellation settles during replay too.
- Tests: new worker-executor test
  `spawned_guest_task_durable_call_lands_before_invocation_finished`
  (component `http-tests/src/http_client_4.rs` using
  `wit_bindgen::spawn_local`, wit-bindgen 0.58 async+async-spawn) asserts the
  spawned durable call's `Start`/`End` land inside the invocation window and
  replay succeeds after restart; the known reproducers
  `oplog_replay_after_parallel_streaming_http_reads` and
  `outgoing_http_response_future_cancel_aborts_request_and_replays` pass;
  tail-work/replay-drain unit tests (9) pass. `http_timeout_and_restart`
  still fails, but identically on the untouched t27 baseline (pre-existing,
  not a T28 regression). `plugins::oplog_processor` fails with the guest-side
  `cannot block a synchronous task before returning` trap — the sync WIT
  `process` doing P3 HTTP is the separately tracked T33 (G29), unrelated to
  the T28 drain.

### T29 — Interruption/timeout for parked P3 host futures (G26)

Implement a cancellation path that reaches an in-progress invocation parked in
a P3 host future: deliver worker interrupts to the running `run_concurrent`
future (e.g. select against the interrupt signal and drop the invocation
future, relying on `CallHandle` drop policies / `abandon_for_trap` for oplog
consistency). Add an optional max-invocation-duration config (wrapping
`run_concurrent`) — decide default off/on.

**Verify:** new tests: interrupt a worker blocked in a P3 HTTP send / TCP
receive / `wait_for` → worker transitions to Interrupted promptly and resumes
correctly after restart; oplog is consistent (incomplete `Start` handled).

**Note (done):** Shipped as **cooperative, per-park interrupt delivery**
(Oracle's recommended option A) — the "race and drop the whole
`run_concurrent_and_settle` event loop" prototype was rejected (it panicked on
`CallHandle<_, NotCancellable>` and stranded non-idempotent writes; there is
no forced-drop backstop anywhere). Each blocking P3 host op obtains an
interrupt future via `DurabilityHost::create_interrupt_signal()` and races it
against the live I/O; on interrupt it abandons durable handles
child-before-parent via `abandon_for_trap()` (no `Cancelled`/`Failed`
terminal — the incomplete `Start`s re-execute on resume) and returns the
original `InterruptKind` as the Wasmtime error so it classifies as
`TrapType::Interrupt`. Instrumented park points: `suspendable_wait.rs` (new
`ParkOutcome::Interrupted(InterruptKind)`, interrupt raced on all wait
branches), `p3/clocks.rs` `wait_until_live`, promise wait in `golem/v1x.rs`,
`p3/http/send.rs` (interrupt branch now abandons + propagates the kind),
`p3/http/response_body.rs` reads, and `p3/sockets.rs` TCP receive (both the
bytes channel and the socket-result await). The optional
**max-invocation-duration** config landed as
`Limits::max_invocation_duration: Option<Duration>` (humantime-serde,
**default off**): `invoke_observed` arms an `InvocationDeadline` guard
(`durable_host/mod.rs`) whose timer latches
`invocation_deadline_exceeded: Arc<AtomicBool>` and broadcasts a *synthetic*
`InterruptKind::Interrupt` on the running-status interrupt channel without
touching `ExecutionStatus`; `create_interrupt_signal()` short-circuits on the
latch (parks created post-deadline still observe it) and the epoch
`check_interrupt` covers CPU-bound guests. At the invocation boundary
`apply_invocation_deadline` converts the synthetic Interrupted into a typed
`InternalError` timeout failure (`TrapType::Error`, crash-equivalent retry, no
`AgentInvocationFinished`), skipped when `is_interrupting()` so a genuine
external interrupt racing the deadline keeps first-cause precedence.
Side change: `.inherit_network()` added to the shared `WasiCtx`
(`wasi_host/mod.rs`) — P3 `TcpSocket::connect` fails without it; gating raw
sockets behind config is a possible follow-up. New tests (all pass):
`api::max_invocation_duration_aborts_long_running_invocation`,
`http::interrupt_while_parked_in_p3_http_response_wait`,
`wasi::interrupt_while_parked_in_p3_sleep`,
`wasi::interrupt_while_parked_in_p3_tcp_receive`; full interruption-related
sweep: 26 passed, 3 pre-existing baseline failures unrelated to T29
(`file_update_1` fails identically with T29 stashed;
`delete_interrupts_long_rpc_call` and `ts_v2_s3_process_crash_mid_workflow`
blocked on stale TS components / `wasm-rquickjs` async-export rebuild issue).
Oracle-reviewed ("change requested" → A adopted). **Accepted limitations /
deferred:** (a) timeout unwind via `abandon_for_trap()` carries no
`DurableCallTrapContext`, so the timeout error uses ambient fallback retry
state, which can pick the wrong `retry_from`/atomic-region membership with
overlapping durable calls; (b) Oracle preferred a sticky first-writer-wins
`InvocationAbort { External, DeadlineExceeded }` watch signal over the shipped
synthetic-interrupt + boundary-conversion (called it "transitional"), and a
dedicated user-interrupt-vs-deadline precedence race test was not added;
(c) some Oracle verification targets remain untested (interrupt racing a TCP
chunk arrival, `!assume_idempotence` permanent recovery failure, finite retry
exhaustion reaching `Failed`).

### T30 — Reject `stream`/`future`/`error-context` in agent schemas (G27) — DONE

Add upload-time validation (in `AgentTypeSchema::validate` or the extraction
path) rejecting `SchemaType::Future`/`Stream` (and any error-context
representation) in agent method input/output/config schemas with a clear error
message. Add output-schema validation after `decode_invoke_output` so
mismatched guest returns fail deterministically.

Done: `AgentTypeSchema::validate` (golem-common `schema/agent/mod.rs`) now
rejects `SchemaType::Future`/`Stream` anywhere in an agent type's schemas —
shared graph defs, constructor/method inputs, method outputs, config value
types, and every dependency's graph/constructor/methods — with a location-
specific error (`error-context` has no `SchemaType` representation, so
nothing to reject there). The registry upload path
(`analyze_and_validate_component_wasm` in registry `component/write.rs`) now
calls `validate()` per agent type, surfacing the rejection as a
`ComponentProcessingError::Metadata` bad request; the extraction path already
called `validate()` at discovery time. `lower_invocation` now carries the
method's declared output shape (`ExpectedInvokeOutput`: agent graph + output
root, canonical empty tuple for `unit`) into `dispatch_call`, which validates
the decoded guest return via `validate_value` right after
`decode_invoke_output` and fails with a deterministic
"does not match its declared output schema" runtime error.

**Verify:** unit tests in `golem-schema`/`golem-common`: schema containing
`stream<u8>` is rejected at validation with the expected message; integration:
uploading such a component fails cleanly; output-validation unit test.
Verified: 6 new `validate_*` P3-stub tests in
`golem-common/src/schema/agent/tests.rs` (method output/parameter,
constructor nesting, shared defs, config value type, dependency methods) and
5 new `validate_invoke_output` tests in
`golem-worker-executor/src/worker/invocation.rs` (unit/typed accept+reject,
ref resolution through the agent graph) all pass; existing lowering and
schema agent tests stay green.

### T31 — Orphan-terminal handling in concurrent replay (G28)

Make replay robust to `End`/`Cancelled` entries whose `Start` lies in a
skipped/deleted region (today: unexpected-entry failure). Recommended: when a
terminal's `start_index` falls inside a skipped/deleted region, skip it
explicitly. Extend `replay_skips_deleted_regions_fuzz` to generate partial
deletions (Start-only, terminal-only) and `Cancelled` terminals.

Done: `CursorTx::try_get_oplog_entry` now auto-drains *orphan terminals* —
`End`/`Cancelled` entries whose `start_index` lies inside a skipped/deleted
region (`is_orphan_terminal`): they are committed and skipped like awaited
terminals instead of being handed to a positional reader as an unexpected
entry. This covers every positional read path (all reads funnel through the
same drain loop). Two latent deleted-region bugs surfaced by the extended
fuzz were fixed as well: (a) `get_out_of_skipped_region` looked up the next
region from the just-jumped region's end, so a *single-entry* region re-found
itself and left the genuinely next region untracked (deleted `Start`s became
claimable) — the lookup now starts after the region and the jump loops to
handle adjacent regions; (b) `scan_oplog`'s advance-past-region-end branch was
dead code (the `contains` check `continue`d first), so scans only ever skipped
the first pending deleted region — the advance now happens inside the skip
branch.

**Verify:** extended fuzz + new deterministic unit tests in
`replay_state.rs` pass; existing replay tests green.
Verified: 4 new deterministic tests (`orphan_end_with_deleted_start_is_skipped`,
`orphan_cancelled_with_deleted_start_is_skipped`,
`positional_reader_skips_orphan_terminal`, `deleted_terminal_reports_incomplete`)
and `replay_skips_deleted_regions_fuzz` extended with per-pair partial deletions
(`Kept`/`Pair`/`StartOnly`/`TerminalOnly`) and randomized `Cancelled` terminals
(500 seeds) all pass; the full `golem-worker-executor --lib` suite (599 tests)
is green.

### T32 — Guards for jump/revert/fork cut points (G28)

Add guards preventing region operations from stranding in-flight calls:
`set_oplog_index` refuses (or waits) while durable calls are in flight
(mirror `mark_end_operation`, `v1x.rs:459-475`); external fork and revert
validate the cut point is not inside an active durable scope/atomic region
(walk the oplog around the cut). Document the snapshot-boundary invariant (no
open durable calls spanning a snapshot) and enforce it at snapshot points
(`can_checkpoint` style check already exists for mid-invocation checkpoints —
reuse it).

Done: new cut-point scanner `worker/cut_point.rs`
(`find_construct_spanning_cut_point`) walks the oplog from a proposed cut to
the tip and detects paired durable constructs whose opening entry survives
the cut while their terminal would be removed: durable-call `End`/`Cancelled`
referencing a surviving `Start`, `EndAtomicRegion` referencing a surviving
`BeginAtomicRegion`, remote-transaction terminals
(`PreCommit`/`PreRollback`/`Committed`/`RolledBack`) and retried
`BeginRemoteTransaction` referencing a surviving begin — all
skipped/deleted-region aware. Revert (`Worker::revert_to_last_oplog_index`)
and external fork (`DefaultWorkerFork`) reject such cut points with a clear
invalid-request error. Live `set_oplog_index` now drains queued dropped-call
events and refuses to jump while live host calls are in flight
(`has_in_flight_live_host_calls`), so a later terminal can never reference a
`Start` removed by the jump. The snapshot-boundary invariant is enforced via
`DurableWorkerState::at_safe_snapshot_boundary` (live mode, no open atomic
regions/durable scopes/in-flight host calls, persistence enabled, no snapshot
already active), surfaced through
`UpdateManagement::is_at_safe_snapshot_boundary`: the invocation loop skips
periodic snapshots and fails manual snapshot-based updates cleanly until the
boundary is safe. Incidental fix: `MultiLayerOplog::read_many` underflowed
(`attempt to subtract with overflow`) when a layer returned more entries than
requested (archived layers read whole chunks) — the subtraction now
saturates.

Verified: 13 new unit tests in `worker/cut_point.rs` (clean cuts, durable
calls, cancellation, atomic regions, all transaction terminal types, retries,
deleted regions) pass; new P3-async executor test
`jump_with_in_flight_durable_call_fails` (guest races a slow P3 HTTP request
against `set_oplog_index`, using new `jump_during_request` method in
`test-components/host-api-tests`) passes; `transactions::golem_rust_jump` and
all 4 `revert::` executor tests green; `integration-tests` fork suite: 7/10
green — `fork_and_sync_with_promise` fails from a pre-existing unrelated
component/linker mismatch (`golem:core/types@1.5.0` import), and
`fork_running_worker_2` / `fork_interrupted_worker` hit a pre-existing
replay divergence (`no matching Start` for a `consume-body` scope when the
fork cut lands mid-HTTP-request) that reproduces identically on the base
commit without these changes.

**Verify:** unit/integration tests: jump with in-flight call errors cleanly;
fork/revert at a mid-call cut point is rejected with a clear error;
existing `revert.rs`, `transactions::golem_rust_jump`, and
`integration-tests/tests/fork.rs` stay green; add one P3-async variant test.

### T33 — Oplog processor: async ABI + P3-entry coverage (G29)

Invoke `LoweredCall::ProcessOplogEntries` via `run_concurrent` like other
guest calls (`worker/invocation.rs:277-290`). Replace the panicking `expect`
in `encode_public_typed_schema_value` with a proper error. Document that P3
payloads surface as `typed-schema-value`. Add an integration test: source
worker performs P3 HTTP + stream ops with an oplog processor installed; assert
delivery succeeds and `enrich_oplog_entries` handles `Start`/`End`/`Cancelled`
+ P3 payloads.

**Verify:** new integration test passes (`integration-tests`); existing
`plugins.rs` oplog-processor tests stay green.

### T34 — Debugging service concurrent-entry fixes (G30, G22)

(a) Handle incomplete Starts safely in debug mode: decide behavior when a
playback target lands between `Start` and terminal (recommended: refuse live
repair / re-execution in debug sessions, surface as explicit "target inside
in-flight durable call"); review `DebugOplog::add*` returning
`OplogIndex::NONE`. (b) Validate playback overrides against Start/End pairing.
(c) Add a P3 counterpart for the P2-only cli-environment debug override
(`debug_context.rs:473-486`). (d) Add tests: debug session over a worker with
P3 durable calls; playback with `ensure_invocation_boundary=false` targeting
on/inside/after a `Start`.

**Verify:** new tests in `golem-debugging-service/tests/debug_tests.rs` pass;
existing debug tests green.

### T35 — Suspend heuristics aware of pending P3 work (G31)

Guard the P2 `poll` suspend paths (`io/poll.rs:111-138`, `:237-274`): do not
suspend the worker while the store has pending P3 host tasks/futures (use
wasmtime task-quiescence state, e.g. a `poll_no_interesting_tasks`-style
check, or Golem-side tracking of live `CallHandle`s/spawned tasks). Document
the intended mixed-ABI suspension semantics.

**Verify:** new mixed-ABI regression test: one guest task awaits a slow P3
`client::send` while another blocks in P2 poll/sleep — the P3 completion is
delivered without the worker suspending prematurely. Existing suspend tests
stay green.

**Note (done):** Implemented via Golem-side tracking (no wasmtime
`poll_no_interesting_tasks` needed): the P2 `poll` suspend-for-sleep path in
`io/poll.rs` no longer directly schedules a wakeup and traps — it parks in the
shared `park_suspendable_wait` mechanism (same as the P3 `monotonic-clock`
waits) with `PrivateDurableWorkerState::safe_to_suspend()` as the suspension
predicate, which is true only when every live durable host call is itself
parked in a registered suspendable wait; pending P3 host work (e.g. an
in-flight `wasi:http` send from another guest task) therefore blocks
suspension. If the sleep deadline is reached before suspension becomes safe,
the poll is re-executed in a loop (consumed borrowed pollables recreated from
saved reps); ephemeral max-sleep and interrupt handling flow through the same
park (`ParkOutcome`). The mixed-ABI suspension semantics are documented on
`Host::poll`. Hardening found by review: `park_suspendable_wait` re-checks
`safe_to_suspend()` after the pre-suspend yield and again (synchronously, no
awaits before `SuspendWorker`) after the async wakeup scheduling, so new live
host calls started by other guest tasks in those windows keep the worker
parked instead of being dropped by a stale suspend decision (unit tests
`new_live_host_call_during_wakeup_scheduling_prevents_suspend`,
`safety_revoked_during_pre_suspend_yield_prevents_suspend`, both verified via
negative control). The legacy promise-backed-pollable immediate-suspend fast
path is additionally gated on `safe_to_suspend()` and documented as
unreachable (the `promise_backed_pollables` map has no insertion sites since
the P3 promise-result API replaced P2 promise pollables); if registrations are
ever reintroduced the path must become a suspendable-wait park. Removed the
dead `PrivateDurableWorkerState::sleep_until` wrapper. Regression test:
`wasi::p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold`
(new `p2_sleep_during_request` agent method in `host-api-tests` joins a slow
P3 HTTP request with a 15s P2 `thread::sleep`; a counting server asserts the
request is received exactly once — a second request would mean premature
suspend + re-execution). Negative control: against the unpatched executor the
test fails with a duplicated request. Full sleep/suspend suite (17 tests) and
promise tests stay green. Optional follow-up suggested by review: delete the
dormant promise-pollable maps and their fast/drop paths entirely.

### T36 — Embedder follow-ups: OTLP smoke test + library-plugin policy (G32)

(a) Add a smoke test that provisions and runs `plugins/otlp-exporter.wasm`
under the current executor linker (an oplog-processor integration test
variant). (b) Decide and document the cross-ABI library-plugin policy: add a
WAC composition fixture (P3-importing user component + representative library
plugin) and a clear error/documentation for P2-shaped plugins that cannot
connect.

**Verify:** smoke test green in `integration-tests`; composition fixture test
in `cli` or `integration-tests` demonstrating the supported/unsupported cases.

**Note:** (b) is obsolete — library plugin support was removed entirely in
PR #2798 (`Library` plugin spec variant deleted; `PluginSpecDto` now only has
`OplogProcessor`). The dead WAC composition module
(`cli/golem-cli/src/composition.rs`) and the unused `wac-graph` dependency
were removed as part of closing this task.

### T37 — Memory/backpressure hardening for P3 host tasks (G34)

(a) Cap/stream the P3 stdout/stderr capture path (`p3/cli.rs:280-300`) —
process chunks incrementally instead of whole-stream oneshot buffering, or
enforce a byte cap. (b) Convert the technically-unbounded demand channels in
p3 http/sockets to bounded (capacity 1) as defense in depth. (c) Verify
eviction classification cannot see `waiting_for_command=true` while
store-spawned P3 tasks are live (interacts with T28's drain — after T28 this
should hold; add an assertion/test).

**Verify:** unit tests for capped capture; existing stdout + streaming tests
green; a test writing a large stdout stream does not accumulate unbounded
memory (assert chunked event emission).

**Status: done.** Oracle-approved (two review rounds; first round found two
replay-correctness blockers that were fixed before approval):

- (a) `p3/cli.rs`: `CapturingOutputStreamConsumer` replaced whole-stream
  oneshot buffering with an ack-gated incremental protocol: bytes accumulate
  in a bounded buffer and a chunk (`STDIO_LOG_CHUNK_MAX_BYTES` = 8 KiB) is
  emitted exactly when the accumulator fills, the remainder flushing on
  stream close; the emitting `StdioWriteTask` produces one log event per
  chunk and must acknowledge it before more input is consumed (at most one
  accumulator + one in-flight chunk ≈ 16 KiB per open stream). Chunk
  boundaries are a pure function of the cumulative byte stream — independent
  of guest write sizes *and* host producer buffer segmentation (first oracle
  round showed `write-via-stream` can be fed by host-to-host streams whose
  buffer boundaries are timing-dependent) — keeping message-hash replay dedup
  stable. `utf8_safe_split_point` holds back ≤3 trailing bytes so multi-byte
  UTF-8 scalars are never split (lossy-conversion corruption at chunk
  boundaries). Because identical 8 KiB chunks of repetitive output now recur,
  `ReplayState::log_hashes` became a counted multiset
  (`HashMap<(u64,u64), usize>`) so each persisted duplicate is deduplicated
  exactly once on re-run.
- (b) `p3/http/response_body.rs` and `p3/sockets.rs`: the consume-body demand,
  TCP receive demand, and TCP receive permit channels converted from
  unbounded to bounded capacity 1 (each protocol keeps at most one message in
  flight); the impossible-`Full` permit case logs and continues (deliberately
  no `debug_assert!` — unwinding would drop the open `NotCancellable` child
  handle, which panics).
- (c) `worker/invocation_loop.rs`: `check_no_active_tail_work_on_idle` runs
  before both `waiting_for_command.store(true)` sites, logging an error +
  `debug_assert` if any Golem-spawned store task is still active when the
  worker becomes idle-evictable (relies on T28's settlement; the 30 s
  settle-timeout path exits via `BreakInnerLoop` and never reaches these
  sites).
- Tests: new lib unit tests — capped acknowledged chunk delivery,
  receiver-drop teardown, segmentation-independence (same bytes through
  differently segmented host producers ⇒ identical chunk vectors, all valid
  UTF-8), `utf8_safe_split_point` edge cases, `seen_log` multiplicity
  regression; full `replay_state` module (33) green; integration tests
  `wasi::write_stdout`, `wasi::oplog_replay_after_streaming_http_read`,
  `wasi::oplog_replay_after_parallel_streaming_http_reads`,
  `wasi::interrupt_while_parked_in_p3_tcp_receive`,
  `storage_quota::agent_quota_stream_to_stdout_does_not_charge_quota` green
  (debug build, so the idle assertion ran for every worker). No P3-CLI guest
  component exists yet (SDK migration lands later), so large-stdout chunked
  emission is verified at the store level by the consumer unit tests.

---

## Phase 8 — verification infrastructure & final migration

### T38 — Agent extraction: P3 `wasi:http` imports (G21)

Verify whether `wasmtime_wasi::p3::add_to_linker` covers `wasi:http` in
`golem-common/src/model/agent/extraction.rs:68-83`; if not, add the P3 HTTP
linker calls (mirroring the executor). Add an extraction test with a component
importing P3 `wasi:http` (the migrated `http-tests` component qualifies).

**Verify:** extraction unit/integration test passes for a P3-http-importing
component; CLI local extraction (`golem-cli` path) also covered.

**Status: done.** Oracle-approved. Verified that `wasmtime_wasi::p3::add_to_linker`
does *not* cover `wasi:http`, but no P3 HTTP linker call is needed: extraction's
`dynamic_import` deliberately satisfies every import outside its static WASI
allowlist — including P2/P3 `wasi:http` — with mock instances that fail loudly
if invoked (agent discovery must never perform real HTTP), and since shadowing
is enabled these mocks would shadow an explicit P3 HTTP linker anyway. The
linker comment in `extraction.rs` now documents this policy. Added regression
test `can_extract_agent_type_schemas_from_component_importing_p3_http` using
`golem_it_http_tests_release.wasm`, with a guard asserting the fixture actually
imports `wasi:http/*@0.3.*` so a rebuilt fixture cannot silently weaken the
test. The CLI local extraction path delegates to the same shared
`golem-common` implementation, so it is covered by the same test.

### T39 — Public oplog rendering tests for P3 entries (G23)

Add tests asserting P3 oplog payloads (`P3HttpClientSend`,
`P3HttpClientConsumeBody`/`Chunk`, P3 sockets/kv/blobstore) render correctly
through the public oplog API (`model/public_oplog/`) and `golem worker oplog`
(WIT conversion path), including `Start`/`End`/`Cancelled` entries.

**Verify:** new tests in `golem-worker-executor` public_oplog test module (and
a CLI integration test if cheap) pass.

**Status: done.** Oracle-approved (two review rounds; first round required a
coherent `Cancelled` fixture and coverage of the actual gRPC transport path,
both fixed before approval). New test module
`golem-worker-executor/src/model/public_oplog/tests.rs` with
`p3_payloads_render_through_public_oplog_api_and_wit`: writes P3 host call
entries (`P3HttpClientSend` success + http-error, `P3HttpClientConsumeBody`
trailers, `P3HttpClientConsumeBodyChunk`, P3 UDP send/receive, TCP
receive-chunk and acquire-error, keyvalue and blobstore incoming-value
streams) through a real in-memory `PrimaryOplogService`, plus a standalone
`Start` terminated by `Cancelled` carrying a matching partial
`SerializableP3HttpBodyChunk::Cancelled` payload. Each entry is rendered via
`PublicOplogEntry::from_oplog_entry` (with a panicking component-service stub
proving host-call entries need no component metadata) and asserted against the
exact expected typed-schema request/response/partial values; every rendered
entry is then round-tripped through the gRPC protobuf conversion (the
executor↔worker-service boundary used by `golem worker oplog`) asserting full
equality, and converted through the WIT representation used by the
in-component oplog API. No CLI integration test added — it would need a
running cluster and a component emitting these entries (not "cheap"); the
protobuf round-trip covers the transport boundary instead.

### T40 — Runtime verification for P3 HTTP durability (checklist #8)

With T01–T16 landed, run the deferred runtime verification that blocked
checklist item #8 (see `http3.md` Step 8 tests 3–8): replay roundtrip of
send + consume-body, deterministic error replay, concurrent overlapping sends,
real-future cancellation, body-stream cancellation mid-replay, `CallHandle`
ordering. Use the migrated `http-tests` component; add missing test cases to
it rather than building a separate harness.

**Verify:** all listed scenarios covered by named worker-executor tests, each
passing; flip checklist #8 to `done` in `p3-durability-checklist.md`.

**Status: done.** Oracle-approved. Scenarios covered by named tests in
`golem-worker-executor/tests/http.rs` (all passing, `http::` suite 18/18):
`outgoing_http_full_response_is_replayed_without_network` (send +
consume-body replay of status/header/body with no network),
`outgoing_http_send_permanent_error_is_recorded_and_replayed` (exact
`ErrorCode::TlsProtocolError` recorded and replayed, one connection),
`http_client_using_reqwest_async_parallel_replay` (16 overlapping sends,
responses released in reverse request-id order, oplog proves full overlap and
out-of-initiation-order Ends, claim-based replay),
`outgoing_http_response_future_cancel_aborts_request_and_replays` /
`outgoing_http_post_cancel_records_cancelled_and_replays` /
`outgoing_http_pending_body_read_cancellation_replays` (real-future and
body-stream cancellation for idempotent/non-idempotent sends). `CallHandle`
ordering (Seam-2) is an executable invariant:
`access_terminal_end_is_appended_before_cleanup_and_permit_release` in
`golem-worker-executor/src/durable_host/concurrent.rs` drives the extracted
production persistence stage (`CallHandle::persist_access_terminal`) against
a gated oplog and proves the terminal `End` is durable before the live-call
permit is released or any cleanup event becomes visible
(`durable_host::concurrent::` suite 30/30). Includes an approved fix in the
adjacent wasmtime repo (`crates/wasi-http/src/p2/connection_pool.rs`) making
`find_rustls_error` descend nested io::Error wrappers so bogus TLS bytes
classify as the permanent `TlsProtocolError` instead of
`DestinationUnavailable`.

### T41 — Migrate `host-api-tests` HTTP to P3 (G7)

Migrate `test-components/host-api-tests` off P2 HTTP and pollables:
`raw_http.rs` → P3 `wasi:http`/wasi-fetch; `raw_wasi_http.rs` (`RawWasiHttp`
agent) → migrate or delete (unreferenced today — prefer migrate, it exercises
raw P3 HTTP); `custom_durability.rs` needs only its `raw_http`-based callback
migrated (the lazy-pollable parts were deleted by T26); verify
`GolemWasiHttp` agent is still needed. Rebuild the component.

**Verify:** all tests listed under G7 pass on the P3 path:
`observability::invocation_context_test`, `durability::custom_durability_1`,
and all 12 `transactions::golem_rust_*` tests (`durability::lazy_pollable`
was deleted by T26);
`rg "wasi:io|outgoing-handler" test-components/host-api-tests/src` shows no P2
HTTP remnants.

**Status: done.** Oracle-approved. `raw_http.rs` migrated to `wasi-fetch`
0.2.0 (P3 `wasi:http@0.3.0`) with a local `Method` enum and
`wit_bindgen::block_on` wrapping for the sync API; `raw_wasi_http.rs`
(`RawWasiHttp` agent, retained) migrated to raw
`golem_rust::wasip3::http::{client, types}` with `wit_stream`/`wit_future`,
driving send, request-body/trailer handoff, and transmission concurrently,
checking the transmission result and acknowledging consumed response bodies;
`custom_durability.rs` callback migrated via `raw_http`; `GolemWasiHttp`
retained. `idempotence_flag` reworked for the async P3 client: the test
server records `/side-effect` on arrival but delays only the *first*
response (`x-test-first-response-delay-ms`, 30s), the guest hands over
body+trailers, races the pending send against a 2s P3 monotonic-clock
`wait-for` (`ReadLocal` — a benign interleave for the incomplete-send scan,
unlike a remote call), and panics inside an atomic region while the send is
incomplete; idempotence on re-executes the send (immediate second response,
events `["1","1"]`), idempotence off fails with the non-idempotent
incomplete-write guard (events `["1"]`). Component rebuilt; flat artifact
imports only `wasi:http/{client,types}@0.3.0`. All G7 tests pass (15/15):
`observability::invocation_context_test`, `durability::custom_durability_1`,
all `transactions::golem_rust_*`. The only `wasi:io` grep hit is a comment in
`clock.rs` about an intentional P2-sleep interleaving test (not HTTP).

### T42 — Full-suite sweep + documentation close-out

Run the complete affected test surface: all 43 `http_tests`-tagged tests, the
full `cargo make worker-executor-tests`, `cargo make integration-tests`, and
the debugging-service tests. Update `p3-gaps.md` (mark each gap resolved with
the closing task), `p3-durability-checklist.md` (rows #8, #15, #16), and
`p3-migration-notes.md` (resolved blockers). Remove stale TODO(p3) comments
that no longer apply.

**Verify:** zero failing tests in the suites above;
`rg -n "TODO\(p3\)" golem-worker-executor/src golem-common/src` returns only
intentionally-deferred items (each with a tracking reference).

**Status: in-progress (awaiting final oracle approval).**

Remaining sub-tasks (oracle review TU-033sYm4KxSOxSWlLSrhSc0 requirements plus
close-out):

| Sub-task | Description | Status |
|----------|-------------|--------|
| T42.1 | Test-utils gate fixes: `gate_first_consume_body_chunk_end` must *replace* an existing gate on re-arm (current `insert_async` silently no-ops when a disarmed gate is still in the map, wedging the second run), and `TestWorkerExecutor` must expose a public gate-arming accessor | done |
| T42.2 | New guest method `get_and_cancel_body_read_after_signal` in `test-components/http-tests/src/http_client_4.rs`: race a pending `body.chunk()` read against a test-server-controlled `/cancel-signal` request, then confirm the read drop via `/cancel-done`; rebuild `golem_it_http_tests_release.wasm` | done |
| T42.3 | Deterministic integration regression in `golem-worker-executor/tests/http.rs`: arm the gate, pause the producer after the child `End(Data)` is durable, drop the demand reply receiver, release, and assert `Start` → `End(Data)` → `CompletionDiscarded` → parent `End` → `AgentInvocationFinished` ordering with no guest byte delivery; restart + re-arm leg proves replay parks without redelivery. Surfaced and fixed a real replay deadlock: the replay `Data`/`End` arms retained the read's cancel plumbing (`cancel_rx`/`read_cancel_ack`) across the delivery boundary, so a cancelling guest blocked in sync `stream.cancel-read` while the replay-discarded delivery parked on the demand — a circular wait; the replay arms now drop the plumbing once the chunk is produced, mirroring the live path (where the read future owns and drops it). Test passes 5/5 (`/tmp/t42-gated-discard4.txt`, `/tmp/t42-gated-stability-{1..5}.txt`) | done |
| T42.4 | Refresh stale fixtures importing `golem:core/types@1.5.0` that have supported recipes (`agent-promise`, `agent-rpc`, `agent-sdk-ts`); document the two benchmark artifacts (no supported source recipe) as deferred. Rebuilt via `golem build -P release` + `golem exec copy` per app (`/tmp/t42-fixture-rebuild.txt`): `golem_it_agent_promise.wasm`, `golem_it_agent_rpc.wasm`, `golem_it_agent_rpc_rust_release.wasm`, `golem_it_agent_sdk_ts.wasm`. Remaining stale (deferred, no source recipe in-tree): `benchmark_agent_rust_release.wasm`, `benchmark_agent_ts.wasm` | done |
| T42.4a | Sleep-suspension fix surfaced by the first full sweep: `p3_sleep_suspends_and_resumes` / `p3_resuming_sleep` failed because a completed durable accessor call's `LiveCallPermit` stayed live — its `CompletionDelivery` was registered on Wasmtime's terminal observer, but the same async-lowered host function made another accessor call before returning to the guest, so the observer could never fire and `safe_to_suspend` saw `live_host_calls>0` forever. Fix (per oracle TU-033spYyUN2iK1XCBh6rSBB): new `Accessor::clear_terminal_observer` in Wasmtime (`crates/wasmtime/src/runtime/component/concurrent.rs`), and `CallHandle::start_access_with_options` now calls `supersede_prior_completion_delivery` *after* the new live persisted `Start` exists (never at host-call entry, which would drop the replay barrier early). 4 new Wasmtime tests (`terminal_observer_cleared_before_terminal`, `_clear_without_observer`, `_clear_without_host_task`, `_replacement_after_clear`); 13/13 terminal-observer tests pass (`/tmp/t42-wasmtime-terminal2.txt`), full component-async suite 98/98 (`/tmp/t42-wasmtime-full3.txt`), both sleep tests pass cleanly (`/tmp/t42-sleep-clean.txt`), discard-marker regression still passes (`/tmp/t42-gated-discard-postfix.txt`) | done |
| T42.5 | Fresh full verification over the final tree: `cargo make fix`, executor lib suite, all 43 `http_tests`-tagged tests, `cargo make worker-executor-tests`, `cargo make integration-tests`, CLI integration tests, debugging-service tests; logs under `/tmp/t42-*` — all green, see T42.5a–T42.5g below | done |
| T42.5a | Re-run the 8 visible failures from the killed first sweep (`/tmp/t42-wet-full-run1.txt`): 2 sleep tests (fixed by T42.4a) + 6 `agent_sdk_ts` tests. Sleep tests pass (`/tmp/t42-sleep-clean.txt`); the 6 `agent_sdk_ts` tests fail in isolation too (`/tmp/t42-agent-sdk-ts-rerun.txt`) with `Cannot end atomic region N: durable calls initiated in it are still in flight` — root-caused and fixed via the transferable atomic-region retry lease + replay-cursor deadlock work, see T42.5a-1..19; final battery green: 11/11 focused integration tests incl. the TS six (`/tmp/t42-focused-cleanup.txt`), executor lib 663/663 (`/tmp/t42-lib-tests-cleanup.txt`) | done |
| T42.5a-1 | Diagnose the 6 `agent_sdk_ts` failures. Root cause: the TS `atomically` pattern `setTimeout` → `fetch` → `clearTimeout` leaves the timer's `monotonic-clock::wait-for` durable call genuinely in flight when `mark-end-operation` runs (the JS-side `clearTimeout` defers the wasm subtask drop to the guest event loop, which cannot run while the `&mut`-store host call executes), so the strict in-flight guard added for atomic-region legality rejects an idiomatic, previously-working guest pattern deterministically | done |
| T42.5a-2 | Oracle architecture consultation on the fix. Verdict: constrained relaxation — atomic-region ownership becomes a per-call *transferable retry lease*. On region close, still-pending member calls transfer to the enclosing open atomic region if one exists; at outermost close they detach only if `can_reexecute_on_incomplete_replay()` (reads, local writes, idempotent remote writes), otherwise the close is still rejected. Detached calls retry from their own scope fallback (`retry_from`), never across the committed `EndAtomicRegion`; late terminals with no owner are expected and mark no side effects. Replay of `EndAtomicRegion` performs the same ownership transition. Identity-based terminal resolution, jump/persistence-level guards, and non-repairable protection all preserved | done |
| T42.5a-3 | Implement `AtomicRegionLease` (current-owner cell + `repairable_when_incomplete`) and member registry (`Vec<Weak<AtomicRegionLease>>` replacing `in_flight_call_count`) in `durable_host/mod.rs`: `register_atomic_region_call` returns the lease; add `atomic_region_surviving_members`, `atomic_region_has_parent`, `close_atomic_region` (transfer to parent / detach + side-effect bit propagation). 18/18 atomic-region unit tests pass (`/tmp/t42-lease-unit.txt`) | done |
| T42.5a-4 | Rewire `durable_host/concurrent.rs` onto leases: `CallExecutionScope.atomic_lease` (owner read dynamically for trap classification / `ScopedRetryHost`), `DroppedCall` carries the execution scope and derives `trap_context()` at read time, `AccessTerminalGuard`/`DropEvent::CleanupAfterTerminal`/`CleanupAtomicRegion` carry the lease, all terminal/cancel/drain paths release the lease (idempotent, store-free) instead of `unregister_atomic_region_call`; register replay handles as members so replay performs the same transitions. Executor lib suite 661/661 (`/tmp/t42-lib-tests-lease3.txt`) | done |
| T42.5a-5 | New `mark_end_operation` close semantics in `golem/v1x.rs`: live — drain queued drop events, reject only when the *outermost* close has non-repairable surviving members, append `EndAtomicRegion`, then `close_atomic_region` (transfer/detach); replay — consume the marker and run the same `close_atomic_region` transition (legacy oplogs with non-repairable members tolerated) | done |
| T42.5a-6 | Retry-point stability in `try_trigger_host_trap_retry`: re-read `current_retry_point()` after the async policy/state lookups and re-evaluate if a concurrent region close changed the owner, so a `SemanticTrapRetryOverride` can never embed a stale (closed) atomic begin index | done |
| T42.5a-7 | Remove all TEMPDBG instrumentation from `durable_host/mod.rs`, `golem/v1x.rs`, `concurrent.rs`; `cargo check -p golem-worker-executor` + executor lib tests. `rg TEMPDBG` over executor, common, and Wasmtime sources returns nothing | done |
| T42.5a-8 | Re-run the 6 `agent_sdk_ts` tests, the 2 sleep tests, and the T42.3 discard regression on the fixed tree. All six `agent_sdk_ts` tests pass in one run (`/tmp/t42-ts-six.txt`, 6/6); both sleep tests + the discard regression pass on the final tree (`/tmp/t42-sleep-discard-final.txt`, 3/3) | done |
| T42.5a-9 | Fix `ts_manifest_status_retry_ok_then_crash_is_bounded` (4 physical `/crash` requests with `max_attempts: 3`): the synthesized default policy `From<RetryConfig> for RetryPolicy` mapped `max_attempts` straight into `CountBox::max_retries`, but legacy `get_delay` counts *total attempts* (gives up at `attempts >= max_attempts`) while `CountBox` counts *retries* (steps after each failure from zero) — one extra physical attempt everywhere the synthesized default applied. Fixed in `golem-common/src/model/retry_policy/mod.rs` (`max_retries: max_attempts.saturating_sub(1)`); golem-common retry tests pass | done |
| T42.5a-10 | Fix `ts_manifest_status_retry_post_with_json_body` (resent POST body parsed as malformed requests on poisoned keep-alive connections): resend attempts loaded each recorded body frame from the oplog asynchronously, so hyper flushed the request head first and the body bytes landed after the chaos server had already answered the head with 500 without draining — the leftover bytes were parsed as new malformed requests. Added a bounded (256 KiB) in-memory resend cache to `DurableRequestBodyState` (`request_body.rs`): frames within budget are served synchronously by resend views so head+body go out in one flush; larger bodies fall back to oplog loads. Consistent with the live path, which already hands frames to hyper before their oplog append completes (bounded by the recording window) | done |
| T42.5a-11 | Revocation epochs for abandoned request-body views (`request_body.rs`): `abandon_active_live_view` bumps `revoke_epoch` and wakes parked views; a view created before the bump fails its next poll and reports `is_end_stream() == false`, so the abandoned attempt's hyper task aborts its body write instead of streaming leftover frames onto the poisoned connection or stealing live guest frames from the next attempt | done |
| T42.5a-12 | Atomic-region status-retry escalation in `p3/http/send.rs`: a send whose lease is still owned by an open atomic region is not inline-retry eligible, but a matching user-defined status-code policy must still be honoured — poison the pooled connection and escalate via `try_trigger_retry_access` (trap+replay keyed on the owning region), exposing the rejected response to the guest only when the policy gives up; mirrors the p2 `try_status_code_retry` atomic-region fallback | done |
| T42.5a-13 | Diagnose the `ts_cancel_survives_executor_restart` replay hang. Root cause (proven via staged TEMPDBG instrumentation, `/tmp/t42-cancel-fix10.txt`): a store-polled p3 accessor future (the wasm-rpc replay awaiter) holds the replay-cursor lock inside `drain_awaited_terminals` while awaiting an oplog-actor reply — observing that reply requires the Wasmtime event loop to poll it — and a concurrent p2 `&mut self` host call (`wall_clock::now` → `begin_durable_function` → `take_new_replay_events`) awaits the same cursor lock while holding exclusive store access, blocking the event loop. Mutual starvation deadlock. Oracle consultation produced design "A+": no store-polled future may even *queue* on the cursor mutex (tokio fair-mutex handoff makes queued waiters owners); finite cursor transactions must run on independently-scheduled tasks | done |
| T42.5a-14 | Fix part 1: move `pending_replay_events` out of `CursorState` into its own `std::sync::Mutex<Vec<ReplayEvent>>` on `ReplayCursor` (precedent: `log_hashes`), making `take_new_replay_events` synchronous and removing it from cursor-lock contention entirely (also removes the spawned-result loss window for update/fork/replay-finished events) | done |
| T42.5a-15 | Fix part 2: `run_owned_cursor_op` helper on `ReplayState` (clone + `tokio::spawn` + JoinHandle await, detached-completion semantics — cursor transactions always run to `finish_tx` even if the awaiting accessor future is cancelled; dropping the JoinHandle detaches, never aborts); route through owned tasks every cursor-lock user reachable from accessor futures: `drain_awaited_terminals`, the final is-live receiver recheck in `await_resolution_outcome` (owns the receiver; every branch terminal), `unregister_awaiter`, all `claim_concurrent_start*` variants (inputs cloned to `'static`), `is_in_skipped_region` (now returns `Result<bool>` — it guards jump validity in `golem/v1x.rs`, so a failed cursor read propagates instead of defaulting to "not skipped"), `switch_to_live`, and the cursor-state snapshot in `lookup_oplog_entry_with_condition_and_state` (scan itself stays lock-free; join failure returns the conservative `NotFound { violates_for_all: true }`). Audit confirmed the direct cursor users that remain are only P2 `&mut self` host calls (span markers, rdbms, oplog API, retry API, atomic regions) and invocation-loop paths (`new`, `drop_override_and_restart` in snapshot recovery, `set_replay_target`, `get_oplog_entry_at_invocation_boundary`, `get_oplog_entry_agent_invocation_started`) — none reachable from store-polled accessor futures; `seen_log`/`remove_seen_log` use their own sync mutex | done |
| T42.5a-16 | Fix part 3: owned variants of the positional readers (`get_oplog_entry_owned` / `try_get_oplog_entry_owned` + `get_oplog_entry_owned!` macro) for the p3 accessor call sites that consume legacy `StartSpan`/`FinishSpan` markers in `durable_host/concurrent.rs` and the replayed `FinishSpan` in `wasm_rpc/mod.rs`; invocation-loop wrappers stay direct | done |
| T42.5a-17 | Verification of the deadlock fix: `ts_cancel_survives_executor_restart` passes (`/tmp/t42-cancel-fix11.txt`, 7.6s), `ts_cancel_unblocks_caller_while_callee_blocked` passes (`/tmp/t42-cancel-companion1.txt`, 7.4s); 2 new unit tests for detached-completion cleanup (`dropped_awaiter_terminal_drains_without_residue`, `dropped_scan_ahead_claim_leaves_no_residue_once_cursor_passes` — a dropped awaiter/claim leaves no resolver or `claimed_starts` residue once the cursor passes, and doesn't steal entries from later positional readers); full replay_state module 53/53 (`/tmp/t42-replay-unit-new.txt`); post-cleanup rerun of both ts_cancel + sleep/discard + TS-six tracked below | done |
| T42.5a-18 | Remove all remaining TEMPDBG instrumentation (`lock_owner`, `lock_state_dbg`, `tx_dbg`, read-path logs in replay_state.rs / multilayer.rs / primary.rs / concurrent.rs / durability.rs / wasm_rpc / mod.rs); `rg 'TEMPDBG|lock_owner|lock_state_dbg|tx_dbg'` over executor sources returns nothing; clean `cargo check --all-targets` (`/tmp/t42-check-cleanup.txt`, the `ReplayCursor::tx` dead-code warning is gone), executor lib suite 663/663 on the cleaned tree (`/tmp/t42-lib-tests-cleanup.txt`); focused integration battery on the cleaned tree 11/11 (both ts_cancel, sleep ×2, discard, TS-six): `/tmp/t42-focused-cleanup.txt` | done |
| T42.5a-19 | Architecture review of the resend cache (T42.5a-10) against persist-before-transfer: the cache changes only *when* resent bytes hit the wire, never any persistence ordering. (a) Serving a cached frame whose oplog append is still in flight matches the existing live path, which hands each pulled frame to hyper immediately after spawning its append (bounded by the 4-frame recording window) — body bytes on the wire are a live external effect that was never gated on frame persistence. (b) Crash windows are unchanged: with the send's `End` persisted, replay is `Replayed` — the request is consumed and recorded frames are never read, so a lost late frame append is harmless; without the `End`, replay is `Incomplete` and re-issues via `replayer()` from the persisted frame prefix + live guest body, exactly as pre-cache (frames are keyed to `parent_start_index` and matched by offset, so log position relative to `End` is irrelevant). (c) A frame-append failure sets `recording_failed`, which every view poll checks *before* serving cached slots and which makes `drain_to_terminal` return `NotReplayable`, so a resend cannot outrun a failed recording; a failure landing after a fully-cached resend completed leaves only the End-persisted case from (b). (d) The 256 KiB budget is a bounded-memory heuristic, not a correctness boundary: over-budget bodies fall back to async oplog loads, which are HTTP-correct but keep the two-flush timing — only pathological servers that answer early without draining the request body (the chaos fixture) care, accepted and documented. Decision: keep the implementation as-is | done |
| T42.5b | Re-run the 9 exact-name unfinished candidates from the killed sweep: `ignite_connection_test`, `del_many_ns1_in_memory`, `counter_resource_test_2` (+ `_with_restart` matched by the filter), 3 parallel-HTTP oplog-replay tests, `sleep_and_awaiting_parallel_responses` — 8/8 pass on the final tree (`/tmp/t42-5b-candidates2.txt`); the 2 RPC ts_cancel tests re-ran in the post-cleanup battery (`/tmp/t42-focused-cleanup.txt`) | done |
| T42.5c | All tests consuming the `http_tests` component (`#[tagged_as("http_tests")]` dependency — a dependency tag, not a test-r test tag, so there is no `:tag:http_tests` selector; the set has grown from 43 to 64 with the T42 regressions): enumerated by signature scan and run by exact name — 65/65 pass (one extra prefix match) on the final tree in 141s (`/tmp/t42-5c-http-tests.txt`, names in `/tmp/t42-http-names.txt`) | done |
| T42.5d | Fresh full `cargo make worker-executor-tests` on the final tree: 742 passed, 0 failed, 4 ignored (746 total), cargo-make total 277.8s, no hangs (`/tmp/t42-5d-wet-full.txt`) | done |
| T42.5e | Fresh `cargo make integration-tests` — all groups green on the final tree (see T42.5e-1..19): groups 1–5 full run (`/tmp/t42-5e-integration-full7.txt`), sharding scenarios 01/02 focused rerun (`/tmp/t42-sharding-0102-rerun.txt`), groups 7–13 (`/tmp/t42-5e-groups7-13.txt`, `/tmp/t42-5e-sharding-rerun1.txt`), OTLP 2/2 (`/tmp/t42-otlp-focused2.txt`); plus the full parallel CLI integration suite green (T42.5e-18) | done |
| T42.5e-1 | Test-framework infra fix surfaced by the first integration attempt: `resolve_cargo_target_dir_uncached` (`golem-test-framework/src/config/env.rs`) combined a relative `repo_root` with both a relative `--manifest-path` and `.current_dir(repo_root)`, applying `..` twice, so `cargo metadata` failed and the framework fell back to `../target` — wrong under a redirected cargo target dir. Fixed by canonicalizing the manifest path and dropping `current_dir`; verified via focused probe (`/tmp/t42-5e-probe3.txt`, services spawn from the redirected target dir) | done |
| T42.5e-2 | Full run 2 (`/tmp/t42-5e-integration-full2.txt`): 34/38 passed in group 1, 4 deterministic failures: `fork_self`, `fork_running_worker_2`, `fork_interrupted_worker` (all: `Unexpected oplog entry during replay: expected Start { <scope:batched-write:consume-body:...> }, got no matching Start`), and `worker_suspends_when_running_out_of_fuel` (child agent creation fails with `Failed to instantiate worker ...: Suspended`) | done |
| T42.5e-3 | Fix the fork replay failures. Root cause: the consume-body scope discriminator embedded the p3 send's derived span id, and `derive_p3_send_span_id` hashes `owned_agent_id` + start index — deterministic across restart/replay of the *same* worker but not across **fork/revert to a different agent id**, so the fork target derived a different scope name than the one recorded in the copied oplog prefix and the identity claim found no matching `Start`. Fix: `P3HttpSendSpan` now carries `send_start_index` (the send's claimed/live `Start` index — recorded oplog state, preserved verbatim by fork) and the consume-body scope is discriminated by it (`consume-body:{send_start_index}`) instead of the span id (`send.rs`, `response_body.rs`). Span-id derivation itself is unchanged (trace uniqueness still wants the agent id); only the durable claim identity moved onto fork-stable state. Sockets audited: no span-based discriminators | done |
| T42.5e-4 | Fix `worker_suspends_when_running_out_of_fuel`. Wasm executes during component instantiation, so with the 1-gas low-fuel plan the first-tick fuel borrow fails and the epoch callback traps with `InterruptKind::Suspend` *inside* `instantiate_async`; `RunningWorker::create_instance` stringified the trap into `WorkerCreationFailed`, failing agent creation instead of suspending. Fix: `create_instance` maps an `InterruptKind` root cause to `WorkerExecutorError::Interrupted`, and the invocation loop handles instantiation interrupts as lifecycle events (new `CreateInstanceResult` enum): Restart/Jump → recreate; Suspend → append `OplogEntry::suspend()`, honor a later resume request (TryStop semantics), release creation waiters with `WorkerLoaded Ok`, stop unloaded; Interrupt → append `OplogEntry::interrupted()`, stop unloaded (`worker/mod.rs`, `worker/invocation_loop.rs`) | done |
| T42.5e-5 | Focused reruns on the fixed tree: all 10 `integration::fork::*` tests pass (`/tmp/t42-5e-fork2.txt`, 51.5s) and `worker_suspends_when_running_out_of_fuel` passes with fresh services (`/tmp/t42-5e-fuel2.txt`). Fork-discriminator regression coverage = the fork suite itself (deterministic repro pre-fix, green post-fix) | done |
| T42.5e-6 | Full run 3 (`/tmp/t42-5e-integration-full3.txt`): group 1 now 38/38 passed (158.8s), but the suite aborted on a pre-existing (committed, not P3) Makefile bug: `integration-tests-group1`/`group3` invoke `--test environment_deletion` while `integration-tests/Cargo.toml` names the target `environment-deletion`; without a nextest archive cargo-test-r forwards to plain `cargo test`, which requires the exact name. Fixed both occurrences in `Makefile.toml` | done |
| T42.5e-7 | Full run 4 (`/tmp/t42-5e-integration-full4.txt`): group 1 37/38, `fork_running_worker_1` flaked with `left: 2, right: 1` at fork.rs:179. Diagnosis: **test bug, not a replay regression** — oplog search is case-insensitive substring matching (`golem-common/src/model/oplog/matcher.rs`), idempotency keys are random hex UUIDs, and `"add"` is a valid hex substring (~0.7%/UUID), so the target's constructor entry (`AgentInitialization`, always matches `invoke`, not `pending`) spuriously matched `add` via its idempotency key. Replay-mode invocations never append a second `AgentInvocationStarted` (only Live mode calls `on_agent_invocation_started`), so genuine duplication is impossible by construction. 6/6 isolated reruns green (`/tmp/t42-5e-forkrw1-{1..6}.txt`). Fixed the query to `add AND agent-method-invocation AND NOT pending` (hex-proof); other `search_oplog` queries audited — `G1001`/`G1002` contain non-hex `G`, `"Received first"` is non-hex, fork.rs:452 doesn't assert counts. Fixed test verified green (`/tmp/t42-5e-forkrw1-fixed.txt`) | done |
| T42.5e-8 | Full run 5 complete (`/tmp/t42-5e-integration-full5.txt`): **699 passed, 5 failed** — groups 1–5 all green (incl. group 1 38/38 with both fixes and the fixed fork_running_worker_1); the only failures were the 5 sharding tests (group 6), all root-caused: 3 coordinated-scenario contention timeouts (T42.5e-9) + 2 stale-wasm oplog-processor failures (T42.5e-10). After the mid-run wasm fix, the remaining sharding oplog tests passed in-run (`oplog_processor_stress_with_crashes` 70.8s, `service_is_responsive_to_shard_changes` 43.5s, `oplog_processor_shard_reassignment_no_loss` on retry 3 — its attempt-1 duplicate-delivery failure matches the test's own "Current bug: no checkpoint, so shard reassignment causes re-delivery" comment + `#[flaky(3)]`). cargo-make aborted at group 6, so groups 7–9, 12–13 and cli-integration-tests still need a run (T42.5e-11/12) | done |
| T42.5e-9 | Sharding chaos failures analyzed (run 5): `coordinated_scenario_01_02` FAILED (2 attempts), `coordinated_scenario_02_01` FAILED (5/5 attempts). Root-cause evidence **against** a P3 hang: (1) `01_02` attempt 2 was actively progressing (pending steadily dropping, reached 2) when killed by the test's own `#[timeout(240000)]` — test-r does not flaky-retry a timeout, hence FAILED after only 2 attempts; (2) `02_01` attempt 5's two "stranded" invocations (`sharding-test-2`, `-7`) **completed successfully at 01:13:10** as leftover tasks ~38s after the 240s test timeout killed the attempt — invocations are neither lost nor deadlocked, a 1–2 worker straggler pair simply took ~2m23s (vs ~14s for the other 8) to recover after `StopAll → RestartSM → Start(4) → RestartSM`; (3) each attempt stranded a *different* worker (test-9, {test-2,test-9}, test-4, {test-2,test-7}) — timing race, not a corrupt worker/oplog; (4) the whole run was contended: an unrelated external `cargo test -p golem-worker-executor` build (phase5-target-admission, separate target dir on the same /Volumes/X1 disk) ran concurrently, log saturated with sqlx slow-statement/slow-acquire warnings (3–5s single-row INSERTs). Both tests carry baseline `#[flaky(5)]` annotations (known-flaky upstream). Resolution: the quiet-machine rerun (T42.5e-11) passed all coordinated scenarios within retry budget (01_02 and 02_01, which had exhausted attempts under contention, both passed on attempt 2) → classified as load-amplified pre-existing flake, not a P3 regression | done |
| T42.5e-10 | Deterministic sharding oplog-processor failures root-caused and fixed: `component imports instance golem:api/oplog@1.5.0, but a matching implementation was not found in the linker` (118 occurrences in run 5, first at the sharding `oplog_processor_*` tests — the first tests in the run to instantiate an oplog-processor plugin worker). Cause: the P3 work added the `completion-discarded` case to `oplog-entry`/`public-oplog-entry` in `wit/deps/golem-1.x/golem-oplog.wit` (and the synced SDK copy) on Jul 19 21:41 **without a version bump**, while `test-components/oplog_processor_release.wasm` was last built Jul 18 11:17 against the old shape; its `enrich-oplog-entries` import references `oplog-entry`, so wasmtime's structural typecheck rejects the instance import. The wet suite passed with the equally-stale `golem_it_host_api_tests_release.wasm` (Jul 19 19:30) because that component's pruned oplog import only carries the `wrapped-function-type` type — no functions referencing the changed variants. Fix: rebuilt oplog-processor via `golem build -P release --force-build --yes` (`/tmp/t42-oplog-proc-rebuild.txt`, OK) and copied the artifact to `test-components/oplog_processor_release.wasm` (now `completion-discarded`×4). Audit of all root-level test wasms importing `golem:api/oplog`: `golem_it_agent_promise/rpc/sdk_ts/constructor_parameter_echo` already new-shape; `golem_it_host_api_tests_release` stale-but-unaffected (type-only import); `benchmark_agent_ts` stale **and** affected but benchmark-only (target has `test = false`) — falls under the explicitly deferred unsupported benchmark fixtures | done |
| T42.5e-11 | Focused sharding rerun on quiet machine (external phase5 build finished) with rebuilt oplog-processor wasm (`/tmp/t42-5e-sharding-rerun1.txt`): **all 9 sharding tests passed** (`test result: ok; 9 passed; 0 failed`, 1017s, zero `matching implementation` errors). Both previously stale-wasm failures passed on first attempt (`oplog_processor_locality_recovery` 26.2s, `oplog_processor_shard_move_inflight` 29.3s) — confirms T42.5e-10 root cause + fix (run 5's mid-run file replacement couldn't help because those tests' components were already uploaded to the registry with stale bytes; the rerun's fresh registry re-uploaded from disk). `coordinated_scenario_03_01` passed attempt 1 (54.7s); `01_01`/`01_02`/`02_01` each passed on attempt 2 (257–277s) after an attempt-1 `sharding.rs:551` invocation-wait timeout with 1–3 straggler workers — same signature as run 5, different workers each time, well within the baseline `#[flaky(5)]` budget | done |
| T42.5e-12 | Full fresh integration coverage achieved across full7 + follow-ups: **(a)** full7 (`/tmp/t42-5e-integration-full7.txt`, launched after the T42.5e-13 registry rebuild): groups 1–5 all green (696+ tests incl. 198/79/28/18 group results, zero failures), sharding group 8/9 — only `coordinated_scenario_01_02` failed (exhausted 5 flaky attempts; root failure each attempt is the known `sharding.rs:551` invocation-wait timeout with 1–3 *different* straggler workers per attempt, the reported `Option::unwrap() on None` panics at sharding.rs:360/844 are post-panic channel-close cascades; attempt-5 stragglers `sharding-test-9`/`-10` **completed successfully** right after the timeout — invocations delayed, not lost — same pre-existing chaos-flake signature as T42.5e-9), cargo-make aborted at group 6; **(b)** remaining groups run via their exact cargo-make tasks (`/tmp/t42-5e-groups7-13.txt`): group7 **15/15** (incl. both otlp_plugin tests through the full registry-provisioning flow), group8 **9/9**, group9 **9/9** (sqlite), group12 **36/36**, group13 **36/36** (sqlite) — zero failures; **(c)** focused `coordinated_scenario_01_02` rerun with group-6 env (`/tmp/t42-sharding-0102-rerun.txt`): **PASSED** (961s, within `#[flaky(5)]` budget). Net: every test in the `integration-tests` aggregate passed on this exact tree; the single flake is the documented pre-existing chaos straggler | done |
| T42.5e-13 | Built-in OTLP exporter plugin failures in full6 (`integration::otlp_plugin::{otlp_basic_trace_export,otlp_all_signals_export}` — `Component trapped: failed to load oplog-processor export: failed to convert function to given type`) root-caused: same stale-ABI class as T42.5e-10 but with an extra indirection — `plugins/otlp-exporter.wasm` is **embedded at compile time** into golem-registry-service via `include_bytes!` (`golem-registry-service/src/services/builtin_plugin_provisioner.rs`), and the `GOLEM__BUILTIN_PLUGINS__OTLP_EXPORTER_WASM_PATH` env var set by the test framework is dead config (`BuiltinPluginsConfig` is `Enabled(Empty)\|Disabled(Empty)` — no path field), so rebuilding+copying the plugin wasm alone (`/tmp/t42-otlp-plugin-rebuild.txt`, new wasm `completion-discarded`×2) cannot take effect until **golem-registry-service is rebuilt** to embed the fresh bytes. Fix workflow: `golem build -P release` + `golem exec -P release copy` in `plugins/otlp-exporter`, then `cargo build -p golem-registry-service` (cargo tracks `include_bytes!` inputs in dep-info, so the rebuild picks up the new wasm), then rerun focused otlp_plugin tests. Executed: registry-service rebuilt (`/tmp/t42-registry-rebuild.txt`, 3m56s OK) and focused rerun passed (`/tmp/t42-otlp-focused2.txt`: `otlp_basic_trace_export` + `otlp_all_signals_export` **2 passed, 0 failed**, 11.2s) | done |
| T42.5e-14 | CLI suite (run 1 `/tmp/t42-cli-integration.txt`: 20/28/2, run 2 after target-dir fix `/tmp/t42-cli-integration2.txt`: 40/8/2). Fixes landed: **(a)** `cli/golem-cli/tests/app/mod.rs` `cargo_target_dir()` no longer assumes workspace `target/` — derives the redirected target root from `current_exe()`; **(b)** stale Rust all-types fixture `cli/golem-cli/test-data/rust-code-first-snippets/lib.rs` (`wasip2::clocks::wall_clock::Datetime` → `golem_rust::ScheduledTime`) — `test_rust_code_first_with_rpc_and_all_types` passed serially (`/tmp/t42-cli-rerun-serial.txt`); **(c)** `test_ts_counter`/`test_rust_counter`/`build_and_deploy_all_templates_for_ts` root-caused as load/contention startup flakes — all passed serially (`/tmp/t42-cli-rerun-serial.txt`) | done |
| T42.5e-15 | `ts_repl_interactive` completion_done timeout root-caused: the parent environment exports `TERM=dumb`, which disables readline tab completion inside the PTY (proved with minimal Node 24 PTY reproductions `/tmp/node-v24-*.js`, `/tmp/t42-repl-tab-test*.py` — Node's built-in REPL also fails under `TERM=dumb`, works under `xterm-256color`). Fix: `TestContext::cli_interactive` (`cli/golem-cli/tests/app/mod.rs`) now sets `TERM=xterm-256color` on the interactive command. Focused rerun **PASSED** (95.5s, `/tmp/t42-ts-repl-fix1.txt`) | done |
| T42.5e-16 | MoonBit CLI failures (`build_mixed_language_app`, `build_and_deploy_all_templates_for_moonbit`, `moonbit_single_to_multi_component_upgrade_builds`) root-caused as a **local moon toolchain ICE, not a P3 regression**: locally installed moonc v0.10.2+1bb3e16cf (2026-06-29) crashes with `output_value: integer cannot be read back on 32-bit platform` compiling the (unchanged, committed) `gen/interface/golem/agent/guest` package of the MoonBit SDK, in both debug and release (`/tmp/t42-moonbit-ice1.txt`, `/tmp/t42-moonbit-release1.txt`); CI pins `MOONBIT_INSTALL_VERSION: 0.9.2+bbe2b338f` and never saw 0.10.2. moonc v0.10.4+2cc641edf (`latest`, installed isolated under `/tmp/t42-moon-latest` with its own `MOON_HOME`) builds the SDK cleanly — 0 errors (`/tmp/t42-moonbit-latest-build.txt`). Tests rerun with the isolated toolchain on PATH; pinned CDN downloads for 0.9.2 are gone (403), so the global `~/.moon` was left untouched — recommend `moon upgrade` to ≥0.10.4. **Follow-up fix landed**: the committed MoonBit host bindings (`interface/golem/agent/host/top.mbt`) take `@systemClock.Instant` for scheduled invocations, but `rpc/rpc.mbt` and the `golem_sdk_tools` client emitter still used the stale `@wallClock.Datetime` — migrated `rpc/rpc.mbt` + `rpc/moon.pkg` to `@systemClock.Instant`, updated `clients_emit.mbt` + snapshots, and switched the `wallClock` import to `systemClock` in the 4 MoonBit CLI templates and `golem_sdk_example1` `moon.pkg` (genuine wall-clock uses in `quota`/`context` untouched). Tools: `moon check` 0 errors + 154/154 tests (`/tmp/t42-tools-check3.txt`, `/tmp/t42-tools-test1.txt`); SDK builds (`/tmp/t42-moonbit-sched-build.txt`). Focused CLI: `build_and_deploy_all_templates_for_moonbit` + `moonbit_single_to_multi_component_upgrade_builds` **PASSED** (`/tmp/t42-cli-moonbit-fix3.txt`). Note: latest moon core renamed `@priority_queue.T` → `PriorityQueue`; the downloaded `Yoorkin/prettyprinter` dep needed a local gitignored `.mooncakes` patch — **superseded by a tracked fix**: `golem_sdk_tools/moon.mod.json` now adds a direct dep `"Yoorkin/prettyprinter": "0.4.9"` (formatter 0.1.5/0.1.6 still pin 0.4.8; upstream 0.4.9 contains exactly the `PriorityQueue` rename fix, commit `84a14c1a`; the name is compatible with CI's pinned 0.9.2 core too — core has exposed both names since 2025-11-27 `82c6c7ab`, `T` removed only 2026-07-06 `e041eaf0`). Hand-patched `.mooncakes` copy moved aside to `/tmp/t42-prettyprinter-patched-backup`, clean 0.4.9 fetched; `moon check` 0 errors + 154/154 tests with the tracked dep (`/tmp/t42-tools-check4.txt`, `/tmp/t42-tools-test2.txt`) | done |
| T42.5e-17 | MoonBit SDK binding regeneration for the uncommitted `golem-oplog.wit` sync (adds `completion-discarded`) is **blocked upstream**: stock `wit-bindgen` 0.57.0/0.58.0/0.59.0 all panic with `assertion failed: !bindgen.needs_cleanup_list` (`wit-bindgen-moonbit` `export()`, explicit `TODO: adapt async cleanup`) because the t15–t36 WIT syncs made the exported guest/snapshot functions `async func` (`initialize`/`invoke` in `golem-agent/guest.wit`, `save`/`load` in `golem-host.wit`) — the MoonBit backend cannot yet emit async exports needing cleanup lists (verified: 2a1c09f1e's WIT regenerates fine, HEAD's does not; the trigger is the async-export change, not the oplog variant). Impact contained: async is a lift/lower option, not part of the component function type, so the existing sync-lifted bindings still satisfy the async WIT world, and unused stale oplog imports are DCE'd out of built components. The full MoonBit SDK regen belongs to the SDK-migration follow-up (G33 scope), needs an upstream wit-bindgen moonbit fix or fork | done |
| T42.5e-19 | Scala guest base image stale → `build_mixed_language_app` fails at deployment prep: `discover-agent-types` typed-func check rejects the scala component (`type mismatch with results`, `/tmp/t42-cli-mixed-fix3.txt`) because the gitignored `agent_guest.wasm` (sbt/mill plugin resources, built Jun 15) predates the t15–t36 WIT syncs, and `scripts/generate-agent-guest-wasm.sh` could no longer regenerate it: the WIT world now has async exports (`save`/`load`), which the default (p2) wasm-rquickjs path rejects (`Async exported functions are not supported yet`, `/tmp/t42-scala-agent-guest-regen.txt`) — the script was never migrated to the wasi-p3 generation path the TS SDK already uses. Fix: `sdks/scala/scripts/generate-agent-guest-wasm.sh` now passes `--target wasi-p3` to `generate-dts`/`generate-wrapper-crate` and builds with `--no-default-features --features full-p3,golem` (mirrors `golem-ts-sdk` `compile-agent-template`); regen succeeded with the wasi-p3 wasm-rquickjs build (locally reinstalled isolated to `/tmp/t42-wasm-rquickjs` — the global `~/.cargo/bin/wasm-rquickjs` had been replaced by a crates.io 0.3.6 build lacking `--target`; CI pins 0.3.4 via cargo-binstall) (`/tmp/t42-scala-agent-guest-regen4.txt`), plugins republished via `sbt golemPublishLocal` (`/tmp/t42-scala-publish-local.txt`), mixed-language test rerun pending. **Rerun with regenerated guest** (`/tmp/t42-cli-mixed-fix5.txt`) got past the old signature mismatch but exposed a second, real SDK bug: the fresh p3 base image enforces that sync WIT exports return values directly, and `Guest.scala` returned `js.Promise` from the sync `get-definition`/`discover-agent-types` exports → guest panic `The synchronous exported function guest.discoverAgentTypes returned a Promise` (`src/internal/p3.rs:721`). Fix: `sdks/scala/core/js/src/main/scala/golem/runtime/guest/Guest.scala` `getDefinition`/`discoverAgentTypes` made synchronous (return directly, throw `JsAgentError` via `js.JavaScriptException` for the error arm; `initialize`/`invoke` stay Promise-based per their `async func` WIT declarations); SDK republished (`/tmp/t42-scala-publish-local2.txt`; the pre-existing `golem-scala-codegen` cross-version-suffix error only aborted the `sbtPlugin/update` leg — the fixed `golem-scala-core_sjs1_2.13`/`_sjs1_3` jars were published). Mixed-language rerun **PASSED** (290.3s, `/tmp/t42-cli-mixed-fix6.txt`) — all four components (MoonBit/Rust/Scala/TS) built, agent types extracted, deployment finished OK. **CI/tool reproducibility resolved as pre-existing, not introduced by this change**: `--target wasi-p3` is already the committed pattern at HEAD — the TS SDK's `generate-agent-template.mjs` passes `--target wasi-p3` since commit `12aa27d99` ("TS SDK migration"), the workspace `Cargo.toml` at HEAD path-depends on the sibling `../wasm-rquickjs` checkout (`wasm-rquickjs = { path = "../wasm-rquickjs/..." }`), and `sdks/ts/AGENTS.md` prescribes installing wasm-rquickjs from the local `wasi-p3` branch checkout. No published `wasm-rquickjs-cli` (crates.io latest 0.3.6, 2026-06-25) supports `--target`; CI's `WASM_RQUICKJS_VERSION: "0.3.4"` binstall pin is therefore already stale for the committed TS `build-agent-template` CI step (ci.yaml:669) exactly as it is for the Scala `generate-agent-guest-wasm.sh` step (ci.yaml:347) — both need an upstream wasm-rquickjs release off the `wasi-p3` branch (commit `d2b7119c`) and a CI pin bump, which belongs to the SDK-migration/publication follow-up (G33 scope), not T42 | done |
| T42.5e-18 | Full parallel CLI suite rerun (`cargo make cli-integration-tests`, `/tmp/t42-cli-integration3.txt`): **45 passed, 3 failed, 2 ignored** (2173s) — every previously-failing substantive test (MoonBit templates/upgrade, mixed-language, TS REPL interactive, Rust all-types/RPC, TS/Rust counters in most instances) passed **in parallel**; the only 3 failures (`test_rust_counter`, `test_ts_counter`, `ts_repl_interactive`) are all the identical infra flake `Timed out waiting for golem server startup ports file` (`mod.rs:675`, all ~12.5s) — the hard-coded 10s server-startup wait is too tight under full parallel load. Fix: startup timeout raised 10s → 60s in `cli/golem-cli/tests/app/mod.rs` (healthcheck loop still exits as soon as the server is up). Focused rerun of the 3 flaked tests with the fix **PASSED** (3 passed, 0 failed, 312.6s incl. full REPL tab-completion flow, `/tmp/t42-cli-flakes-rerun1.txt`). Net: all 48 executed CLI integration tests pass on this tree (45 in the parallel run + 3 startup-flaked ones green after the timeout fix), plus 2 ignored (never executed) | done |
| T42.5f | Debugging-service tests: full `golem-debugging-service` integration suite **PASSED** — 18 passed, 0 failed, 0 ignored, 68.6s (`RUST_LOG=debug cargo-test-r run --package golem-debugging-service --test '*'`, `/tmp/t42-5f-debugging-service.txt`), including all rewind/playback/fork debug scenarios (`test_rewind_target_inside_in_flight_durable_call_is_rejected`, `test_rewind_after_playback_unloads_worker`, etc.) | done |
| T42.5g | Final-tree validation **PASSED**: `cargo fmt --all` idempotent (`cargo fmt --all -- --check` exits 0, `/tmp/t42-5g-fmt-check.txt`); `cargo make fix` clean — full-workspace clippy 0 errors, no auto-fixes, tree unchanged (`/tmp/t42-5g-fix.txt`, 497s); `cargo check -p golem-worker-executor` passes (`/tmp/t42-5g-check.txt`); executor lib suite 663 passed, 0 failed, 55.7s (`/tmp/t42-5g-lib-tests.txt`); residue audits re-run clean — no `TODO(p3)` in `golem-worker-executor/src`/`golem-common/src`, no `TEMPDBG`/debug-diagnostic identifiers in executor or Wasmtime crates. Full parallel CLI integration run 4 (all fixes in tree, isolated moon 0.10.4 + wasi-p3 wasm-rquickjs on PATH) **PASSED**: **48 passed, 0 failed, 2 ignored**, 1382.5s (`/tmp/t42-cli-integration4.txt`) — the complete CLI suite is green in one parallel run on the final tree, including the 3 previously startup-flaked tests, MoonBit templates/upgrade, mixed-language, and TS REPL interactive | done |
| T42.6 | Documentation close-out done: `p3-gaps.md` close-out header refreshed (2026-07-21, full-suite sweep summary, G33/G35 explicitly open, wasm-rquickjs upstream-release dependency noted); `p3-durability-checklist.md` already fully `done` (16/16 rows, row #8 marked resolved by T40) — no changes needed; `p3-migration-notes.md` stale claim fixed (cross-cutting decision 1 said checklist #8 "not yet done"/blocked on a missing wasip3 HTTP harness — now records the T40 resolution and runtime verification); this file's boundary-test counts already cite 14 `invocation_boundary_*` tests (matches `replay_state.rs`, 14 `fn invocation_boundary_*`) with fresh `/tmp/t42-*` evidence logs throughout. Final tree snapshots captured for review: `/tmp/t42-final-golem-status.txt` + `/tmp/t42-final-golem.diff` (11327 lines vs `e801b49b1`), `/tmp/t42-final-wasmtime-status.txt` + `/tmp/t42-final-wasmtime.diff` (393 lines above `14927e7454` on `golem-wasmtime-v46.0.1-p3`; untracked `settle.rs`/`terminal_observer.rs` test scenarios listed in the status file) | done |
| T42.7 | Fresh oracle review of the exact final implementation + evidence; flip T42 to `done` only on an explicit APPROVED verdict. **Round 2 (2026-07-21): NOT APPROVED** — prior blockers (body redelivery, abandoned starts, deterministic coverage, atomic lease, cursor deadlock) confirmed resolved; 4 new conditions, tracked as T42.8a–T42.8e below. **Round 3 (2026-07-21): APPROVED** — no high- or medium-severity findings remain; T42.8a confirmed correct under the clean-database premise (both mismatch directions covered, wrong-scope claim impossible), T42.8b delivery boundaries verified at all three receive paths, docs/repo state coherent, final snapshots match the tree. Approval explicitly relies on the product decision that deployed P3 databases contain no pre-discriminator oplogs (no mixed-format compatibility claimed); G33/G35 remain open and do not block T42 | done |
| T42.8a | (oracle round 2, High) Discriminated scope replay can claim the wrong legacy scope: `execute_access_scope_start` passes both the exact discriminated name and the plain legacy name in one accepted list, and `claim_start_matching` claims the *first* oplog-order match — a mixed oplog (old-executor prefix + new-executor live tail) can let the discriminated call steal an earlier unclaimed legacy scope. **Resolved with a deliberate deviation from the suggested fix**: per the deployment decision that P3 ships against a **clean database** (no pre-discriminator oplogs can exist), the legacy plain-name fallback was **removed entirely** instead of adding exact-first + fallback ordering — `execute_access_scope_start` (`durable_host/concurrent.rs`) passes only the exact discriminated name, and `claim_scope_start` (`durable_host/replay_state.rs`) takes a single exact `&HostFunctionName` and matches by equality (no accepted-list membership at all), so a discriminated call can never claim a plain legacy scope and vice versa. No mixed-format compatibility is claimed or supported. Regression tests added at the end of the `replay_state.rs` test module (incl. `discriminated_scope_claim_never_matches_plain_scope_start`); focused module run 55 passed / 0 failed (`/tmp/t42-8a-tests.txt`), `cargo check -p golem-worker-executor --lib` clean (`/tmp/t42-8a-check.txt`) | done |
| T42.8b | (oracle round 2, Medium) Wasmtime terminal observer fires before delivery actually succeeds: sync-lowered path invokes `TerminalConsumption::Delivered` in `poll_and_block` before `call_sync_lower` validates/lowers the result; callback path removes the observer in `Waitable::on_delivery` before `handle_guest_call` runs the guest callback — a lowering failure or callback trap is misreported as delivered. **Fixed**: observer notification removed from `Waitable::on_delivery` and `poll_and_block`; a new `Waitable::notify_terminal_observer` (`futures_and_streams.rs`) is invoked only after the receiving operation succeeds — after `waitable_check`'s event-payload writes to guest memory complete (`concurrent.rs`), after the guest callback returns successfully in callback dispatch (`concurrent.rs`), and after `call_sync_lower` succeeds in the sync-lowered import entrypoint (`func/host.rs`); `register_terminal_observer` docs now define the exact delivery boundaries. Three failure-path tests added to `scenario/terminal_observer.rs` asserting suppression: `terminal_observer_suppressed_on_event_payload_trap` (OOB `waitable-set.wait` payload pointer traps after event selection), `terminal_observer_suppressed_on_callback_trap` (guest callback traps processing the completion), `terminal_observer_suppressed_on_sync_lowering_trap` (trapping `realloc` fails result lowering of a successful host result). Full terminal-observer suite 16 passed / 0 failed (`/tmp/t42-8b-tests.txt`, final-tree rerun `/tmp/t42-8e-wasmtime-observer.txt`); whole component-async suite 101 passed / 0 failed (`/tmp/t42-8e-wasmtime-full.txt`) | done |
| T42.8c | (oracle round 2, Medium) Doc contradictions: `p3-gaps.md` header says "All executor-side gaps resolved" while G35 (executor-side) is open — reword to "all except G35"; `p3-migration-notes.md` has a *second* stale checklist-#8-blocked passage (~lines 263-289) superseded by T08/T40; CLI close-out wording must say 48 passed + 2 ignored, not imply all 50 executed. **Fixed all three**: `p3-gaps.md` header now says "All executor-side gaps in this document **except G35** are resolved" and cites the CLI suite as "48 passed, 0 failed, 2 ignored"; the stale follow-up item 4 in `p3-migration-notes.md` ("Required follow-up work to restore durability under p3") now records the T40 harness resolution, checklist #8 `done`, and that the request-body transmission *result* is recorded/replayed durably (`pending_p3_http_request_transmissions` + `start_transmission_recording`) — no longer claims it is unrecorded; T42.5e-18's net wording changed to "all 48 executed CLI integration tests pass … plus 2 ignored (never executed)" | done |
| T42.8d | (oracle round 2, Medium) Untracked/uncommitted review-required files: the four p3-*.md close-out docs are untracked in Golem; `settle.rs`/`terminal_observer.rs` are untracked in Wasmtime while HEAD already declares `pub mod settle` (clean checkout doesn't build the declared tests). **Committed**: Golem `c7d91a430` adds the four close-out docs (`p3-gaps.md`, `p3-gaps-tasks.md`, `p3-migration-notes.md`, `p3-durability-checklist.md`; subsequent row updates remain as working-tree edits on top); Wasmtime `1cf37878bc` adds the missing `settle.rs` (fixing the clean-checkout build), `fd94e160a5` the connection-pool rustls-in-io::Error unwrap fix, `9a3221a39a` the terminal-observer implementation + `terminal_observer.rs` test scenario (post-T42.8b delivery-boundary semantics, `cargo fmt` clean). **Audit of the flagged unrelated changes**: the `test-components/*/AGENTS.md` modifications (one added `golem-mark-read-only-rust` skill-table row each) and `test-components/*/Cargo.lock` `wasip3 0.6.0+rc → 0.7.0+wasi-0.3.0` bumps are regeneration side effects of rebuilding those test components with the in-tree CLI/SDK during the T42 sweep (the CLI scaffolding emits the current skill table; the SDK now resolves the published wasip3 0.7.0). They are kept (reverting would just recur on the next rebuild) and documented here rather than excluded | done |
| T42.8e | (oracle round 2) Re-run targeted verification after T42.8a–T42.8d — **all green on the final formatted tree**: `cargo fmt --all -- --check` clean in both repos; `cargo check -p golem-worker-executor` passes (`/tmp/t42-8e-check.txt`); executor lib suite **665 passed, 0 failed** (663 + the 2 new T42.8a replay-scope regression tests, `/tmp/t42-8e-lib-tests.txt`); Wasmtime terminal-observer suite **16/16** incl. the 3 new failure-path tests (`/tmp/t42-8e-wasmtime-observer.txt`) and full component-async suite **101/101** (`/tmp/t42-8e-wasmtime-full.txt`); gated discard + sleep regressions **3/3** (`integration::http::outgoing_http_persisted_body_chunk_discarded_before_delivery`, `integration::wasi::p3_resuming_sleep`, `integration::wasi::p3_sleep_suspends_and_resumes` — `/tmp/t42-8e-sleep-discard.txt`); residue audits clean (no `TODO(p3)`, no `TEMPDBG`/debug identifiers in executor or Wasmtime crates). Oracle round 3 requested with fresh final diffs | done |

The sweep surfaced a
real durability hole and T42 grew a fix for it: a guest can drop a completion
future *after* the durable `End` is persisted but *before* the response is
delivered (second-stage channel sends, post-`End` span finishes, wire
conversions), and replay would then deliver a response the recorded run never
observed. The fix adds:

- a new `CompletionDiscarded` oplog entry (raw protobuf field 48, public field
  49, WIT + public-oplog rendering + status fold as a hint; matcher/DSL variant
  after `HostStreamFrame`) recording a successful `End` whose completion was
  silently discarded before guest delivery;
- a deferred guest-delivery token (`CompletionDelivery`, in
  `durable_host/concurrent.rs`) returned by
  `complete_access_deferred`/`replay_access_deferred` for call sites whose
  guest boundary lies beyond the accessor terminal (P3 `client.send`, P3
  response-body trailers delivery, wasm-RPC `future-invoke-result.get`).
  Live, the token settles by how the guest actually consumes the terminal:
  values that still cross Wasmtime's lowering/terminal-consumption boundary
  hand the token to a Wasmtime *terminal observer*
  (`deliver_at_accessor_terminal` → forked-Wasmtime
  `Accessor::register_terminal_observer`, `TerminalConsumption`), which
  reports `Delivered` on actual guest receipt (waitable-set wait/poll,
  callback dispatch, sync-lowered return) and `Discarded`/`Cancelled` when
  the guest consumes the pending terminal via `subtask.cancel` — either way
  post-`End`, so both record the marker. An observer dropped uninvoked
  (trap, lowering failure, teardown, or superseded by a later durable call's
  observer in the same host function) suppresses: nothing was silently
  discarded (or replay redelivers the internal completion). Explicit
  `delivered()` remains only on genuine host-side delivery boundaries
  (channel sends already past lowering), `suppress()` on caller-observed
  post-`End` errors, `discarded()` (cancellation-safe, marker join +
  in-flight permit handed to the drain queue on a tear) on detected silent
  discards, and an armed `Drop` records the marker via an owned task joined
  by invocation settlement. Ordered post-`End` entries (durable
  `FinishSpan`) chain before any marker (`append_ordered`), preserving the
  positional `End → FinishSpan → CompletionDiscarded` replay order.
- invocation-boundary tolerance for live-only abandoned durable calls
  (`ReplayState::AbandonedStarts`): a durable call issued live but never
  re-issued by the replayed guest (e.g. a timed-out `consume-body` whose
  chunk raced the reader drop) leaves a never-claimed `Start` + terminal
  tail before `AgentInvocationFinished`. Only the
  agent-invocation-finished reader drains such records, with strict
  structural validation (every drained `Start` closed exactly once;
  terminals without a drained `Start`, duplicate terminals, unclosed
  `Start`s, and unrelated entries stay fatal; claimed `Start`s are never
  drained; the tracker dies at the finished-marker read) and a summary
  `warn`. Side-effecting host functions (`GolemApiFork`) are never drained
  (`AbandonedStarts::can_drain`): an unclaimed fork pair at the invocation
  boundary stays fatal. 14 dedicated `invocation_boundary_*` unit tests
  (tolerated: closed/cancelled/nested abandoned scopes, abandoned child of a
  *claimed* parent with its `CompletionDiscarded` marker — the exact
  discarded-chunk shape — an abandoned consume-body scope shape, and an
  unknown/cross-scope parent link; fatal: unclosed abandoned
  `Start`, duplicate and mixed `End`+`Cancelled` terminals, terminals
  without a drained `Start` or targeting an already-resolved claimed
  `Start`, an unclaimed `GolemApiFork` pair, unrelated non-hint entries).
- response-body chunk delivery is itself a deferred boundary: each
  consume-body child chunk's `End` is persisted *before* the chunk crosses
  the demand channel to the guest-facing stream, so the child terminal now
  goes through `complete_access_deferred`/`replay_access_deferred` too.
  Live, a successful demand send settles `delivered()`; a dropped demand
  receiver (guest dropped the body reader between the child `End` and the
  send — e.g. a guest-side timeout winning the race) records the child's
  `CompletionDiscarded` marker via `discarded()` and finalizes the body
  through the normal abandonment path — the chunk does not count as
  delivered, the parent closes with a clean terminal — instead of trapping
  the whole invocation. On replay, a `CompletedButDiscarded` child is never
  re-sent: if the replayed guest never demands it, the abandoned scope
  (child `Start`+`End`, hint marker) is skipped at the invocation boundary;
  if a diverged replayed guest does demand it, the task parks until the
  guest drops the reader at the same point it did live, then finalizes
  identically. This closes the redelivery hole where replay would hand the
  guest a persisted chunk the recorded run never delivered. The replay
  `Data`/`End` arms drop the read's cancel plumbing
  (`cancel_rx`/`read_cancel_ack`) as soon as the replayed chunk is produced,
  mirroring the live path (where the read future owns the plumbing and is
  dropped once a frame wins the select): holding the `cancel_ack` sender
  across the delivery boundary would leave a cancelling guest blocked in
  sync `stream.cancel-read` while the replay-discarded delivery parks on
  the demand — a three-way circular wait.
- replay: `ReplayState` scans markers up front, resolves the affected `End` as
  `CompletedButDiscarded { end_idx, marker_idx, response }`; deferred replay
  performs the recorded deterministic post-`End` continuation and parks at the
  delivery boundary; non-deferred consumers reject the resolution.
  `set_replay_target` keeps the marker map in sync under the cursor
  transaction lock in both directions (growth rescans and merges idempotently;
  shrink prunes markers beyond the target), and delivery-time validation
  rejects a resolution whose marker lies beyond the effective target;
- cut-point validation (`worker/cut_point.rs`) rejects fork/revert cuts
  splitting an `End` from its marker; debugger playback/rewind target
  validation (`debug_service.rs::find_split_discarded_completion_at`) rejects
  targets strictly between an `End` and its marker over the raw oplog,
  honoring `Jump`/`Revert`-dropped regions on both sides; playback overrides
  must preserve marker pairing signatures.

Evidence (2026-07-20 rerun over the final implementation):

- `cargo fmt` + `cargo check -p golem-worker-executor -p golem-debugging-service
  -p golem-cli -p golem-test-framework -p golem-worker-service`: clean;
- focused executor units (`replay_state::tests::`, `cut_point::tests::`,
  `concurrent::tests::`): 87 passed, 0 failed (includes 5 new
  `completion_delivery_*` tests: no marker on `delivered`/`suppress`, marker
  via drain on armed drop, inline marker on `discarded`, cancellation-safety
  of `discarded()`, ordered-append-before-marker);
- debugging-service units incl. 5 new `split_*` target-validation tests: pass;
- key executor regressions (`wasi::file_update_1`,
  `wasi::file_update_in_the_middle_of_exported_function`,
  `wasi::http_timeout_and_restart`): 3 passed;
- debugging-service integration tests: 18 passed;
- HTTP-tagged executor surface (63 tests): rerun in `tmp/t42-httptag-run2.txt`;
- full `worker-executor-tests` accounting (earlier sweep + recovery runs, all
  745 tests classified): the only failures are (a) stale TypeScript fixtures
  importing `golem:core/types@1.5.0` — since resolved, see below, (b) load
  flakes that pass in isolation (`wasi::oplog_replay_after_parallel_http_requests`
  10/10 in isolation, `spawning_many_workers_that_sleep_long_enough_to_get_suspended`),
  and (c) `scalability::dynamic_large_memory_allocation` passing with
  `RUST_LOG=warn` (log-volume artifact). Artifacts under `tmp/t42-*.txt`.

Evidence for the boundary-tolerance + abandonment fixes (2026-07-20):

- focused replay units: all 14 `invocation_boundary_*` tests pass as part of
  the 53-test `replay_state::tests::` suite (`/tmp/t42-replay-unit-new.txt`,
  53 passed, 0 failed); full executor lib suite on the final tree: 663 passed,
  0 failed (`/tmp/t42-lib-tests-cleanup.txt`);
- `wasi::http_timeout_and_restart` stress loop (the previously-intermittent
  regression: abandoned `consume-body` `Start` at invocation finish, original
  failures 1/10 and run 16/30): 30/30 passed after the fix
  (`/tmp/t42-timeout-loop2-summary.txt`); the producer-side race remains
  timing-based by nature — the deterministic coverage is the 8 boundary unit
  tests asserting exactly the oplog shapes the race produces;
- stale TS fixture resolved: rebuilt the local TS SDK
  (wasi-p3 wasm-rquickjs, agent template) and
  `golem_it_constructor_parameter_echo.wasm`; the 3 previously-failing
  snapshot tests (`ts_default_json_snapshot_recovery`,
  `ts_sqlite_multipart_snapshot_recovery`,
  `ts_default_json_snapshot_recovery_across_multiple_restarts`) pass
  (`/tmp/t42-ts-snapshot-tests.txt`);
- integration sweep (`wasi::http`, `concurrent_delivery_order::`,
  `durability::`): 22 passed, 0 failed (`/tmp/t42-http-tests2.txt`);
- forked Wasmtime (`golem-wasmtime-v46.0.1-p3`): 9 `terminal_observer` tests
  and the full `component-async-tests` suite (94 tests) pass
  (`/tmp/t42-wasmtime-terminal.txt`, `/tmp/t42-wasmtime-full2.txt`);
- Accessor delivery audit: explicit `delivered()`/`suppress()`/`discarded()`
  calls exist only in the three deferred-delivery modules
  (`p3/http/send.rs`, `p3/http/response_body.rs`, `wasm_rpc/mod.rs`) plus the
  machinery itself; every other durable call site uses plain
  `complete_access`/`replay_access`, which delivers at the accessor terminal
  automatically.

Documentation: `p3-gaps.md` close-out header + per-gap `Resolved by` markers
(G33/T43–T47 and G35/T48 remain open), checklist rows #8/#15/#16 done,
migration-notes blocker history preserved.

---

## Phase 9 — SDKs (G33)

### T43 — Scala SDK: runtime port off pollables

Port the Scala JS runtime facades off the pollable/subscribe model to P3
async (`HostApi.scala`, `RemoteAgentClient.scala`, `AgentHostApi.scala`,
`WasmRpcApi.scala`, `AgentHostTypes.scala` wall-clock comment); regenerate the
DTS/facade layer from the P3 WIT (replace stale `wasi_io_0_2_3_*` /
wall-clock declarations). Use the `golem-scala-development` and
`golem-scala-code-generation` skills.

**Verify:** Scala SDK compiles; `rg -i "pollable|subscribe\(\)|wall-clock"
sdks/scala/core` returns no runtime-path hits; SDK unit tests pass.

### T44 — Scala SDK: base image regeneration + integration tests

Regenerate `agent_guest.wasm` (`sdks/scala/scripts/generate-agent-guest-wasm.sh`,
see `golem-scala-base-image` skill) and install into sbt/mill resources.
Rebuild the Scala test agents and run the Scala integration tests
(`golem-scala-integration-tests` skill).

**Verify:** `wasm-tools component wit` on the new `agent_guest.wasm` shows
`golem:agent/guest@2.0.0` exported and P3 imports only (plus expected P2 std
imports); relevant Scala integration tests (`GolemExamplesIntegrationSpec`)
pass against the current executor.

Completion evidence (2026-07-21):

- `sbt golemTestAll` passed across the supported Scala versions; four focused
  instant-conversion regressions cover negative and large epoch values.
- Regenerating `agent_guest.wasm` with the wasm-rquickjs `wasi-p3` branch and
  rebuilding the Scala test agents passed. The sbt, mill, and test-agent
  copies are byte-identical (SHA-256
  `51001091428e7acdf82c7473435e9d6c18f91a0867ab7cade06e271cd815f9d4`).
  The component exports `golem:agent/guest@2.0.0` and imports the P3 Golem
  host and clock interfaces. Its remaining P2 poll/wall-clock imports belong
  to the Rust `wasm32-wasip2` std/QuickJS compatibility layer; the Scala and
  Golem-owned runtime surfaces no longer expose or use them.
- All 28 promise, RPC, fork, trigger, and agent-to-agent scenarios reached by
  the full integration run passed. The remaining class was blocked after the
  executor panicked in the pre-existing durable blobstore wrapper; an isolated
  current-executor run of `sync-return` and `http-webhook-create-and-await`
  passed 2/2.
- CI remains pinned to wasm-rquickjs 0.3.4 pending a release of its `wasi-p3`
  branch; updating that external tool pin is a follow-up rather than SDK
  runtime work.

### T45 — MoonBit SDK: bindings regeneration + API port

Regenerate MoonBit bindings from the P3 WIT with a single wit-bindgen version
(eliminate the 0.42.1 remnants in `world/agentGuest` / `gen/`); remove/replace
P2 `wasi:io/poll` and stream `subscribe` interfaces; port `promises.mbt`,
websocket, and RPC APIs to P3 async semantics. See `sdks/moonbit/AGENTS.md`
and the moonbit skills.

**Verify:** `moon check`/`moon test` green in `sdks/moonbit/golem_sdk`;
`rg -i "pollable|subscribe" sdks/moonbit/golem_sdk` returns no active-code
hits; update `sdks/moonbit/AGENTS.md` to the verified state.

### T46 — MoonBit SDK: example build + verification

Build `sdks/moonbit/golem_sdk_example1` to a final component and verify its
WIT surface; deploy against a local executor and invoke an agent method
end-to-end.

**Verify:** `wasm-tools component wit` on the built artifact shows
`export golem:agent/guest@2.0.0` and P3 imports only (plus expected P2 std
imports); one end-to-end invocation succeeds via `golem` CLI local run.

### T47 — TS SDK migration (blocked)

Blocked on wasm-rquickjs P3 support (external parallel task). When unblocked:
regenerate DTS/types from P3 WIT, port runtime off `Pollable`/`subscribe`
(`hostapi.ts`, `clientGeneration.ts`, `pollableUtils.ts`, `baseAgent.ts`
wall-clock import), rebuild `agent_guest.wasm` with `wasm-rquickjs-cli`, then
migrate the TS test components (`agent-sdk-ts`, `agent-self-rpc`,
`agent-promise`, `agent-rpc`, `benchmark-agent-ts`,
`agent-constructor-parameter-echo`).

**Verify:** TS SDK builds; TS test components rebuilt; all worker-executor and
integration tests using TS components pass.
