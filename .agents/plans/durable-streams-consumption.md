# External Durable Streams: read and append

## Scope and baseline

Implement DS-7 and DS-7a–d on PR #3877, using the existing DS-8/DS-9 runtime.
The protocol baseline is durable-streams/durable-streams PROTOCOL.md at
461b40267aabd644558f9b19dbb9507dd5f691cf. Implement external JSON/byte reads,
catch-up, long-poll and SSE, idempotent appends, and closure in Rust, TypeScript,
Scala and MoonBit. This is a client, not a new Durable Streams server.

Golem forks copy the retained oplog and reconstruct ordinary reader/writer
state. They do not create an external DS fork, change URLs, or allocate new
producer identities or epochs. Explicit administration of external streams
(creation, deletion, DS-level forking, subscriptions) is outside this change.

## Two finite, stateless host operations

Add two asynchronous operations to golem:agent/host:

1. `read-durable-stream-batch`: URL, checkpoint, transport, content mode,
   optional secret authentication and bounded request options -> complete
   payload, content type, next checkpoint, up-to-date and closed flags, or a
   typed error.
2. `append-durable-stream-batch`: URL, content type, payload, producer ID/epoch/
   sequence, close flag, optional secret authentication and bounded request
   options -> acknowledged producer state, optional next offset and closed flag, or a
   typed error.

The checkpoint contains an opaque offset and an optional transport cursor.
Never derive offsets from item counts, bytes, Golem stream offsets or socket
state. The input producer tuple and append body are immutable for one attempt.

The host implements framing, HTTP transport, authentication, response validation
and error classification once. SDKs serialize/validate application values and
own small deterministic reader/writer state machines. There is no new external
source kind, external-cursor stream-session record, ingestion daemon, host-owned
progress resource, producer service, or fork-specific metadata.

Host operations add typed request/response variants and host-function pairs to
the existing oplog payload registry. They do not add top-level oplog entries.
Update WIT, linker implementations, generated bindings and public-oplog payload
exposure together. Retain strict request identity and delivery ordering.

## Read protocol and durability

Catch-up/long-poll returns only a complete HTTP response body with a valid
server-returned checkpoint. SSE returns only a completed data/control pair (or
a control-only checkpoint), honoring multiline events and standard base64 for
binary data. Discard partial bodies and data without a following control event;
retry the original offset. Never apply HTTP Range-based recovery to DS reads.

Default to offset `-1`. Resolve `now` once with an initial catch-up GET; that
returns content type and a concrete tail without data. Once this result is
delivered, all later calls use the concrete offset, including after recovery.
The initial catch-up request also supplies metadata, so no HEAD host operation
is needed. Echo Stream-Cursor separately from Stream-Next-Offset.

Each SSE call opens a connection, returns its first completed checkpoint and
closes the connection. Later events are re-fetched from that checkpoint; no SSE
connection is required to survive between host calls.

Use ReadRemote and the existing cancellable DurableCallSession. Resolve replay
before any network operation or secret retrieval. Persist payload and checkpoint
together in End. Complete at the existing accessor terminal boundary; do not
advance host progress after complete_access, which merely arms delivery.

Completed replay returns recorded results and makes no external request.
Markerless End uses existing replay-tail delivery. Discarded completion does not
advance SDK state. Incomplete Start repeats the same read only after the existing
framework permits live repair. Interruptions abandon the call; guest cancellation
uses the normal cancellation machinery rather than inventing another terminal.

Use bounded body accumulation with a configurable safety limit. Do not truncate
or split a remote batch and invent a checkpoint to fit a memory limit. An
oversized batch is a typed error at the same offset; document that retrying with
the same limit cannot make progress. Respect existing HTTP call quotas, network
permissions and memory accounting instead of creating a separate policy system.

## SDK reader state and native streams

Each reader stores its descriptor, current checkpoint, pending batch with its
next checkpoint, item/byte index, closure and retry state. Permit one read per
reader at a time. Install a delivered batch before the next await. Decode and
yield on demand, promote its checkpoint after draining it, then fetch again.
Do not prefetch beyond one batch. Separate readers may run concurrently.

JSON GET bodies are arrays of logical messages. Decode deterministically using
each SDK's existing schema facilities, preserving integer precision. Byte mode
is a byte sequence, not a sequence of original append chunks. Transport batch
boundaries must not become application message boundaries.

Expose ordinary AgentStream-compatible values for local iteration, nested/output
streams and RPC inputs. Follow existing affine ownership and backpressure. Add
only necessary SDK source-adapter plumbing (particularly for Rust). Local errors
are read failures; forwarded producer errors fail the active producer/session,
not clean EOF. Recoverable errors require an explicitly result-valued stream,
consistent with existing native stream contracts.

Reader close prevents new pulls and releases the pending HTTP operation when
the language/runtime permits cancellation. Do not promise interruption of an
arbitrary pending native async import; request deadlines bound that interval.
Reconstruction recreates buffers from durable results and deterministic guest
execution. Any supported explicit snapshot state includes the pending remainder
and index, not just an offset. Preserve existing restrictions on snapshots of
active native streams; never serialize a socket or resource-table handle.

## Waiting and retries

Start with catch-up; use long-poll for live tailing and support SSE through the
same finite host operation. Bound each attempt. After an empty live response,
SDKs can wait using existing durable timers before the next request. This lets
the worker unload between requests without a new host wait/reconnect subsystem.
An active long-poll remains resident until completion or its bounded timeout;
idle polling introduces latency. Do not claim active HTTP awaits automatically
suspend. Suspending within a network attempt is an optimization, not a hidden
requirement of this implementation.

The host performs one HTTP attempt and at most one secret-revision fetch per
call. Persist remote failures as typed results, rather than trapping for a
transient transport or secret-service failure. The SDK owns the retry loop for
transport failures, 429 and appropriate 5xx responses, using bounded exponential
backoff, Retry-After and existing durable clocks/timers. A configured reconnect
budget survives reconstruction and does not reset on process restart. Empty
success/204 is not a failure. Separate per-attempt timeout from the failure/
reconnect budget. Keep long waits outside custom durable or atomic scopes.

## Append protocol and durability

SDK writers retain producer ID, epoch, next sequence and one pending immutable
append. Generate an ID once using existing durable identity/randomness or accept
an explicit ID. Assign and store the tuple/body/close flag before awaiting the
host call. Only one uncertain append per writer; independent writers may overlap.

For JSON, the host receives individually encoded JSON values, validates them as
complete values and frames the outer array itself. A single array value is
therefore wrapped rather than flattened into multiple messages. Byte payloads
are sent exactly as supplied with matching content type. Reject empty appends;
an empty body is allowed only for close-only. Append-and-close is one request.

Use ordinary WriteRemote DurableCallSession with Cancellable drop policy, exact
request identity and the standard completion-delivery boundary. Append Start
before sending; retain the framework's existing commit semantics rather than
adding a forced commit. SDK tuple/body assignment derives from durable inputs
and reconstructs identically even if that Start was not committed before loss.
Store stable secret identity/revision, never plaintext, in the request. Completed replay
returns the receipt without POST. An incomplete write is retried with identical
producer tuple, body and close flag under the existing idempotence policy.
The default Golem idempotence mode permits this; explicitly disabled mode retains
its existing fail-closed recovery semantics. Do not mutate that global mode,
misclassify writes as reads, or add a DS-specific retry-policy override.
In particular, an interrupt/crash during a POST with idempotence disabled can
make recovery fail; guest cancellation records a terminal and closes its scope.

No SDK custom-durability wrapper or generic wasi-http append is needed. The
finite direct host write avoids the generic streaming-HTTP batched-scope recovery
path. Validate concurrent writer completion order with actual guest tests.

On acknowledgement, the SDK advances the sequence once and clears pending state.
If a call is cancelled or its outcome is uncertain, retain the pending tuple and
body; resolve that exact request before accepting different data. Dropping a
writer does not undo an external append. A host result being recorded is not
permission to advance a host-owned sequence.

Validate all three producer headers together; epoch and sequence are nonnegative
integers at most 2^53-1. A new epoch starts at zero. Close-only also receives and
consumes a sequence, and retries retain it. Receipt epoch must equal the submitted
epoch. Acknowledged sequence must be at least the submitted sequence: less or
missing is a protocol error; greater is valid protocol but signals a diverged
writer and is surfaced as a typed error for this non-pipelining SDK. On equality
advance to submitted sequence plus one, never acknowledged sequence plus one.
Handle 200 append and 204 success. A 200 append requires a concrete next offset;
a 204 acknowledgement may omit it, so the receipt uses an optional offset.
Validate any supplied offset, but advance the writer from the acknowledged tuple,
not from offset availability. Do not expose a duplicate boolean inferred from
HTTP status: 204 also acknowledges a newly accepted close-only request.
Distinguish 409 sequence gap, 409 closed stream and 409 content-type conflict.
403 with Producer-Epoch is fencing, not generic auth failure. Do not automatically
steal an epoch, renumber uncertain data, or split an uncertain 413 request.
Recover sequence gaps only from retained requests assigned to the missing
sequence; otherwise surface a typed conflict. Normal serialized writer operation
does not create gaps.

Exactly-once external effects depend on the server atomically persisting producer
state and data and retaining deduplication state. The protocol may deduplicate a
tuple without comparing payloads. In particular, divergent Golem forks using the
same tuple do not automatically become independent external writers. Document
these limits and do not imply Golem can strengthen the peer's guarantees. Users
can explicitly construct a new writer with a different producer ID or a higher
epoch and sequence zero; never do this implicitly or abandon uncertain data.

## Authentication and protocol corrections

Both operations accept a secret capability and resolve its pinned revision only
on the live wire path. Require SecretVerb::Reveal and the existing entity
secret_keys_revealable restriction, plus network permissions. Hold alone is not
authorization to send secret material to a third party. The secret must contain
a string; use it as Authorization: Bearer <value>, without revealing it to the
guest. Other secret types are rejected before network I/O.
No reveal to the SDK or plaintext Authorization header in oplog request payloads.
Use the existing outbound network authorization target and quota accounting.
Reject URL userinfo; accept HTTPS, with HTTP permitted only for localhost or
literal loopback addresses for development/testing. This is an explicit DS
client rule, not a claim that generic wasi-http already enforces TLS.
Do not follow redirects automatically or
forward credentials to a different origin. Errors record sanitized status and
protocol headers, not arbitrary response bodies or secret values.

Protocol corrections superseding ticket wording:

- 410 is retention loss/deletion, not EOF; 404 is a missing-stream error.
- Stream-Up-To-Date, empty payload and 204 do not imply closure.
- Deliver the final payload before observing its closed marker as EOF.
- Raw byte streams do not preserve append boundaries.
- SSE payload is not checkpointed until its control event.
- Never blindly resynchronize a producer sequence by changing a pending payload's
  assigned sequence.

## Forks, compatibility and scope

Golem fork/revert uses existing exact retained-prefix replay and live repair.
Both branches retain the same external URLs, checkpoints and producer identity.
No DS-fork calls or new producer epochs occur implicitly. Test cuts before End,
after End before delivery, within SDK buffers and during uncertain appends.

The diagnostic packed-byte output fork failure is a separate shared-stream
composition limitation, not a failure of external reads or appends. Keep any
affected downstream acceptance failure visible; do not bypass it by changing
byte schemas or introducing DS-specific stream journals. Do not claim complete
composition coverage while a required case fails.

Make contract changes directly with all in-tree consumers updated. Do not add
legacy WIT aliases, old payload parsers or compatibility shims.

## Implementation and verification

1. Freeze WIT and serializable request/response types; synchronize all copies.
2. Implement the shared host protocol and two durable host operations, including
   auth, quotas, limits and typed outcomes. One DurableCallSession per operation,
   with AccessClaimOptions.request_identity; secret fetch occurs inside the live
   action, not as a nested durable call. One executor-configured batch-size cap
   bounds payloads. Test parsing/framing independently.
3. Implement Rust, TypeScript, Scala and MoonBit SDK readers/writers with identical
   protocol vectors and language-native source/error/cancellation handling.
4. Build affected bindings, SDK runtimes/templates and targeted test components.
5. Verify actual guest delivery, cancellation, restart, zero-network completed
   replay, concurrent calls, buffer recovery, timers/unloading and normal forks.
6. Run third-party reference-server CLI integration tests, then bounded bug-finder
   review, fix accepted findings, and final oracle code review. Do not report
   design approval or passing diagnostic tests as implemented-feature proof.

Important discriminating tests: nested JSON arrays and large integers; asymmetric
bytes; fragmented/multiline/base64 SSE; lost control event; now fixed after
delivery; missing headers; 410 versus closure; final nonempty closed batch;
oversized bodies with no advancement; discarded read and append completions;
crash before/after remote append commit and before/after End/delivery; duplicate
POST attempts counted separately from external mutations; stable tuples and exact
bytes; cancelled append followed by different data; 409/403/413 outcomes; multiple
readers/writers in both completion orders; no token in raw/public oplogs; ordinary
forks make no DS-fork requests.

Use in-process HTTP fixtures for protocol/runtime tests. External reference
server processes belong in CLI integration tests or sanctioned framework
dependencies. Start with targeted tests, preserve failing evidence, then broaden
checks for the shared WIT/runtime impact. Update executor durability explanations
and SDK documentation alongside implementation changes.
