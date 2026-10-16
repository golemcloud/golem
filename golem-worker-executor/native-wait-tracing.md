# Native wait tracing

`golem::native_wait=debug` enables native-wait spans and structured events through
the existing `golem-common` tracing setup. For Tempo, configure Golem's existing
OTLP exporter to send to the collector that forwards traces to Tempo, and include
this target at debug level in `GOLEM_OTLP_FILTER`. `RUST_LOG` controls log output
separately. There is no additional exporter, transport, store or deployed collector.

- `execution_window` spans belong to the actual concurrent-agent permit. They
  record `physical_release=true` only after the existing physical release path.
  The span is never entered across an await. Its stored `TraceOrigin` is only an
  OpenTelemetry context, so retained monthly targets do not hold the span open.
- `native_wait` spans identify one actual future. They carry the operation,
  invocation key, agent, stream rep or child Start, and the active monthly window
  identity where available. IDs are process-local attributes, not durable inputs,
  billing keys or enforcement authority. All-off, closed and replaced monthly
  targets are not synthesized; absent window/capacity fields mean unavailable.
  `was_pending=true` records its first actual Pending. `state` records only its
  Returned or Dropped terminal, once, rather than exporting duplicate state keys.
- `native_wait_transition` spans are short, parentless spans linked to the wait
  through `TraceOrigin`. They carry the operation and owner attributes themselves.
  A first real Pending, Returned or Dropped observation therefore has a completed
  span without waiting for the operation span to close. Returned includes native
  errors. Dropped means future destruction, not a particular cancellation cause.
- `execution_window_observation` spans record the actual monthly acceptance
  result and proposed revision, period and fuel generation against that window.
  Capacity attributes are read-only published snapshots. Tracing never settles
  usage, borrows fuel, advances the accounting period or refreshes policy.

`rpc_result_wait` observes the task-result future after its existing lock is acquired.
`p3_udp_receive` observes only native receive. Replay, durable completion, delivery and
interruption priorities remain outside tracing. The RPC and UDP Worker test observers
are removed; tests collect these operations through the same structured subscriber.

For UDP, an enabled diagnostic reads the actual bound local address and records
`socket_rep` and `local_address` on the existing `native_wait` span. Its `socket_binding`
event carries the same fields and `wait_id`. Missing binding information stays absent;
it does not bind a socket or change receive's result. This address is diagnostic only,
not a guest value, durable identity or reconnection instruction.

WebSocket handshakes, reconnect-pool acquisition and the first timed frame wait
also use `native_wait`. Their operation labels distinguish initial, accessor and
direct reconnect paths. The wrapper stays at each original future, outside replay,
connection publication and durable completion. Timed receive observes only its
first frame future, after the reader lock, with the existing timeout unchanged.
The passive Worker observer families and reader-order gates are removed.
`websocket_receive` observes untimed native frame wait; `websocket_timed_reader_lock`
observes the actual timed reader-lock future. Their `connection_binding` events record
`connection_rep` and `wait_id` on the existing wait span. Returned from the lock future
means the lock was acquired, not that a frame arrived.

The contention fixture creates its own ordinary guest promise. Tests complete it only
after the untimed frame wait reports actual Pending, then require timed lock Pending on
the same connection. The committed create response supplies the promise ID; its owner
restriction is unchanged. This is test-side orchestration at an existing dependency,
not a runtime-only gate or a change to WebSocket recovery.

Tempo normally receives spans after they close, subject to the configured filter,
sampling, batching and collector delivery. The long wait/window spans are not a
live dashboard. The short Pending transition can arrive while the wait remains
open, but tracing does not promise a delivery deadline and does not force a flush.

Pollable bindings are structured numeric index/rep events on their wait span.
Their order and duplicates are retained. Native wrappers forward exactly the
underlying poll and waker. They do not add a readiness gate or an async boundary.
The standard tracing OpenTelemetry adapter encodes unsigned integer fields as
decimal strings in exported attributes. Test-side tracing visitors still receive
the original typed unsigned values; no additional attribute mapping is installed.

Tests install a structured subscriber layer before the existing global tracing
initialization. Its receipt ledger and channels live only in the test binary.
They do not exist in deployed executor code or control native progress.
