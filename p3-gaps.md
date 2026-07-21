# P3 executor gaps

> **Close-out status (T42, 2026-07-21).** All executor-side gaps in this
> document **except G35** are resolved; each resolved gap section below
> carries a `Resolved by <task>` marker pointing at the closing task in
> [`p3-gaps-tasks.md`](./p3-gaps-tasks.md) (task status entries there hold the
> implementation details and verification evidence). The T42 full-suite sweep
> is green: executor lib/unit suites, worker-executor test groups, full
> integration + sharding tests, the CLI integration suite (48 passed, 0
> failed, 2 ignored in one parallel run, incl. MoonBit/Scala/TS/mixed-language
> after tracked SDK/template fixes), and the debugging-service suite (18/18).
> Two items remain open:
> **G33** (Scala/MoonBit/TS SDK runtime migration — tasks T43–T46 todo, T47
> blocked on wasm-rquickjs; the wasm-rquickjs `--target wasi-p3` support and
> the CI `WASM_RQUICKJS_VERSION` pin bump also depend on an upstream release,
> see T42.5e-19) and **G35** (claim-safe span records for concurrent P3 sends
> — T48 todo). The body of this document is a historical snapshot of the
> audits; statements about missing functionality describe the state *at audit
> time*, not the current code.

Three parts:

- **Part 1 (G1–G7):** status snapshot after migrating `test-components/http-tests`
  to WASI P3, measured by running the 43 `http_tests`-tagged worker-executor
  tests one by one.
- **Part 2 (G8–G24):** full P2↔P3 host-function parity audit (2026-07-02),
  covering every durable host wrapper, the durability core, and executor/workspace
  integration points — not just HTTP.
- **Part 3 (G25–G34):** deep-dive results for twelve areas the Part 2 audit
  flagged as uncovered: spawned tasks, re-entrance, timeouts, stream/future
  types, region manipulation, oplog processors, debugging, oplog GC, mixed
  ABI, embedders, SDKs, and memory accounting.

---

# Part 1 — wasi-http test-run gaps (G1–G7)

Status snapshot after migrating `test-components/http-tests` to WASI P3
(`wasi-fetch` + P3 `wasi:http`, no `wstd`, no `golem-wasi-http`, no pollables).

**These are not "P2-only semantic tests."** They are Golem's own semantic tests
(durable replay, idempotency/trace-header injection, HTTP-call accounting,
transparent HTTP retries, streaming durability). They fail because the
**executor's P3 `wasi:http` / `wasi-fetch` implementation is incomplete**, not
because the tests inherently require P2. The migrated component is correct; it
exposes real gaps in the runtime.

`host-api-tests` is **also not on P3 yet, and that is a gap, not a design
choice.** Its P2 HTTP helpers (`raw_http.rs`, `raw_wasi_http.rs`) carry a doc
comment claiming the P2 path is kept because durable oplog recording and
trace/idempotency-header injection are *"[not] available through the
asynchronous wasi-fetch client"* — but that is exactly the missing P3
functionality (G1–G6 below). `host-api-tests` must be migrated to P3 like every
other component; where a P3 migration is not yet possible or would not pass,
that is a runtime gap to fix, recorded in the dedicated section below.

## How this was measured

- Component: `test-components/http-tests` (flat artifact
  `test-components/golem_it_http_tests_release.wasm`, rebuilt + `golem exec copy`).
- Test set: the 43 worker-executor tests tagged `http_tests`
  (in `golem-worker-executor/tests/{http,api,wasi,in_function_retry,resource_limits}.rs`).
- Run one-by-one:
  ```bash
  cargo-test-r run --package golem-worker-executor --test integration -- <name> --exact --report-time
  ```
- macOS has no `timeout`; long-hanging runs were killed by an external wrapper.
  A "TIMEOUT" below means the test did not terminate within the wrapper window
  (and, for POST-body cases, is heading for the 600s first-byte timeout).
- Prereqs: `redis-cli ping => PONG`.

Regenerate the tagged list:
```bash
for f in http api wasi in_function_retry resource_limits; do
  awk -v mod="$f" '
    /^async fn /{name=$3; sub(/\(.*/,"",name)}
    /tagged_as\("http_tests"\)/{print mod"::"name}
  ' "golem-worker-executor/tests/$f.rs"
done | sort -u | grep -v "::$"
```

## Summary

| Result   | Count |
|----------|-------|
| PASS     | 11    |
| FAIL     | 22    |
| TIMEOUT  | 10    |
| **Total**| **43**|

| Gap | Theme | Tests |
|-----|-------|-------|
| G1  | Outgoing request **body is never uploaded** on the P3 send path | 6 |
| G2  | Durable **replay / restart / streaming** of in-flight P3 HTTP is broken | 9 |
| G3  | **Trace-context / idempotency-key headers not injected** into P3 requests | 1 |
| G4  | **HTTP call-limit accounting** not applied to P3 requests | 1 |
| G5  | **Transparent in-function HTTP retry** not wired for the P3 path | 12 |
| G6  | Transport/protocol errors on P3 not **classified as retryable** (worker fails instead of retrying) | 3 |
| G7  | `host-api-tests` HTTP still on the **P2 path (incl. pollables)** — must migrate to P3 | 14 |

Likely primary fix surface: `golem-worker-executor/src/durable_host/p3/http.rs`
and the P3 `wasi:http` host bindings (send / body-stream / consume-body /
durability recording), plus the retry/idempotency/accounting hooks that today
only exist on the P2 `outgoing-handler` path.

---

## G1 — Outgoing request body is not uploaded (P3 send path)

**Resolved by T01** (request-body upload future driven and its result recorded).

**Symptom.** Any POST whose test server actually *reads* the request body
(`post(move |headers, body: Bytes| ...)`) hangs. Request headers reach the
server, but the request body upload never completes, so the server never
produces a response and the client sits until the first-byte timeout.

**Proof (discriminator).**
- `http::outgoing_http_contains_idempotency_key` is a POST whose server reads
  **only headers** → it returns quickly (fails for a *different* reason, see G3).
- `http::http_client` is a POST whose server reads `body: Bytes` → it hung until
  `Transport("ErrorCode::ConnectionReadTimeout")` (~600s first-byte timeout).

So on the current P3 path the guest request body stream is not driven to
completion before awaiting the response.

**Affected tests**
- `http::http_client` — TIMEOUT (~600s, `ConnectionReadTimeout`)
- `http::http_client_using_reqwest` — TIMEOUT (HttpClient2 `run`, POST `/post-example`)
- `http::http_client_using_reqwest_async` — TIMEOUT (HttpClient3 `run`, POST)
- `http::http_client_using_reqwest_async_parallel` — TIMEOUT (parallel POSTs)
- `wasi::http_client_response_persisted_between_invocations` — TIMEOUT
  (HttpClient `send_request` is a POST whose server reads the body; blocks before
  the persistence behavior it is meant to test can even run)
- `in_function_retry::http_awaiting_response_retry_resends_full_body_after_output_stream_retry`
  — TIMEOUT (POST body; also depends on G5)

---

## G2 — Durable replay / restart / streaming of in-flight HTTP is broken

**Resolved by T09** (in-flight P3 HTTP sends rebuilt after restart), with
T11 covering the suspend-coupled streaming+sleep cases and T40 providing the
runtime replay/cancellation verification.

**Symptom.** Restart/replay of a worker that is mid-HTTP (streaming a response,
or resuming a poll loop) fails fast with:

```
Runtime error: Non-idempotent remote write operation was not completed, cannot retry
```

or times out (parallel streaming reads never complete), or the worker never
returns to `Running` after a restart.

**Suspected cause.** P3 HTTP durable-call/replay semantics for a live send plus a
streamed response/body are incomplete: the runtime cannot safely replay or
resume a non-idempotent remote write, and streamed body state is not
reconstructed on restart. (Note: `p3-migration-notes.md` also flags that P3
suspend/sleep during a streaming read is not implemented yet, which contributes
to the streaming timeouts.)

**Affected tests**
- `wasi::oplog_replay_after_streaming_http_read` — FAIL (fast)
- `wasi::oplog_replay_after_raw_streaming_http_read` — FAIL (fast)
- `wasi::oplog_replay_after_parallel_streaming_http_reads` — TIMEOUT
- `wasi::oplog_replay_after_parallel_raw_streaming_http_reads` — TIMEOUT
- `wasi::oplog_replay_streaming_http_then_sleep_future_trailers_bug` — TIMEOUT
  (also blocked by unimplemented P3 suspend/sleep)
- `wasi::http_connection_pool_contention_with_restart` — FAIL
- `wasi::http_client_interrupting_response_stream` — FAIL (timeout waiting for
  worker status `Running` after interrupt/restart)
- `wasi::http_client_interrupting_response_stream_async` — FAIL (same bucket)
- `api::invocation_queue_is_persistent` — TIMEOUT (HttpClient2 `start_polling`
  GET poll loop, interrupt + restart; worker does not resume the poll loop after
  restart) — *suspected G2, not fully root-caused*

---

## G3 — Trace-context / idempotency-key headers not injected into P3 requests

**Resolved by T03** (idempotency-key injection) and **T04** (trace-context
headers + outgoing HTTP spans). See also open follow-up G35/T48 for span
records under concurrent sends.

**Symptom.** The executor injects the Golem invocation-context trace headers and
the per-invocation idempotency key into **P2 `outgoing-handler`** requests, but
not into P3 `wasi-fetch` / `client::send`.

**Evidence.** `http::outgoing_http_contains_idempotency_key` completes (its
server reads only headers, so G1 does not apply) but the assertion fails: the
server observed `idempotency-key: None` where a key was expected. This is the
same capability `raw_http.rs` documents as unavailable on the async client.

**Affected tests**
- `http::outgoing_http_contains_idempotency_key` — FAIL (header missing)

---

## G4 — HTTP call-limit accounting not applied to P3 requests

**Resolved by T02** (call-limit + monthly quota accounting on the P3 send path).

**Symptom.** The per-invocation outgoing-HTTP call limit is not counted or
enforced for P3 `client::send`, so an invocation that should trap for exceeding
the limit succeeds instead.

**Affected tests**
- `resource_limits::http_call_limit_exceeded_traps_invocation` — FAIL
  ("expected invocation to fail due to HTTP call limit, but it succeeded")

---

## G5 — Transparent in-function HTTP retry not wired for the P3 path

**Resolved by T13–T16** (design pass, awaiting-response phase, request-body
write phase, resuming-response-body phase).

**Symptom.** The transparent HTTP-retry feature (inline retries driven by
status code, output-stream write failures, resuming/skipping response bodies,
trailers, idempotence policy, write-zeroes body reconstruction, transient
connection failures, and the zone-1 delay/trap thresholds) is implemented for
the P2 outgoing-handler / output-stream / body-stream hooks. Those hooks do not
exist on the P3 `wasi-fetch` path, so the retry classification and body
reconstruction never happen; tests either get a wrong result, a hard
`HttpProtocolError` trap, or an un-retried raw body.

**Affected tests**
- `in_function_retry::http_get_retried_inline_even_when_idempotence_disabled` — FAIL
- `in_function_retry::http_no_output_stream_retry_when_subscribe_used` — FAIL
- `in_function_retry::http_no_resuming_response_body_retry_when_body_skip_used` — FAIL
- `in_function_retry::http_no_retry_when_trailers_present` — FAIL
- `in_function_retry::http_output_stream_inline_retry_on_body_write_failure` — FAIL
- `in_function_retry::http_post_fails_permanently_when_idempotence_disabled` — FAIL
- `in_function_retry::http_resuming_response_body_inline_retry_accepts_matching_non_partial_success_status` — FAIL
- `in_function_retry::http_resuming_response_body_inline_retry_on_body_read_failure` — FAIL
- `in_function_retry::http_status_retry_policy_retries_matching_status` — FAIL
- `in_function_retry::http_write_zeroes_body_reconstruction` — FAIL
- `in_function_retry::http_zone1_falls_back_to_trap_when_delay_exceeds_threshold` — FAIL
- `in_function_retry::http_zone1_inline_retry_on_transient_connection_failure` — FAIL

(`http_awaiting_response_retry_resends_full_body_after_output_stream_retry` also
belongs to this feature but is currently blocked earlier by G1, so it is listed
under G1.)

---

## G6 — Transport/protocol errors not classified as retryable (worker fails)

**Resolved by T06** (worker-level retry classification for P3 HTTP errors);
the systemic routing helper is T07 (see G17).

**Symptom.** When a P3 request errors, the guest's `.expect()`/`.unwrap()` on the
result panics, surfacing as a deterministic trap, and the worker goes to
`Failed`. The tests expect the worker to enter `Retrying` (worker-level durable
retry of the HTTP failure) and then be interruptible/deletable.

**Observed.** `Request failed: Transport("ErrorCode::HttpProtocolError")` →
trap → worker `Failed`; test times out waiting for status `Retrying`.

**Affected tests**
- `api::interrupt_worker_during_delayed_recovery_retry` — FAIL (worker `Failed`, not `Retrying`)
- `api::delete_worker_during_delayed_recovery_retry` — FAIL (same)
- `api::long_running_poll_loop_http_failures_are_retried` — FAIL

---

## G7 — `host-api-tests` still on the P2 HTTP path (must migrate to P3)

**Resolved by T41** (`raw_http.rs`/`raw_wasi_http.rs`/`custom_durability.rs`
migrated to P3; all G7-listed tests pass on the P3 path).

`host-api-tests` has not been migrated to P3 for its HTTP-bearing behaviors. Its
`Cargo.toml` already carries `wasi-fetch` (and one agent, `GolemWasiHttp`, is
already P3), but the following surfaces still use the synchronous P2 `wasi:http`
path — including pollables — precisely for the durability/tracing behaviors the
P3 path does not yet provide:

- `test-components/host-api-tests/src/raw_http.rs` — P2 `outgoing-handler` helper
  that blocks on `future.subscribe().block()` (a pollable).
- `test-components/host-api-tests/src/raw_wasi_http.rs` — the `RawWasiHttp` agent,
  raw P2 `wasi:http` driven by `wasi::io::poll::poll` (pollables).

These are not a legitimate exception to "no pollables / no P2 HTTP"; they are the
work still to do. Migrating them to P3 depends on closing G1–G6, plus durable
transaction replay/undo and custom-durability integration over the P3 HTTP path.

**Who depends on `raw_http` (P2) today**

- `InvocationContext` (`invocation_context.rs`) — serializes the current
  invocation context and POSTs it to `/invocation-context`; exercises
  trace-context propagation and header injection (G3).
- `GolemHostApi` transaction/saga methods (`golem_host_api.rs`) — `remote_call`,
  `remote_call_undo`, `remote_side_effect`, and `idempotence_flag` all issue HTTP
  via `raw_http::request`. HTTP is the *non-idempotent remote side effect* these
  saga tests replay, compensate, and checkpoint over — so they need durable
  request recording + replay/undo on the P3 path (G1, G2, G3, G5).
- `CustomDurability` (`custom_durability.rs`) — `perform_callback` does an HTTP
  GET inside a custom-durability wrapper; needs P3 HTTP to compose with custom
  durability.

**Executor tests that currently pass only because they use the P2 path**
(migrating `host-api-tests` HTTP to P3 must keep these green):

- `observability::invocation_context_test` — trace propagation over HTTP (G3)
- `durability::custom_durability_1` — custom durability wrapping HTTP
- `durability::lazy_pollable` — custom durability / lazy pollable over HTTP
- `transactions::golem_rust_atomic_region`
- `transactions::golem_rust_atomic_region_async`
- `transactions::golem_rust_idempotence_on`
- `transactions::golem_rust_idempotence_off`
- `transactions::golem_rust_persist_nothing`
- `transactions::golem_rust_persist_nothing_async`
- `transactions::golem_rust_fallible_transaction`
- `transactions::golem_rust_infallible_transaction`
- `transactions::golem_rust_checkpoint`
- `transactions::golem_rust_checkpoint_async`
- `transactions::golem_rust_jump`

**Not currently exercised, but still on P2 and must be cleaned up**

- `RawWasiHttp` agent (`raw_wasi_http.rs`) — no executor test references it today;
  it still uses P2 `wasi:http` + `io::poll` pollables. Either migrate it to P3 or
  remove it.
- `GolemWasiHttp` agent (`golem_wasi_http.rs`) — already P3 (`wasi-fetch`) but
  also unreferenced by executor tests; verify whether it is still needed.

These were not run one-by-one in this pass; they are recorded as the migration
work / gaps blocking `host-api-tests` from moving to P3.

## What already works (bounds the gaps)

These 11 pass, so basic P3 GET flows, interrupt/resume of a poll loop (without a
full restart), connection-pool sharing without restart, and the timeout race all
work:

- `api::long_running_poll_loop_works_as_expected`
- `api::long_running_poll_loop_works_as_expected_async_http`
- `api::long_running_poll_loop_interrupting_and_resuming_by_second_invocation`
- `api::long_running_poll_loop_connection_breaks_on_interrupt`
- `api::long_running_poll_loop_connection_can_be_restored_after_resume`
- `api::long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker`
- `api::long_running_poll_loop_worker_can_be_deleted_after_interrupt`
- `resource_limits::concurrent_agent_idle_releases_permit`
- `resource_limits::concurrent_agent_limit_waits_for_running_agent_to_finish`
- `wasi::http_connection_pool_contention_between_agents`
- `wasi::http_timeout_and_restart`

---

## Appendix — full per-test result

| Test | Result | Gap |
|------|--------|-----|
| api::long_running_poll_loop_works_as_expected | PASS | — |
| api::long_running_poll_loop_works_as_expected_async_http | PASS | — |
| api::long_running_poll_loop_interrupting_and_resuming_by_second_invocation | PASS | — |
| api::long_running_poll_loop_connection_breaks_on_interrupt | PASS | — |
| api::long_running_poll_loop_connection_can_be_restored_after_resume | PASS | — |
| api::long_running_poll_loop_connection_retry_does_not_resume_interrupted_worker | PASS | — |
| api::long_running_poll_loop_worker_can_be_deleted_after_interrupt | PASS | — |
| resource_limits::concurrent_agent_idle_releases_permit | PASS | — |
| resource_limits::concurrent_agent_limit_waits_for_running_agent_to_finish | PASS | — |
| wasi::http_connection_pool_contention_between_agents | PASS | — |
| wasi::http_timeout_and_restart | PASS | — |
| http::http_client | TIMEOUT | G1 |
| http::http_client_using_reqwest | TIMEOUT | G1 |
| http::http_client_using_reqwest_async | TIMEOUT | G1 |
| http::http_client_using_reqwest_async_parallel | TIMEOUT | G1 |
| wasi::http_client_response_persisted_between_invocations | TIMEOUT | G1 |
| in_function_retry::http_awaiting_response_retry_resends_full_body_after_output_stream_retry | TIMEOUT | G1 (+G5) |
| wasi::oplog_replay_after_streaming_http_read | FAIL | G2 |
| wasi::oplog_replay_after_raw_streaming_http_read | FAIL | G2 |
| wasi::oplog_replay_after_parallel_streaming_http_reads | TIMEOUT | G2 |
| wasi::oplog_replay_after_parallel_raw_streaming_http_reads | TIMEOUT | G2 |
| wasi::oplog_replay_streaming_http_then_sleep_future_trailers_bug | TIMEOUT | G2 |
| wasi::http_connection_pool_contention_with_restart | FAIL | G2 |
| wasi::http_client_interrupting_response_stream | FAIL | G2 |
| wasi::http_client_interrupting_response_stream_async | FAIL | G2 |
| api::invocation_queue_is_persistent | TIMEOUT | G2 (suspected) |
| http::outgoing_http_contains_idempotency_key | FAIL | G3 |
| resource_limits::http_call_limit_exceeded_traps_invocation | FAIL | G4 |
| in_function_retry::http_get_retried_inline_even_when_idempotence_disabled | FAIL | G5 |
| in_function_retry::http_no_output_stream_retry_when_subscribe_used | FAIL | G5 |
| in_function_retry::http_no_resuming_response_body_retry_when_body_skip_used | FAIL | G5 |
| in_function_retry::http_no_retry_when_trailers_present | FAIL | G5 |
| in_function_retry::http_output_stream_inline_retry_on_body_write_failure | FAIL | G5 |
| in_function_retry::http_post_fails_permanently_when_idempotence_disabled | FAIL | G5 |
| in_function_retry::http_resuming_response_body_inline_retry_accepts_matching_non_partial_success_status | FAIL | G5 |
| in_function_retry::http_resuming_response_body_inline_retry_on_body_read_failure | FAIL | G5 |
| in_function_retry::http_status_retry_policy_retries_matching_status | FAIL | G5 |
| in_function_retry::http_write_zeroes_body_reconstruction | FAIL | G5 |
| in_function_retry::http_zone1_falls_back_to_trap_when_delay_exceeds_threshold | FAIL | G5 |
| in_function_retry::http_zone1_inline_retry_on_transient_connection_failure | FAIL | G5 |
| api::interrupt_worker_during_delayed_recovery_retry | FAIL | G6 |
| api::delete_worker_during_delayed_recovery_retry | FAIL | G6 |
| api::long_running_poll_loop_http_failures_are_retried | FAIL | G6 |

---

# Part 2 — full P2↔P3 parity audit (2026-07-02)

Method: systematic comparison of every P2 durable wrapper under
`golem-worker-executor/src/durable_host/` against its P3 counterpart under
`golem-worker-executor/src/durable_host/p3/`, plus the shared durability core
(`concurrent.rs`, `durability.rs`, `replay_state.rs`), oplog payload model,
and executor/workspace integration points. All findings are code-inspection
backed; uncertainty is flagged explicitly.

## HTTP root causes and additional findings (extends G1–G6)

These root-cause the Part 1 gaps and add findings not visible from the test run:

- **G1 root cause found.** The P3 `WasiHttpHooks::send_request` in
  `golem-worker-executor/src/durable_host/mod.rs` (~line 249) receives the
  request-body I/O future from wasmtime and discards it: `_ = fut;`. Nothing
  drives the guest body upload. Fix: spawn/drive that future on the live path
  and record its terminal result.
- **G3 root cause.** P2 injects a derived `idempotency-key` header
  (`http/outgoing_http.rs:312-330` via `derive_idempotency_key`); no equivalent
  exists anywhere on the P3 send path (`p3/http.rs`).
- Trace-context injection + outgoing HTTP **spans** both missing on P3: P2
  starts an invocation-context span and injects `traceparent`/`tracestate`
  (`http/outgoing_http.rs:278-309`); `p3/http.rs` has no `start_span`, trace,
  or idempotency code at all. P3 HTTP calls are invisible to observability.
- **G4 root cause.** P2 calls `check_and_increment_http_call_count()` and
  `record_monthly_http_call()` before sending (`http/outgoing_http.rs:226-237`);
  P3 send has neither.
- **G5 scope.** The entire 2,210-line `http/inline_retry.rs` stack has no P3
  hooks: status-code retry policies, transient-connection retry while awaiting
  response, output-stream write-failure retry with full-body resend,
  resuming-response-body retry via `Range`/206/416, write-zeroes body
  reconstruction, trailers-disable-retry, idempotent-method policy (GET retried
  even with idempotence off; POST fails permanently), zone-1 delay/trap
  thresholds, pooled-connection poisoning before retry. P3 send only picks
  `WriteRemote` vs `WriteRemoteBatched(None)` by method idempotency
  (`p3/http.rs:77-99`). Needs a port or redesign against P3 body streams.
- **G6 root cause.** P2 classifies `ErrorCode`s Transient/Permanent
  (`http/types.rs:1555-1614`) and calls `try_trigger_retry`; P3 serializes the
  `ErrorCode` into the oplog and returns it to the guest
  (`p3/http.rs:172-184`) — guest unwraps → deterministic trap → `Failed`.
- **G2 root cause.** P2 rebuilds a mid-flight request from recorded body writes
  (`HostFutureIncomingResponse::deferred` + `rebuild_request_after_replay`,
  `http/types.rs:1353-1444`). P3 has no rebuild path; an interrupted
  non-idempotent send hits the generic incomplete-Start rule
  (`concurrent.rs:2275-2285`) and errors with "Non-idempotent remote write
  operation was not completed, cannot retry".
- Per-request timeout/config parity: P2 captures request options into
  `HttpRequestState` and rebuilds `OutgoingRequestConfig` for retries; P3
  serializes options to the oplog but has no rebuild/retry path using them.
- Cancellation semantics differ: P2 has HTTP-specific cleanup on
  `future-incoming-response` drop (`http/types.rs:1321-1331`); P3 relies solely
  on generic `CallHandle` drop policies (`LeaveIncompleteOnDrop` for idempotent,
  `Cancellable` otherwise). Needs a deliberate decision + tests for dropping
  the response future / body stream mid-flight.
- Connection pooling itself is shared and works on P3
  (`pooled_send_request_p3`); what is missing is the retry-specific pool
  behavior (poisoning, resend).

## G8 — Request-body transmission result not recorded

**Resolved by T08** (transmission result recorded/replayed).

Already documented as a follow-up in `p3/http.rs` (~line 242) and checklist
item #8: a *non-deterministic* mid-body upload network error replays as
`Ok(())` on the guest's transmission `FutureReader`. Fix direction: wrap the
transmission future at `HostRequestWithStore::new`, mirroring
`HttpTrailersFutureProducer`.

## G9 — No suspend-on-long-sleep / suspend-coupled waits on P3

**Resolved by T11** (suspend-on-long-sleep for P3 `wait_until`/`wait_for`,
checklist #15) and **T12** (promise-await suspend parity).

- P3 `monotonic_clock::wait_until`/`wait_for` are durable (`ReadLocal`) but
  await wasmtime's timer directly (`p3/clocks.rs:142-179`). No
  `SuspendForSleep`: a P3 guest sleeping 1 hour keeps the worker resident for
  1 hour. P2 path: sleep-threshold hook in `durable_host/mod.rs:547-558` +
  `io/poll.rs:237-274` (suspend + scheduled wakeup). This is checklist item
  #15 and contributes to the G2 streaming+sleep timeouts.
- P2 `poll` also detects promise-only waits and suspends
  (`io/poll.rs:111-138`); the P3 promise `get` awaits in-process — no suspend.
- Likely also behind `api::invocation_queue_is_persistent` (G2-suspected):
  poll-loop resumption after restart.

## G10 — P2 worker stdio capture regressed (affects "P3" components too)

**Resolved by T17** (P2 worker stdout/stderr capture restored).

`ManagedStdOut`/`ManagedStdErr` (`durable_host/io/mod.rs:96-186`) now just wrap
`tokio::io::stdout()/stderr()` — worker output is piped to the **executor
process's stdio**, with no `InternalWorkerEvent::stdout/stderr` and no oplog
log entries. The P3 path *does* capture per-worker (`p3/cli.rs:170-235`).
Because Rust std lowers `println!` to **P2** `wasi:cli/stdout` even in wasip3
builds (see Blocker 0 in `p3-migration-notes.md`), effectively every
component's stdout/stderr currently bypasses worker log events. High priority:
route the P2 managed streams through the same worker-event/oplog emission as
the P3 path. Stdin is uniformly disabled on both paths (matches previous
behavior).

## G11 — P3 `cli::environment` not enriched

**Resolved by T18** (enriched environment on P3 `get_environment`).

P2 `get_environment` builds the deterministic enriched worker environment
(worker metadata env + agent defaults + Golem config)
(`cli/environment.rs:25-55`). P3 `get_environment` is a raw pass-through to
wasmtime-wasi (`p3/cli.rs:303-306`). A P3-native env lookup returns the wrong
environment; mitigated today only because std's `env` goes through P2. Port the
P2 logic.

## G12 — Filesystem: four specific holes (otherwise at parity)

**Resolved by T19** (metadata-hash durability, `fail_if_read_only`
enforcement, read-only flag masking).

P3 filesystem is at parity or better for streams, `stat`/`stat_at`,
`read_directory`, and storage-quota accounting. Remaining:

- `metadata_hash`/`metadata_hash_at` delegate directly instead of routing
  through durable stat (`p3/filesystem.rs:1257-1275`) — hashes can diverge
  between live and replay.
- Missing `fail_if_read_only` enforcement on P3 `set_times`, `set_times_at`,
  `rename_at`, `symlink_at`, and possibly `unlink_file_at` (P2 enforces all).
- P2 masks the write bit in `get_flags` for read-only workers; P3 doesn't.

## G13 — P3 DNS lookup lacks transient-failure retry classification

**Resolved by T07** (generic retryable-error routing helper + P3 DNS retry).

P2 `resolve_addresses` retries transient resolver failures
(`sockets/ip_name_lookup.rs:63-119`); P3 records/replays the result durably
(`p3/sockets.rs:1659-1691`) but has no transient/permanent classification.
Otherwise sockets on P3 are *ahead* of P2: TCP/UDP send/receive are newly
durable.

## G14 — `lazy-initialized-pollable` removed with no P3 replacement

**Resolved (T26): removed with no replacement.** The p2 resource was a
level-triggered, reusable, *rebindable* readiness handle; p3
`future`/`stream` handles are one-shot and linear, so no future-based design
can reproduce that contract — and no consumer needs it (golem-ai only used it
as an internal p2 bridge, never across caller-facing WIT; its Bedrock
`nopoll` build already runs the same durable state machine without it). In p3
the replay→live transition is expressed by resolving replayed results
immediately and directly awaiting the live source (e.g. mapping a host
`stream<u8>` to guest-created `wit_stream`/`wit_future` handles — no host
support needed). WIT decision note in `golem-durability.wit`; host TODO
removed; feature-specific test deleted (coverage held by
`durability::custom_durability_1` and
`wasi::oplog_replay_after_streaming_http_read`).

## G15 — Websocket: implemented but WIT design decision open — RESOLVED (T27)

Decision (T27): keep the request/response API shape, but `receive` and
`receive-with-timeout` are now `async func` in WIT and implemented on the
accessor-based host path (`HostWebsocketConnectionWithStore`), so a parked
receive no longer blocks other guest tasks. ABI change for the two functions
(components must be rebuilt against the new WIT); durable record shape and
replay/reconnect semantics unchanged. A P3-native `stream<message>` redesign
remains a possible separate follow-up.

## G16 — Keyvalue leftovers now guest-visible on P3

**Resolved by T20** (atomic ops implemented + async body write).

- `keyvalue/atomic.rs:29,40`: `increment` and `compare_and_swap` are
  `unimplemented!()` (traps the worker if called).
- `keyvalue/types.rs:222-235`: `outgoing_value_write_body_async` returns an
  "unsupported" error resource.

Decide: implement or remove from WIT.

## G17 — Systemic: guest-value errors bypass retry classification

**Resolved by T06 + T07** (retry classification for P3 HTTP errors and the
generic retryable-error routing helper used by all P3 wrappers).

Worker-level retry parity for P3 accessor calls holds only for errors escaping
via `CallHandle::trap` (call-owned trap context, `concurrent.rs:1818-1836` →
`model/mod.rs:286-324`). Any host wrapper that returns an error *value* to the
guest (HTTP `ErrorCode`, etc.) skips classification entirely — the guest
unwraps and the worker fails deterministically. G6 is the HTTP instance of
this; every future P3 wrapper must route retryable failures through the
host-retry machinery instead of returning them.

## G18 — Oplog schema diverges from the design notes; compat untested

**Resolved by T24** (pre-migration oplog compatibility test + design-notes
update).

Implemented design uses the `Start` oplog index as call identity with
`OplogEntry::{Start, End, Cancelled}` — not the notes' `call_id` +
format-version bump (`p3-migration-notes.md`). Appears backward-compatible by
`desert` evolution, but:

- no explicit test replays a pre-migration P2-era oplog through the concurrent
  resolver;
- the design notes should be updated to describe what was actually built.

## G19 — Durability core loose ends

**Resolved by T22** (PersistNothing on the live accessor path), **T23**
(cancellation-drain tests + stale comment) and **T25** (un-ignored
concurrent/suspendable durable-call tests).

- Cancellation drains are opportunistic (dropped `Cancellable` handles enqueue
  events recorded at the next safe drain point); stale comment "There is no
  recorder actor in production yet" (`concurrent.rs:352-355`) though the sink
  is wired. Needs comment fix + tests for guest-drop mid-call and worker
  interrupt mid-call.
- `PersistNothing` on the live accessor path: replay guards exist
  (`concurrent.rs:1513-1522`), but it is unproven that live
  `execute_access_start` suppresses `Start` entries under `PersistNothing`
  (only snapshotting is visibly handled). Audit + test.
- True runtime overlap of durable async host calls is untested: comments in
  `concurrent.rs:24-26` admit ported host methods "cannot truly overlap yet";
  the overlapping-call runtime tests are still `#[ignore]`d
  (`golem-worker-executor/tests/wasi.rs:1864,1929,1993,3338,3426`,
  `tests/durability.rs:139` — all `TODO(p3)`).

## G20 — Signed-instant time conversions

**Resolved by T21** (signed time conversion hardening).

`golem-common/src/model/oplog/payload/types.rs`:

- `SystemTime → SerializableDateTime` (lines ~143-149) still does
  `duration_since(UNIX_EPOCH).unwrap()` — panics on pre-epoch times, which P3
  signed instants permit.
- `SerializableDateTime → SystemTime` / P2 `Datetime` silently clamps negative
  seconds to the epoch. Decide whether clamping is acceptable; guard the
  panicking direction.

## G21 — Agent type extraction: P3 `wasi:http` imports unverified

**Resolved by T38** (P3 `wasi:http` imports verified/linked for extraction).

`golem-common/src/model/agent/extraction.rs:68-83` links P2 WASI + P3 WASI +
**P2 HTTP only** (`add_only_http_to_linker_async`). If
`wasmtime_wasi::p3::add_to_linker` does not cover `wasi:http`, extraction fails
for components importing P3 `wasi:http`. Verify with a component that imports
it.

## G22 — Debugging service: P2-only env override

**Resolved by T34** (debugging-service concurrent-entry fixes incl. the P3
env override).

`golem-debugging-service/src/debug_context.rs:473-486` overrides
`cli::environment` for P2 only; the P3 env pass-through bypasses the debug
override. Also no P3 debug-mode smoke test exists. (Linker itself is fine — it
reuses the executor's `create_linker`.)

## G23 — Public oplog rendering of P3 entries untested

**Resolved by T39** (public oplog rendering tests for P3 entries).

The generic `Start`/`End` typed-schema path
(`golem-worker-executor/src/model/public_oplog/mod.rs:344-376`) should render
P3 payloads (`P3HttpClientSend`, `P3HttpClientConsumeBody*`, P3 kv/blobstore,
sockets), but no test asserts they render correctly in `golem worker oplog`.
Add coverage.

## G24 — P3 host calls invisible to metrics/tracing

**Resolved by T05** (`observe_function_call` in all P3 wrappers).

P3 wrappers never call `observe_function_call` — zero hits under
`durable_host/p3/` vs 65 in `http/types.rs` alone. No
`golem_host_function_call_total` metrics, no per-call debug tracing for any P3
host call. Cheap fix; add to every P3 wrapper (or centralize in
`run_read_access` / the `CallHandle` entry points).

## Verified as done (no action needed)

- clocks/random `now`/`get_resolution`/bytes/u64/seed: durable parity.
- P3 stdout/stderr per-worker capture (`p3/cli.rs`) — the *P2* side regressed
  (G10), not P3.
- Filesystem streams, stat/stat_at (incl. `status_change_timestamp = None`),
  read_directory, storage quotas.
- TCP/UDP send/receive + DNS durable on P3 (better than P2).
- `HostFutureInvokeResultWithStore::get`, `HostGetPromiseResultWithStore::get`,
  keyvalue cache futures, kv/blobstore `incoming_value_consume_async`,
  `list_objects` — real durable accessor-based implementations, not stubs
  (checklist items #9–#14 hold up under inspection).
- rdbms / config / logging / quota durable hosts: no P3 gaps found.
- Compilation service + executor wasmtime `Config` parity
  (`wasm_component_model_async` etc.).
- Public oplog / protobuf conversions handle `Cancelled` and new entries
  structurally (only test coverage missing, G23).

## Test estate still to migrate

- `host-api-tests`: `raw_http.rs` / `raw_wasi_http.rs` still P2+pollables (G7)
  — blocked on G1–G6 + G14. *(Done: migrated by T41.)*
- All TS components — blocked on wasm-rquickjs P3 (external parallel task).
  *(Still blocked: T47.)*
- Six `#[ignore]`d executor tests tagged `TODO(p3)` (see G19).
  *(Done: un-ignored by T25.)*
- No wasip3 HTTP runtime harness exists — declared blocker for checklist item
  #8 runtime verification (replay roundtrip, overlapping sends, cancellation
  mid-replay). Prerequisite for calling the HTTP work done.
  *(Done: runtime verification landed with T40 against the migrated
  `http-tests` component.)*

## Suggested priority order

1. Drive the discarded request-body future (`_ = fut`) — unblocks all POST
   flows (G1).
2. P3 send preamble parity: idempotency key, trace headers + span, call limits
   (G3, G4).
3. Worker-level retry classification of P3 HTTP errors (G6/G17).
4. Suspend-on-long-sleep for P3 waits (G9 / checklist #15).
5. In-flight HTTP replay/rebuild (G2) + transmission-result recording (G8).
6. Inline retry stack port (G5) — biggest chunk; design pass first.
7. P2 stdio capture restoration (G10) and CLI env parity (G11).
8. The rest: G12, G13, G16, G14/G15 designs, G18–G24 cleanups; then
   host-api-tests migration (G7) once 1–6 land.

---

# Part 3 — deep-dive investigation results (G25–G34)

Twelve areas flagged by the Part 2 audit were investigated in dedicated
passes (2026-07-02, read-only code inspection; wasmtime internals verified
against the local wasmtime checkout). New gaps are numbered G25–G34; areas
that came back clean are recorded at the end.

## G25 — Guest-spawned tasks can outlive the invocation and break replay

**Resolved by T28** (spawned guest tasks drained before invocation completion).

The SDK enables wit-bindgen `async-spawn`; guests can spawn tasks that outlive
the exported function. Wasmtime's plain `run_concurrent` returns as soon as
the driving future completes and **leaves unfinished tasks parked in the
store** (they "will not make progress until `run_concurrent` is called again"
— wasmtime `func.rs:330-335`); wasmtime provides `run_concurrent_and_drain`
for exactly this, but Golem uses plain `run_concurrent`
(`worker/invocation.rs:207-215`) and writes + commits
`AgentInvocationFinished` immediately after.

Consequences:

- A tail task performing a durable host call after the invocation-finished
  marker produces `AgentInvocationStarted … Start/End … AgentInvocationFinished
  … stray Start/End` — not positionally replayable (replay expects
  `AgentInvocationStarted` next: `replay_state.rs:1144-1170`).
- Parked tasks are silently lost when the store is dropped
  (suspension/eviction); their continuations are not represented in the oplog.
- Effectively untested: only `ifs_update_inside_exported_function.rs` uses
  guest `spawn`, and it completes before the export returns.

Fix direction: drain tail work (use `run_concurrent_and_drain` or a
`poll_no_interesting_tasks` loop) before writing `AgentInvocationFinished`,
and define the semantics for tasks that never finish (cancel / trap / timeout).
Add replay + suspension tests with a spawned task doing durable work after the
export returns.

## G26 — No interruption/timeout reaches a parked P3 host future

**Resolved by T29** (interruption/timeout delivery for parked P3 host futures).

- Epoch interruption and fuel checks fire only at wasm execution checkpoints;
  wasmtime documents that they "do not assist in handling WebAssembly code
  blocked in a call to the host" (wasmtime `config.rs:681-697`). A guest
  parked in a P3 host future executes no wasm.
- The executor's interrupt signal is only checked between queued invocations
  (`invocation_loop.rs:451-462`); the in-progress `run_concurrent` future is
  not dropped/cancelled by the normal interrupt path. No `check_interrupt`
  calls exist under `durable_host/p3/`.
- There is **no max invocation wall-clock timeout** at all; wasmtime
  recommends wrapping the entire `run_concurrent` call.
- Eviction correctly refuses to evict a worker mid-invocation — so a worker
  stuck in a never-completing P3 host future stays resident indefinitely.
- If the future *is* dropped (e.g. task abort), `CallHandle` drop policies and
  `abandon_for_trap` leave a consistent incomplete `Start` — the machinery is
  there, but no executor path exercises it.

Fix direction: decide the cancellation delivery mechanism (timeout around
`run_concurrent`, cancellation token into P3 host ops, or interrupt-driven
future drop) and add tests for interrupting a worker blocked in a P3
HTTP/TCP/wait future.

## G27 — `stream`/`future`/`error-context` in agent signatures: accepted but unusable

**Resolved by T30** (upload-time rejection of `stream`/`future`/`error-context`
in agent schemas).

- `SchemaType` has `Future`/`Stream` variants ("parseable only; no semantics
  yet", `golem-schema/src/schema/schema_type.rs:218-228`) and upload
  validation does **not** reject them (`AgentTypeSchema::validate` only checks
  read-only/ephemeral) — so an agent type using `stream<u8>` uploads fine.
- But `SchemaValue` has **no** stream/future variants; JSON/CLI rendering
  explicitly reject them; wasm-rpc carries `schema-value-tree` and cannot
  marshal them. Result: invocation fails later as a confusing shape mismatch,
  and a stream-typed *result* is under-specified (no output validation in
  `decode_invoke_output`).
- `error-context` has no schema or value representation at all.

Fix direction: either add explicit upload-time rejection with a clear error,
or implement real value/marshalling support. Also add output-schema validation
after `decode_invoke_output`.

## G28 — Region manipulation (jump/revert/fork/snapshot) can strand Start/terminal pairs

**Resolved by T31** (orphan-terminal handling in concurrent replay) and
**T32** (guards for jump/revert/fork cut points).

The concurrent resolver matches `End`/`Cancelled` to `Start` purely in-memory
by start index, registered only when replay positionally consumes the `Start`
(`replay_state.rs:703-730`, `660-694`). Skipped/deleted regions are jumped
over without registering their Starts. Partial-region cases:

- **Start inside a skipped region, terminal outside → broken.** The orphan
  terminal is not an awaited entry; positional replay hits it and fails with
  an unexpected-entry error.
- **Start outside, terminal inside a skipped region → incomplete.** Safe calls
  re-execute; non-idempotent calls hard-error (fail-closed, but unguarded).
- `set_oplog_index` (jump) has **no in-flight durable call guard** — unlike
  `mark_end_operation`, which refuses while calls are in flight
  (`v1x.rs:459-475`). This is the riskiest entry point.
- External fork (`worker_fork.rs:455-493`) and revert
  (`worker/mod.rs:2282-2304`) validate nothing about cut points vs active
  durable scopes/atomic regions. (In-guest `fork()` has bespoke repair and is
  fine.)
- **Snapshot-based update/recovery** sets a skipped region over the
  pre-snapshot prefix; a post-snapshot terminal referencing a pre-snapshot
  Start is the same stranded-terminal failure. No invariant was found
  forbidding open durable calls spanning a snapshot boundary.
- Test gap: `replay_skips_deleted_regions_fuzz` only deletes whole
  `Start`/`End` pairs and never uses `Cancelled`; no revert/fork/jump test
  combines with P3 async calls.

Fix direction: guard jump/fork/revert cut points against in-flight/spanning
durable calls (or make orphan terminals explicitly skippable when their
`start_index` lies in a skipped/deleted region); add partial-region replay
unit tests and P3 integration tests.

(Oplog **archival/layering** itself is safe: transfers preserve absolute
indices across layers and drop prefixes only after append; multilayer reads
merge by index. Only the logical-region features above are at risk.)

## G29 — Oplog processor plugins: structurally compatible, two caveats

**Resolved by T33** (async ABI for the oplog-processor export + P3-entry
coverage).

WIT `oplog-entry` includes `start`/`end`/`cancelled`
(`golem-oplog.wit:634-643`, `730-739`) and all conversions handle them; P3
payloads surface as `typed-schema-value` (not named WIT variants — document
this contract). Caveats:

- `LoweredCall::ProcessOplogEntries` calls the export directly
  (`guest.call_process(&mut *store, ...)`) instead of via `run_concurrent`
  (`worker/invocation.rs:277-290`) — a P3-async oplog-processor export would
  misbehave; the current test component avoids it with guest-side `block_on`.
- No test feeds a plugin entries from a source worker doing P3 host calls
  (existing tests use counters workers). `encode_public_typed_schema_value`
  has a panicking `expect` if a payload is unencodable.

## G30 — Debugging service: concurrent-entry edge cases unhandled/untested

**Resolved by T34** (debugging-service concurrent-entry fixes + tests).

- Default invocation-boundary playback is safe. With
  `ensure_invocation_boundary=false`, a target between a `Start` and its
  terminal yields `Incomplete` (no hang), which may trigger **live repair /
  re-execution in debug mode** while `DebugOplog::add` and
  `add_start_with_reserved_raw_payload` return `OplogIndex::NONE`
  (`debug_oplog.rs:71-93`) — semantically dubious; may re-execute side
  effects during debugging.
- Playback overrides apply arbitrary entries by index with no Start/End
  pairing validation — can corrupt resolver state.
- Debug fork with arbitrary cutoffs inherits the G28 stranding issues.
- Zero test coverage for debug sessions over workers with concurrent
  Start/End entries or P3 host calls.

## G31 — Mixed-ABI: suspend heuristics ignore pending P3 work

**Resolved by T35** (suspend heuristics aware of pending P3 work).

Wasmtime integrates pending P2 blocking host calls into the same per-store
event loop as P3 accessor tasks (`poll_and_block` parks the future into
`ConcurrentState::futures`) — **no inherent deadlock** for well-behaved calls;
only a host future doing long synchronous work inside one `poll` can starve
the loop (documented wasmtime limitation). The real Golem-specific risk:

- P2 `poll`'s suspend heuristics (`io/poll.rs:111-138` promise-only suspend,
  `:237-274` sleep suspend) inspect only the P2 pollables passed to that call.
  If unrelated P3 host futures are pending in the same instance, the worker
  can be suspended even though a P3 completion is imminent. No guard exists.
- `println!`/stdout writes are short-circuited to log events and cannot block
  P3 progress (fine).
- The two durability APIs (legacy P2 `Durability`, P3 `CallHandle`) do share
  begin/end indices, durable scopes, retry point, and atomic-region state —
  coordinated by design, with call-owned trap context as the overlap
  mitigation; overlap-safety of every legacy P2 path is not proven.

Fix direction: make the suspend heuristics aware of pending P3
tasks/futures (e.g. wasmtime `poll_no_interesting_tasks`-style check), and
add a mixed-ABI regression test (slow P3 send + P2 poll/sleep in parallel).

## G32 — Embedders/plugins: consistent, minus two follow-ups

**Resolved by T36** (OTLP-exporter smoke test added; the cross-ABI
library-plugin fixture/policy item was found obsolete).

All wasmtime Config/linker sites are consistent (executor, debugging service,
extraction, compilation service, CLI, test fixtures); the builtin OTLP
exporter is already a mixed P2+P3 component (imports P3 http/clocks + P2
cli/io) served by the executor linker. Follow-ups:

- No smoke test loads/runs `plugins/otlp-exporter.wasm` under the current
  linker — add one.
- Library-plugin WAC composition (`cli/golem-cli/src/composition.rs`) connects
  plugs by WIT subtype match: a P2-shaped library plugin will silently fail to
  connect to a P3-shaped socket import. Needs an explicit fixture/policy for
  cross-ABI library plugins.

## G33 — Scala and MoonBit SDKs: WIT synced, runtimes NOT migrated

**Still open** — tracked by T43/T44 (Scala), T45/T46 (MoonBit), T47 (TS,
blocked on wasm-rquickjs).

- **Scala**: `sdks/scala/wit` matches root, but the embedded base image
  `agent_guest.wasm` still imports `wasi:io/poll@0.2.9` /
  `wall-clock@0.2.9` and exports `golem:agent/guest@1.5.0` (pre-migration!),
  and the JS runtime facades use `subscribe()`/`pollable.block()` throughout
  (`HostApi.scala`, `RemoteAgentClient.scala`). Full runtime port +
  base-image regeneration + integration test run needed.
- **MoonBit**: WIT synced; AGENTS.md claims a working state but generated and
  hand-written code still contains `wasi:io/poll@0.2.6`, stream `subscribe`,
  pollable-based promises/websocket, and mixed wit-bindgen versions (0.58.0 +
  0.42.1 remnants). Needs regeneration + API port; no built example artifact
  present to verify.
- **TS**: WIT synced, runtime blocked on wasm-rquickjs (known).
- No executor/integration test exercises Scala- or MoonBit-built components.

## G34 — Memory accounting/backpressure under concurrent host tasks

**Resolved by T37** (memory/backpressure hardening for P3 host tasks).

- P3 HTTP body and TCP receive are demand-driven with a one-pending-demand
  invariant (TCP additionally uses a capacity-1 channel) — practically
  bounded, though the demand channels are technically unbounded types.
- **P3 stdout/stderr capture buffers the whole written stream** through a
  oneshot (`p3/cli.rs:280-300`) with no byte cap — a guest writing a huge
  stream accumulates it all in host memory.
- Host-side buffers (body chunks, capture buffers, oplog serialization) are
  not charged to the worker's memory grant; they only show up indirectly in
  process RSS.
- Eviction classification (`worker/mod.rs:1600-1624`) checks loop state but
  not wasmtime store task quiescence — confirm `waiting_for_command=true`
  cannot coincide with live store-spawned P3 tasks (ties into G25).
- No test covers memory limits under concurrent P3 streaming.

## G35 — Outgoing-HTTP spans replay positionally; unsound under concurrent P3 sends

**Still open** — tracked by T48 (claim-safe span records for concurrent P3
sends).

**Symptom (latent).** The `outgoing-http-request` invocation-context span added
for the P3 `client::send` path (T04) is recorded as positional
`StartSpan`/`FinishSpan` oplog entries, consumed on replay with
`get_oplog_entry!` (read-next semantics), while the durable call records
themselves are claim/identity-based. The live write sequence
[durable-scope `Start`] → `StartSpan` → [host-call `Start`] spans multiple
awaited oplog appends inside `execute_access_start`, and accessor
(`HostWithStore`) host calls run genuinely concurrently — so two overlapping
sends can interleave these entries in the recorded oplog. On replay, each
send's request builder consumes "the next" `StartSpan` positionally: an
interleaving that differs from the recorded one makes a send consume a
sibling's `StartSpan` (wrong span id → wrong re-derived `traceparent` on an
incomplete-replay re-execution) or fail outright with an
unexpected-oplog-entry error. The same applies to the `FinishSpan` consumed
after the consume-body terminal and to the deferred
`DropEvent::FinishSpan` drain for responses dropped unconsumed.

**Contrast with RPC.** The P3 RPC span path (`create_invocation_span` in
`durable_host/wasm_rpc/mod.rs`) writes the same positional entries but runs
inside `&mut self` host calls, which hold the store exclusively for the whole
call — its [scope `Start`][`StartSpan`][call `Start`] sequence cannot
interleave with sibling host calls, and it relies only on deterministic guest
initiation order (the same invariant the `Start`-claim machinery documents in
`replay_state.rs`). The accessor-based HTTP send has no such atomicity, so
this gap is primarily about the HTTP path; any future accessor-based span
user inherits it.

**Evidence.** `durable_host/p3/http.rs` (send request builder, consume-body
task span finish, response-drop deferred finish);
`durable_host/concurrent.rs` (`start_span_access`, `finish_span_access`,
`DropEvent::FinishSpan`, the request-builder hook in `execute_access_start`);
positional replay: `durable_host/mod.rs::start_child_span` /
`get_oplog_entry!`.

**Fix direction (pick one, design first).**
1. *Atomic batch append*: extend the oplog append path so the scope `Start`,
   `StartSpan`, and host-call `Start` are appended with no awaits in between
   (like `add_start_with_reserved_payload` guarantees for a single `Start`),
   making each send's entries contiguous; then positional consumption in
   deterministic initiation order is sound again.
2. *Identity-keyed spans*: attach the owning call to the span entries (e.g. a
   `start_index` field on `StartSpan`/`FinishSpan`, or record the span id in
   the call's `Start` request payload) and resolve them through the concurrent
   resolver instead of positionally.
3. *Derived spans, no extra entries*: derive the span deterministically from
   the claimed host-call `Start` (deterministic span id, timestamps from the
   `Start`/terminal entries) and stop writing separate span entries on this
   path — changes the public-oplog/OTLP surface, needs an explicit decision.

**Interaction.** T09/T40 acceptance runs the parallel streaming replay tests
(`wasi::oplog_replay_after_parallel_streaming_http_reads` and friends), which
exercise overlapping sends with restarts — they will hit this once they pass
their primary G2 blockers, so this should be fixed before those are used as
acceptance gates.

## Investigated and found sound (no gap raised)

- **Invocation serialization**: exactly one invocation loop per worker; the
  store mutex is held across each whole invocation; all export-call paths
  (initialize, manual/auto snapshot, load-snapshot recovery, resume, oplog
  processor) go through the same queue/loop. No overlapping-export path found.
  (Leftover P3 resources across invocations remain a G25 concern.)
- **Oplog archival/multilayer storage**: index-stable, append-then-drop,
  merge-by-index reads; existing tests cover layer movement.
- **Wasmtime event loop for mixed ABI**: no inherent P2-blocks-P3 deadlock
  (see G31 for the heuristic caveat).
- **Fuel/epoch config parity** across executor, debugging service,
  compilation service, extraction; fuel accounting unaffected by host tasks.
