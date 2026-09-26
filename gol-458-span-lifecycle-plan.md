# Durable span lifecycle and the Golem 1.6 OTLP exporter

Design and implementation plan, 2026-09-24.

## 1. Outcome

Replace standalone positional span oplog records with span transitions carried by
ordinary durable operations. Update the built-in OTLP exporter for this contract
and complete the Golem 1.6 telemetry inventory and implementation required by
[GOL-458](https://linear.app/golem-cloud/issue/GOL-458/update-the-otlp-plugin-for-golem-16-features)
in the same changeset.

A span's lifetime is independent of the lifetime of the durable operation that
opens it. One operation may open a span and a later operation may close it by
identity. Ordinary RPC spans can still open and close within one call.

The general replay cursor must not understand span creation, attributes, or
finishing. It claims and resolves ordinary durable calls. The owning host
operation reconstructs the invocation context; the OTLP plugin interprets the
recorded span transitions for export.

This document is an implementation plan, not a claim that the design has already
been implemented or verified. The agreed direction is settled; exact Rust names,
scheduling integration, and the remaining decisions listed in section 10 must
be resolved before or proved by the vertical slice below. Exporter-visible span
transitions are structured fields on raw oplog entries, not opaque call payloads.

### References

- [PR #3961: deferred RPC span-tail replay ordering](https://github.com/golemcloud/golem/pull/3961)
- [Design discussion](https://ampcode.com/threads/T-01a0d430-1494-759f-b1b3-720ab1243661)
- GOL-581 execution plan supplied at `~/Downloads/gol-581-execution-plan.md`.
- [GOL-581 plan discussion](https://ampcode.com/threads/T-01a0cfef-461c-713f-859d-7d663ac4c6a6)

## 2. Why change the recording contract

The current RPC path can record:

```text
Start RPC
StartSpan RPC
Start timer
End timer
End RPC
FinishSpan RPC
CompletionDelivered RPC
```

Replay can resolve the RPC's successful `End` through completion-marker
lookahead while the positional cursor still points at the timer's `Start`.
The RPC continuation then calls `finish_span_access`, which performs an ordinary
positional read and consumes the timer entry before rejecting its type.

The lookahead is intentional. Host task scheduling need not recur in the same
order, while guest-visible completion delivery does. Resolving a result is not
permission to consume the next positional entry.

PR #3961 adds span-specific terminal-tail handling to the call session and replay
cursor. That addresses the immediate ordering problem but puts RPC tracing
knowledge in general replay machinery. The alternative discussed earlier was a
generic post-terminal reader. This plan instead removes the redundant positional
span suffix and its separate ownership obligation.

Embedding also eliminates the `Start → StartSpan` and `End → FinishSpan` crash
gaps for converted operations. It does not eliminate ordinary incomplete calls,
completion-delivery boundaries, or the need for strict replay matching.

## 3. Settled design constraints

1. **A span is not a durable scope.** Do not keep a durable call or atomic lease
   open merely because a resource or user span remains open. Span lifetime alone
   must not block checkpoints or change retry eligibility.
2. **Span transitions have durable owners.** Creation, mutation, and closure
   belong to identifiable host operations, not anonymous cleanup appended after
   some unrelated terminal.
3. **There is no positional span replay protocol.** Remove `StartSpan`,
   `FinishSpan`, and `SetSpanAttribute` oplog variants after converting all
   producers and consumers. Retain the guest API behavior, not the old records.
4. **Guest observations remain deterministic.** Preserve recorded span identity,
   start time, attribute lookup, current-context behavior, and trace propagation.
5. **Metadata lookup is not state application.** A prefetched terminal must not
   mutate guest-visible invocation context early.
6. **The resident runtime remains disposable.** Suspension, crash, restart, and
   Store destruction do not fabricate successful span closures.
7. **No compatibility implementation.** Replace the in-tree contract directly,
   including SDK bindings, generated schemas, fixtures, and the plugin artifact.
   Do not retain old variants, fallback parsing, or dual emission.
8. **No recursive instrumentation.** Span-management and internal cleanup
   operations must not automatically create further spans.
9. **Processor-instance continuity is out of scope.** Retain the stateful,
   best-effort OTLP exporter. Exactly-once source batch delivery does not promise
   the same processor instance for subsequent batches. Accept the pre-existing
   telemetry loss around instance switches, tracked separately in
   [GOL-667](https://linear.app/golem-cloud/issue/GOL-667/preserve-otlp-span-assembly-across-oplog-processor-instance-switches).
   Do not duplicate complete span descriptors on close, change plugin delivery
   guarantees, or implement routing/state transfer in this changeset.

## 4. Oplog representation

### 4.1 Transitions refer to spans, not necessarily the matching call

The conceptual shape is:

```text
Start(operation A, optional span_started = S)
End(operation A)                                  # S remains open

Start(operation B, request identifies the lifecycle operation/resource)
End(operation B, optional span_finished = S)       # closes S
```

An ordinary call can use the shorter lifetime:

```text
Start(rpc.invoke, span_started = S)
End(rpc.invoke, span_finished = S)
```

`Cancelled` must also be able to carry terminal span transitions. A trap leaves
the call incomplete; it must not be converted into a cancellation for telemetry.

Unlike a strictly call-owned-span model, a terminal needs an explicit identity
for the span it closes. The terminal's `start_index` identifies the operation,
not necessarily the operation that opened the span.

Opening, closing, and applied attribute transitions must be structured fields on
the raw entries, alongside `request`, `response`, and `partial`. The oplog
processor receives raw payload references, which can contain opaque bytes or
refer to external storage; it cannot be expected to decode typed host requests
or fetch their payloads to reconstruct spans. Expose the same structured data
through public oplog projections.

Opening metadata must be available when its immutable `Start` is constructed,
including request-less scope starts. Extend the session's start-construction API
to accept that metadata; do not append `Start` first and try to enrich it later.
Live may prepare an ID beforehand or construct it using a reserved start index.
Evaluate the existing indexed-start construction path for P3 rather than
mandating random IDs. Neither choice puts span metadata into request matching.

### 4.2 Metadata to retain

An opening descriptor must preserve the information required for reconstruction
and export:

- Span identity.
- Explicit origin trace identity and trace state, retained with the span rather
  than recovered from whichever invocation happens to close it.
- A same-trace parent identity and links carrying full trace/span context,
  including trace state. Cross-trace relationships are links, not parents.
- Recorded start timestamp. Guest `span.started-at` must not use replay time.
- Name and initial attributes.
- Any lifetime/ownership distinction required to separate invocation-owned spans
  from spans allowed to remain open across invocations.

A closing transition identifies the span and its recorded completion time.
Before implementing the schema, decide whether that time is the containing
terminal's timestamp or an explicit captured event time. Deferred cleanup must
not accidentally measure arbitrary queue delay as part of the span without a
documented decision. The Oracle passes recommended different policies; neither
recommendation is treated as settled by this plan.

Terminal metadata may include final attributes and status/reason when required.
Do not equate every `End` with application success: a completed call can return a
recorded error response. Define cancellation, denial, and failure mappings
explicitly rather than inferring them solely from the terminal variant.

Keep representation size proportional to actual requirements. Use optional
single transitions where sufficient. Use a collection only where a real host
operation must atomically apply multiple changes, such as `set-attributes`.
Do not introduce a general event language or extensible plugin protocol.

### 4.3 Attribute mutations

Convert guest `set-attribute` and `set-attributes` into normal durable local
operations. Their requests identify the target through its durable resource or
creation-operation identity and contain the requested changes. The resource's
recorded span ID belongs to tracing metadata, not replay request matching. Their
successful terminals carry structured applied changes, including the recorded
span identity, directly on the raw entry for the exporter.

Do not make the exporter treat a mutation request's `Start` as proof that the
mutation succeeded. Duplicating attribute changes between the request payload
and structured terminal metadata is justified: the former supports execution
and matching, while the latter makes the applied transition visible to raw-oplog
consumers. Do not make span IDs part of the matching request to avoid that
duplication.

Preserve current validation and mutation semantics, including any behavior of a
partially failing multi-attribute operation; inspect that behavior before choosing
an all-or-nothing representation.

### 4.4 Operation identity and classification

**Span IDs do not participate in request matching.** Use the existing typed
durable host-function request/response and claim machinery to identify the
operation. Opening, closing, and attribute-transition metadata are separate from
that identity. Do not add trace/span IDs, timestamps, parents, or links to request
equality, claim discriminators, or replay-routing rules.

Replay first claims the operation using its execution identity, then restores
the span metadata from the claimed record. There is no need to generate or know
the recorded span ID in order to find that record.

New resource cleanup and guest span-management operations still need correct
caller/target association. Use their durable resource or creation-operation
identity where target matching is required, not the OTLP span ID or a Wasmtime
resource-table index. This is an execution identity, not a new span-aware claim
protocol. Preserve the existing owner and ordering rules; do not use tracing
metadata to compensate for missing operation ownership.

Checking that a resolved transition refers to the span owned by the operation
is a host-layer consistency check after matching, not a way to search for a
different matching call. The cursor need not interpret that relationship.

The session/resolver may transport recorded transition data to its owning host
continuation if needed. Transporting data is not permission to interpret or
apply tracing effects in the cursor, nor a requirement that terminal metadata
be export-only.

Choose local durability classifications according to the actual effect and
existing retry rules. Do not classify a span-only close as a remote write just
because its resource originally represented a remote connection.

Opening metadata generated live is recorded before propagation or guest
observation. Replay restores it from the claimed record rather than generating
another random identity. Preserve fork/revert and atomic identity rules; this
change must not redesign RPC idempotency keys.

## 5. Producer conversion

### 5.1 RPC connections

Current behavior opens a connection span in `wasm-rpc.new` and finishes it when
the RPC resource is dropped. Multiple invocations can share that resource.

Attach the opening descriptor to the appropriate connection-creation operation.
Make guest resource drop a short durable local operation whose terminal closes
the connection span. Reconstruct the resource/span relationship on replay even
when connection creation returns a recorded result.

Audit all constructor branches, including ephemeral logical proxies, failed
creation, and early validation failures. An opening record must not leave an
unaccounted span when no resource is returned. Do not force a connection-lifetime
span to close at the constructor's `End`.

### 5.2 Synchronous and ordinary async RPC

Attach the invocation span to the invocation `Start` and close it on the result
`End` or logical cancellation. Propagate the recorded span context to the callee.

For async RPC, the existing call may start before `future-invoke-result.get`.
Retain that eager durable identity. If an unfinished future is dropped, attach
the close to the existing `Cancelled` path rather than recording a second
operation unnecessarily.

Cover delivered results, discarded completions, explicit cancellation with a
partial response, drop before `get`, cancellation during `get`, streaming RPC,
and incomplete-call redispatch. Completed effects must not be re-executed.

Once the span is closed, subsequent resource destruction must not close/export
it again. Explicit finish followed by drop has the same exactly-once closure
requirement.

### 5.3 Rejected/baked async RPC

Define the span as the rejected invocation attempt, not the lifetime of a future
containing an already-produced failure. Close it when rejection is produced.
This is an intentional, meaningful timing change.

Record deterministic validation failures as short operations when they produce
an invocation span. Handle branches that currently have no durable invocation,
and branches that already persist `End` before returning a baked future.

Keep returned errors, authorization boundaries, and absence of remote dispatch
unchanged. Do not add a second durable invocation around an already-recorded
admission denial merely to obtain telemetry.

### 5.4 P2 HTTP

P2 request cleanup already closes a request scope and then finishes its span.
Attach the transitions to the existing request-lifetime scope where its
boundaries match. Preserve request/header propagation, body ownership transfer,
retry behavior, final-owner drop, and forced-commit semantics.

Remove the separate span-only cleanup phase once the scope terminal owns the
close. Retain exactly-once cleanup when several resource wrappers share the
request session.

### 5.5 P3 HTTP

Preserve the existing useful span duration: request initiation through response
body completion or abandonment, not merely response-header arrival.

- Send `Start` opens the span and records its context before header propagation.
- A successful send `End` returning a response leaves the span open.
- The body-consumption terminal closes it by explicit span identity.
- Send failure closes it at the send terminal.
- Guest drop of an unread response schedules a short durable cleanup operation
  whose terminal closes it.
- A guest-dropped in-flight send using `LeaveIncompleteOnDrop` needs an explicit
  closure owner too. Record a short local span-cleanup operation while leaving
  the original remote call incomplete. Do not change it to `Cancelled` merely to
  close the span. Distinguish actual guest abandonment from a trap or Store loss,
  and prove that handoff to a returned response cannot also close the same span.
- Audit cancelled/discarded sends, response handoff failure, body cancellation,
  trailers, transparent retries, and intermediate ownership transfers. Every
  path must have one owner of the remaining closure obligation.

Current P3 spans are derived and in-memory-only; the exporter has no direct
opening/closing records for them. Make their lifecycle visible without adding
positional entries. A derived ID may remain if appropriate, but the complete
recorded metadata, not the derivation alone, is the export contract.

Do not keep the durable send pending until body consumption. The send result,
body stream, and span have different lifetimes.

### 5.6 Guest-created spans

Make `context.start-span` a normal durable local operation. It records opening
metadata and returns/reconstructs the span resource. Explicit finish and guest
resource drop are short operations that close the identified span.

Preserve activation of the current span, restoration of its parent on finish,
inherited attribute lookup, linked contexts, and the recorded start timestamp.
The resource can remain live across other durable calls without keeping its
creation operation open.

Convert attribute operations as described above. Pure context reads need not
acquire new durable records if their outputs remain deterministic functions of
correctly reconstructed context.

## 6. Replay, cleanup, and crash correctness

### 6.1 Separate resolution from context mutation

The cursor and resolver may discover metadata before the associated operation
can apply it. Do not install, activate, mutate, or remove guest-visible spans
from a generic lookahead callback.

The host/session lifecycle must establish the application boundary. In
particular, closing a span must not make an earlier recorded guest attribute
read fail merely because its terminal was prefetched. Starting/activating a span
must preserve caller association when equivalent operations overlap.

Replay performs the local state transitions even when the external result is
fully recorded. Tests must check state and propagation, not just return values.

### 6.2 Guest drop versus runtime teardown

Guest lifecycle operations and completion of their owned host tasks can record
successful cleanup. Destroying a Store during crash, suspension, restart, or
recovery failure must not invent `End`, `Cancelled`, or span-finished metadata.
Do not treat an arbitrary Rust destructor as evidence of guest abandonment.

P3 synchronous destructors cannot await replay or persistence. Reuse the existing
owned deferred-cleanup infrastructure where appropriate, but prove its admission,
identity, cancellation, and settlement behavior for the new operation. Merely
queuing a closure in resident memory is not a durability guarantee.

Required ordering:

1. Capture the cleanup obligation and durable operation/resource identity at the
   guest drop; retain its span metadata separately.
2. Claim or record its operation through the normal durable lifecycle.
3. Reconstruct/apply local cleanup at the correct operation boundary.
4. Persist/consume its terminal and span transition.
5. Settle required cleanup before invocation completion can overtake it.

This list expresses obligations, not permission to blindly perform step 3 before
durable admission or to return from a synchronous import with unsafe ownership.
The vertical slice must establish the precise safe ordering.

### 6.3 Retained-prefix cases

Test fresh reconstruction for each applicable boundary:

| Retained history | Required outcome |
|---|---|
| Before creation `Start` | Ordinary live admission; no inherited span |
| After creation `Start`, before `End` | Restore recorded identity; repair only incomplete work |
| After creation `End`, before guest observation | Respect existing completion-delivery rules |
| Span open across other calls | Reconstruct attributes, parentage, and resource ownership |
| Before cleanup `Start` | Guest execution reconstructs and reaches the same cleanup |
| After cleanup `Start`, before `End` | Complete local cleanup without duplicate closure |
| After cleanup `End`, before delivery marker | Do not repeat completed effects or close twice |
| Completion discarded | Preserve recorded discard and required local lifecycle effects |
| Runtime teardown with no guest cleanup | No invented successful close |

Use only supported fork/revert cut points. Deleted regions are not replayed.
Snapshot save/load retains existing recording suppression; do not append span
management records merely because the helper is now a durable call.

Deferred cleanup retains the originating operation's persistence provenance.
Do not decide whether to persist solely from the Store's mode when the queue is
eventually drained. An opening and drop performed with durability suppressed
must not produce an orphan persisted close if cleanup drains during a later
normal invocation. Test that delayed drain explicitly, alongside the existing
snapshot rules for resources that predate snapshot execution.

## 7. Relationship to GOL-581

GOL-581 covers two established shared defects:

1. Positional consumption before ownership validation.
2. Direct host calls retaining exclusive Store access while waiting for progress
   that requires that same Store.

Span conversion removes one family of positional records and the PR #3961 tail
reader. It does not solve the second defect, nor ownership of unrelated markers.
Converting an API to a normal durable call can expose the same Store-progress
requirements at admission, terminal resolution, or replay-to-live.

Coordinate with the GOL-581 implementation owner before changing shared call
machinery. Use the same corrected lifecycle rather than a competing cursor
reservation scheme or blanket Accessor conversion. Do not interpret the plan's
historical experiments as proof that a current candidate is correct.

Reuse the RPC/timer regression and GOL-581's deterministic same-Store scheduling
tests where applicable. Include incomplete retained histories; completed-call
prefetch alone is not a liveness solution.

The audit found no other non-span user of the optional `post_end_entry` facility.
Recheck this before removing it. Other positional families remain separate
GOL-581 obligations: NoOp/checkpoints, atomic markers, retry-policy mutations,
and remote-transaction protocol markers. Resource/stream hint records are not
equivalent positional tails. Transaction commit/rollback markers precede their
scope `End`; do not classify them as the RPC suffix bug without evidence.

## 8. Complete GOL-458, not just its new span match arms

### 8.1 Required inventory

Build a coverage matrix against the pinned implementation revision. For every
relevant feature, identify its authoritative records, expected telemetry,
attributes, correlation identity, lifecycle, tests, and documentation changes.
Classify each as implemented, intentionally not exported, or requiring work.

Produce this matrix in Step 1, before the vertical slice, not during final
verification. Each row must name concrete records/fields and a test with expected
output. A missing recorded source is a design gap to resolve or explicitly
justify, not automatic permission to declare a relevant feature out of scope.
Any new instrumentation beyond the span conversion needs a demonstrated telemetry
requirement and a bounded implementation proposal; do not expand by speculation.

At minimum inspect:

- Agent identity, owner kind, mode, component/environment metadata, and invocation
  context.
- Durable call completion, cancellation, delivery/discard, custom durable
  invocations, retries, and recovery versus invocation failures.
- RPC connection/invocation spans, streaming RPC, and ephemeral execution.
- P2/P3 HTTP and body lifetimes.
- Tool/entity execution, middleware, native/MCP tool paths, and their ownership
  and parentage.
- Durable streams and session lifecycle, including terminal/cancellation
  semantics and early invocation results.
- Suspension, resumption, interruption, restart, recovery success, snapshots,
  updates, fork/revert, and deleted-history effects on exported telemetry.
- Resource and memory metrics and existing log correlation.

Not every oplog variant needs its own span or metric. Record intentional
non-export decisions rather than manufacturing telemetry for every entry.
Avoid high-cardinality metric labels and secrets in attributes or logs.

### 8.2 Span state and trace identity

Today the plugin maintains one current trace context per worker, flushes all
pending explicit spans on invocation finish, and clears span state at the next
invocation start. The runtime, however, can preserve open spans across invocations.

Replace this assumption with explicit span lifetime and per-span trace context.
Invocation completion closes invocation-owned spans, not every resource/user
span indiscriminately. A new invocation must not discard still-open resource/user
spans. Store origin trace identity/state with every pending span and export it
under that context, not the worker's current invocation context at close time.

`Error` and `Interrupted` are not by themselves proof of permanent logical span
completion. Preserve appropriate logs/counters or attempt diagnostics without
closing every span. Establish which recorded facts prove definitive termination
before assigning fallback closes; a retry attempt must not prematurely export
the surviving logical operation as permanently failed.

Define and test:

- Logical spans versus execution-attempt diagnostics during retries/recovery.
- Which spans close at invocation completion, explicit cleanup, or definitive
  termination.
- How unclosed spans are handled on permanent failure/deletion without inventing
  guest execution events.
- Storage bounds for legitimate long-lived spans and cleanup of terminal owners.
- Parentage when a resource is reused under another invocation's trace.

An OTLP parent must belong to the appropriate trace. Preserve a long-lived span's
origin context. A new span under a later invocation uses its appropriate
same-trace ancestor as parent; a related resource span from another trace is a
link. Resolve the exact ancestor selection in the runtime context model before
the schema slice and test propagated headers as well as exporter output.
Changing only the exporter cannot repair incorrect propagated parentage.

Keep RPC connection spans as agreed. Do not solve state growth by silently
evicting legitimate pending spans or removing that telemetry. Decide lifecycle
and resource bounds explicitly, including what authoritative information is
available for permanent failure and source deletion.

The existing exporter drops linked-context information. Account for links in this
work rather than carrying that omission into the new schema.

### 8.3 Processing and export

Consume structured raw-entry transitions from `Start`, `End`, and `Cancelled`,
including applied attribute changes on successful terminals. Preserve span
identity and timestamp across batch boundaries. Use the processor's first-entry
index when operation-index tracking is needed; it is currently ignored.

Define explicit mappings for completed application errors, denied calls,
cancellation, abandonment, and recovery. Delivery/discard markers describe guest
observation, not whether an external effect occurred; do not rewrite completed
operation outcomes based on a discard.

Preserve correct parent resolution, attribute types supported by the source,
trace state, names, span kinds, and log correlation. Keep existing useful metrics
without counting internal span-management operations as extra user work by
accident. Document intentional changes in names, meaning, or cardinality.

Correct the existing state/export failure coupling. Current code stages state
and commits it only after all signal exports succeed; any export error returns
before that commit. The local processor caller ignores the application result,
and a guest `Err` is recorded as a completed invocation result. Therefore a
returned error is not proof that the batch will be retried, and replaying the
same idempotency key is not proof of another collector attempt.

Separate accepted source-state advancement from export outcome. A collector
failure must not forget a batch's openings, attribute changes, or closures and
corrupt later batches. Before implementing the exporter slice, trace local and
remote delivery/checkpoint paths and choose an explicit policy:

- Best-effort export with processed state retained independently of send success,
  observable loss/failures, and documented delivery semantics; or
- Retained output with a bounded retry mechanism and explicit per-signal progress,
  recovery, backpressure, and duplicate-delivery semantics.

The choice is not settled by the review. Do not promise reliable retries while
implementing best effort, or silently drop telemetry while claiming delivery.
Test a close-containing batch whose collector request fails, traces succeeding
before logs fail, subsequent batches, processor reconstruction, and the actual
redelivery behavior. Do not claim exactly-once collector delivery merely because
Golem deduplicates the plugin invocation.

### 8.4 Compatibility policy

GOL-458 asks to preserve telemetry consumers/configuration or document intentional
changes. Retain sensible existing telemetry names and configuration where they
still describe the new behavior. Document intentional differences.

Repository policy prohibits backward-compatibility machinery. Do not implement
old-oplog readers, version negotiation, legacy configuration fallbacks, or dual
span emission. Update all in-tree consumers in the same changeset.

## 9. Contract and artifact update inventory

Inspect and update the actual source owners, not just generated output:

- `golem-common/src/base_model/oplog/mod.rs`: raw/public entry definitions,
  constructors, transition types, and removal of positional span variants.
- `golem-common/src/model/oplog/`: serialization, protobuf conversions, matching,
  typed host-function identities and payloads.
- `golem-api-grpc/proto/`: raw/public oplog messages and generated bindings.
- `wit/deps/golem-1.x/golem-oplog.wit` and synchronized WIT consumers.
- `golem-worker-executor/src/model/public_oplog/`: raw/public/WIT projection.
- Executor cursor/claims, custom observational subtrees, status folding,
  checkpoints, cut validation, fork payload handling, and drop cleanup.
- Rust, TypeScript, Scala, and MoonBit SDK oplog consumers and generated artifacts.
- CLI oplog rendering, filtering, and structured-output schemas.
- OpenAPI and generated REST documentation where public shapes change.
- OTLP plugin code, tests, documentation, examples, committed
  `plugins/otlp-exporter.wasm`, built-in descriptor version, and exported
  instrumentation-scope version (currently a separate hardcoded value).

Load the relevant WIT, SDK, plugin, output-schema, and HTTP-generation skills
before making those changes. Follow generation commands rather than manually
editing derived files. A plugin source update without rebuilding the embedded
WASM does not deliver the new exporter.

## 10. Implementation sequence and gates

### Step 1 — Pin baseline and coordinate

Record repository revision, relevant PR/GOL-581 integration state, Wasmtime
revision, and fixture build identity. Confirm shared-file ownership with ongoing
work. Keep existing unrelated changes intact.

Inventory every span producer, mutation, terminal, and exporter consumer. Add the
GOL-458 coverage matrix with concrete source fields, expected outputs, and named
test cases; distinguish observed defects from hypotheses. Include documentation
and runnable examples in its acceptance rows.

### Decision gate — Resolve before Step 3

Record the following decisions in this plan or its implementation design notes
before schema/session and exporter implementation begins:

- Opening metadata construction for calls and scopes, including index-derived
  identities where used; metadata remains excluded from claim matching.
- Close timestamp policy, including deferred cleanup and incomplete recovery.
- Exact same-trace ancestor selection and cross-trace link propagation.
- Invocation-owned versus explicit span lifetime, attempt-error mapping, and
  handling of definitive termination and long-lived pending state.
- Export delivery policy and source-state commit ordering under collector failure,
  justified by both local and remote processor delivery paths.
- Concrete GOL-458 coverage rows, including treatment of features whose required
  telemetry lacks an existing recorded source.

These are bounded design obligations, not a requirement to reopen the agreed
cross-operation span model or introduce tracing-dependent replay matching.

### Step 2 — Establish discriminating regressions

Preserve the RPC/timer ordering reproduction before changing the contract.
Add tests for a span opened by one operation and closed by another, deferred
response drop, current-context mutation ordering, and exporter split batches.

Use deterministic gates rather than sleeps for decisive schedules. Use
asymmetric values and different caller results so swapped identities or early
state application fail visibly.

### Step 3 — Prove one complete vertical slice

Implement the minimal new schema/session support, ordinary async RPC, and one
cross-operation lifecycle with a real cleanup boundary. Include exporter support
for those records so observability is part of the slice, not an afterthought.

The slice must demonstrate:

- No positional span reads in converted paths.
- Strict request/owner matching before consumption.
- Correct local reconstruction and guest completion order.
- Cleanup after incomplete history and no close on Store teardown.
- Persistence provenance retained by deferred snapshot cleanup.
- No same-Store progress deadlock under the relevant forced schedules.
- Correct collector-visible IDs, parentage, attributes, and timestamps.
- Collector failure does not discard processed lifecycle state; retry/loss
  behavior matches the selected delivery policy.

If it fails, revise the shared boundary before expanding conversions.

### Step 4 — Convert the complete producer family

Convert RPC connections, all RPC outcomes, P2/P3 HTTP, guest span APIs, attributes,
and deferred drop paths. Remove standalone span records and obsolete tail-append
machinery after confirming no remaining consumers.

Do not leave a fallback to the positional implementation. Do not convert unrelated
host imports for consistency alone.

### Step 5 — Finish the GOL-458 coverage matrix

Implement required 1.6 mappings identified by the inventory, including lifetime,
trace-context, and link handling. Close each matrix row with tests or an explicit
reason no export is appropriate. Update configuration/data documentation.

### Step 6 — Validate one combined candidate

Run targeted tests after each change, then the combined relevant suites against
one final revision and freshly built fixtures/plugin artifact. Obtain a focused
review of replay application boundaries, teardown, trace identity, and export
retry behavior. Fix accepted findings and rerun affected checks.

Update durable-execution guidance and the executor walkthrough. Inspect any
changed rendered walkthrough diagrams using the repository's browser workflow.

## 11. Verification matrix

| Area | Required discriminating checks |
|---|---|
| Original defect | Prefetched RPC result with an interleaved timer; exact result and cursor ownership |
| Strictness | Wrong operation owner/target/request fails without consuming another operation; missing calls are not skipped |
| Tracing independence | Span metadata does not select the claimed Start; the host restores recorded span IDs after claiming and rejects inconsistent transitions without searching for another call |
| Context ordering | Span-map membership, current_span_id, guest attribute/started-at reads, and propagated headers are correct at observation boundaries despite prefetch |
| Caller association | Equal operation shapes with distinct recorded results stay attached to the correct guest callers |
| RPC lifecycle | Delivered, discarded, denied, validation failure, explicit cancel, drop-before-get, cancel-during-get |
| Rejected RPC spans | Rejection produces the intended short span and failure mapping without remote dispatch or an extra invocation around a recorded denial |
| RPC recovery | Incomplete call uses original identity; completed remote effect is not repeated |
| Resource cleanup | Explicit finish then drop; shared HTTP owners; response transfer; teardown does not close |
| HTTP | Headers before body terminal, body failure/cancel, unread-response drop, retries, trailers |
| Abandoned P3 send | Guest drop closes the span once through local cleanup while LeaveIncompleteOnDrop still leaves the remote call incomplete; teardown does neither |
| Guest spans | Recorded started-at, nested activation, attributes, links, explicit finish, resource drop |
| Cross-invocation spans | Open span survives invocation boundary; origin trace retained; cross-trace parenting is valid |
| Retained prefixes | Before/after Start, after End, delivery/discard boundary, legal fork/revert cuts |
| Snapshots | Suppressed opening/drop drained after snapshot mode exits produces no persisted orphan transition; retained resource provenance is respected |
| Atomic | No resource-lifetime durable scope; new local operations obey membership/lease rules without extending the region; remote retry/rollback idempotency guarantees remain intact |
| OTLP batches | Structured opening/updates/finish split across batches; interleaved spans and externally stored call payloads need no payload decoding |
| Export failures | Failed close-containing batch, partially successful signal exports, subsequent batches, reconstruction and actual redelivery preserve lifecycle state and meet the selected delivery policy |
| Recovery telemetry | Error/interruption followed by successful recovery does not prematurely close or lose a surviving logical span |
| OTLP semantics | Correct durations, statuses, attributes, trace state, links, logs and metrics for inventory rows |
| Artifact delivery | Built-in provisioned WASM actually emits the new format in an end-to-end collector test |
| Documentation/examples | Updated documented lifetimes, statuses, delivery guarantees, configuration and runnable examples agree with collector output |

Use actual guest execution and fresh Store/executor reconstruction for decisive
runtime cases. Unit cursor tests alone cannot prove resource-state reconstruction
or Store-safe progress. Assert required oplog growth, effect counts, and span
counts, not only successful guest return values.

Run package-scoped formatting/build/lint checks, replay-state and concurrent-call
tests, relevant RPC/HTTP/context/atomic/snapshot/reconstruction tests, plugin
tests, and targeted end-to-end telemetry tests. Broaden worker-executor coverage
for shared lifecycle changes. Follow the `testing` skill; do not run the umbrella
`cargo make test` task.

Regenerate and check affected WIT, public APIs, schemas, SDK bindings, docs, and
plugin artifacts. Do not declare GOL-458 complete from span unit tests alone.

## 12. Completion criteria

- No production path emits or consumes standalone positional span records.
- General replay contains no span-specific terminal-tail handling.
- Every span transition has an ordinary durable-operation owner, including
  cancellation and guest cleanup, without keeping resource-lifetime calls open.
- Recorded span state and guest observations reconstruct correctly after Store
  loss; completion prefetch does not apply visible state early.
- HTTP and RPC span lifetimes follow the documented semantics.
- The GOL-458 inventory has no unexplained unsupported relevant feature, and
  focused plus end-to-end tests validate the chosen mappings.
- The committed plugin artifact and descriptor match the updated source and
  public oplog contract.
- Required contract consumers and generated artifacts are updated together,
  with no compatibility path left behind.
- Collector failures preserve processed lifecycle state, and documented export
  delivery guarantees match executed failure/reconstruction tests.
- Verification reports the exact final candidate, commands, results, and any
  remaining limitations honestly.

Keep implementation stages reviewable. Local commits may separate regressions,
contract changes, producer conversions, exporter work, and final artifact updates,
but the delivered changeset must be internally consistent. Pushing, opening a PR,
merging, publishing, or deployment requires separate authorization.

## 13. Implementation findings and acceptance inventory

### Baseline and coordination, 2026-09-24

- Local `main` is pinned at `3fbd3c45187514ba5a8f31e101046c151463d9bf`, including
  the synchronous RPC span reconstruction fix from PR #3962. This is not a claim
  about the current remote branch. PR #3961 is closed, not merged.
- Wasmtime is `46.0.1`, pinned by Cargo.lock to
  `252ab61f67fc16e49c83575e8477a8c4eca13d0b`.
- Checked-in fixture SHA-256 values (not newly rebuilt):
  `concurrent-runtime-events/concurrent_runtime_events.wasm` =
  `43bb73f4f9b4d88c4ba417073c9a9164eca433c782bb40d6a14a06dc63bbb202`;
  `concurrent-delivery-order/concurrent_delivery_order.wasm` =
  `dee90bf640651c7e59a4b2caedd8063dfe96cc0ecc07171ba005677449b03255`.
- Initial committed OTLP WASM SHA-256 =
  `43bcc6837fe433feed11dd9e976656c31b3ece41c18b95de0f42cc08aa4bebe1`.
  No new span fixture or plugin artifact has been built yet.
- GOL-581 owner replied from
  [the implementation thread](https://ampcode.com/threads/T-01a0d2b9-b2e3-757b-9574-09d7aff934de).
  Its candidate is unverified and not integrated here. It retains unclaimed
  starts while advancing the cursor, and changes admission predicates/claim
  outcomes. It does not change the Start/End/Cancelled schema or drop policies.
  Keep span changes out of its cursor/claims/abandoned implementation and four
  call-session claim sites; reconcile the combined candidate before declaring
  Store-safe reconstruction verified.
- GOL-458 is In Progress; the approved plan is attached to the issue. Local
  implementation changes remain uncommitted.

### Producer ownership inventory

| Producer | Opening owner | Closing owner / obligation |
|---|---|---|
| RPC connection | `wasm-rpc.new` operation | Short guest resource-drop operation; constructor failure must not orphan an opening |
| Synchronous/fire-and-forget RPC | Existing invocation Start | Same invocation terminal, preserving error outcomes |
| Async RPC | Existing eager invocation Start | Same End/Cancelled for get/cancel/drop; baked rejection closes when produced |
| Streaming RPC | Existing invocation Start | Scalar invocation terminal, not later stream draining |
| P2 HTTP | Existing request-lifetime batched scope | Final owner's scope terminal, including response/body/trailer ownership transfer |
| P3 HTTP | Send Start | Send failure terminal, consume-body terminal, or local abandonment cleanup |
| Guest span | New local span-creation operation | Local finish/drop targeting creation-operation identity; repeat finish is a no-op transition |
| Guest attribute mutation | Local mutation operation | Applied delta on terminal; preserve current per-attribute prefix semantics |
| Implicit invocation | AgentInvocationStarted | AgentInvocationFinished, not every Error/Interrupted hint |

Current sources: `durable_host/wasm_rpc/mod.rs`, `durable_host/http/{mod,types}.rs`,
`durable_host/p3/http/{send,response_body}.rs`,
`durable_host/golem/invocation_context_api.rs`, and `durable_host/mod.rs`.
No public arbitrary-context start-span API is required; the existing public API
starts under the current context. Internal RPC parent/link selection must still
handle long-lived connection contexts.

### Concrete GOL-458 coverage matrix

The names below are acceptance tests to implement, not claims of passing tests.
Except the localized service-name source fix, all required changes remain open.

| Feature | Authoritative source / existing behavior | Required output and discriminating acceptance test |
|---|---|---|
| Service naming | Processor agent ID; `export.rs` currently used full ID for both modes | `agent_type_service_name_ignores_constructor_parameters_and_phantom_id`: two instances share the type service name; agent-ID/resource identity remains full. External-tool owners have no type and keep their name without a panic |
| Agent identity, mode, owner | Raw Create has agent-id, instance-id, owner-kind, agent-mode, environment-id; processor metadata lacks owner-kind/mode | `resource_attributes_cover_16_agent_metadata`: bounded mode/owner attributes, environment identity, correct component revision, no env/config values or secrets |
| Invocation and cross-invocation context | AgentInvocationStarted trace-id/trace-states/invocation-context; current exporter clears spans at invocation start | `cross_invocation_span_retains_origin_trace`: later invocation closes old resource span under its origin trace; later child uses same-trace parent and full-context cross-trace link |
| Durable call lifecycle | Raw Start/End/Cancelled payloads opaque/external; Start currently only counts calls | `split_batch_structured_span_lifecycle`: opening, applied delta and close in separate batches produce one correct span without decoding request/response; delivery/discard do not reclassify the effect |
| RPC families | Existing span producers above and new structured transitions | `rpc_connection_survives_invocations`, `async_rpc_cancel_and_discard_statuses`, `rejected_rpc_does_not_dispatch`, `streaming_rpc_result_precedes_stream_terminal`: exact span counts, duration and status at their respective owners |
| HTTP | P2 lifetime scope; P3 send and consume-body operations | `p3_headers_then_body_terminal_duration`, `p3_unread_response_drop_closes_once`, `p3_abandoned_send_preserves_incomplete_call`: recorded duration extends beyond headers, propagation matches recorded context, teardown adds no close |
| Guest spans/attributes | Local operations replacing positional records | `guest_span_nested_activation_links_and_attributes`, `finish_then_drop_is_single_close`, `set_attributes_preserves_applied_prefix`: reads and context membership match live across fresh reconstruction |
| Retry/recovery | Error kind/retry-from, RecoverySucceeded, Restart, Interrupted, Resumed | `retry_error_then_recovery_keeps_logical_span_open`: attempt diagnostics plus one logical completion, not premature permanent failure; preserve interruption/restart counters |
| Tool/entity/middleware/MCP | Entity Start ownership is structured; tool identity/outcome are currently in typed opaque requests/responses | `tool_entity_parentage_and_mcp_error`: add structured span metadata at entity operation owner; pinned safe tool/middleware attributes, correct completed error status, no body/secret attributes |
| Durable streams/sessions | StreamRegistered/Items/End/Cancel/Session raw records contain opaque payload references | `durable_stream_early_result_and_terminal`: result and stream terminal are distinct; required semantic lifecycle metrics need structured safe summaries or another explicit source, not payload decoding in plugin. Do not turn every item into a span |
| Lifecycle termination | Suspend/Resumed/Interrupted are resumable hints; source deletion has no processor terminal notification | `interrupt_resume_preserves_open_span`; definitive retirement/reclamation test requires the decision below. Never infer success or deletion from silence |
| Snapshot/update | Snapshot, SuccessfulUpdate, FailedUpdate; runtime snapshot recording suppression | Preserve existing metrics; `snapshot_suppressed_cleanup_has_no_orphan_close` proves deferred provenance survives mode exit |
| Fork/revert | ForkCut, Revert, copied raw prefix and instance identity | `fork_prefix_reconstructs_open_span`, `revert_before_close_does_not_fabricate_close`: define copied-history export identity; document OTLP cannot retract already-exported data. In-memory pending state must follow retained history |
| Logs | Raw Log has timestamp/level/context/message, no explicit span ID | Preserve logs and bounded severity counts; `interleaved_entity_log_correlation` needs authoritative context metadata rather than the exporter's ambient current invocation guess |
| Resources/memory | Create initial memory, GrowMemory delta, CreateResource/DropResource | `resource_metrics_balance_across_batches`: preserve names and cumulative state even when no spans remain; no per-resource metric labels |
| Checkpoints | OplogProcessorCheckpoint sending/confirmed indices | `checkpoint_lag_and_internal_operation_filtering`: retain lag metric; internal local tracing operations do not inflate user call counts |
| Collector failures | Source confirms batch enqueue; plugin Err is a completed invocation result | `collector_partial_failure_retains_lifecycle_state`: failed signal must not forget openings/updates/closes; remaining signals attempted; document best effort, not exactly-once collector delivery |
| Artifact and docs | Embedded `plugins/otlp-exporter.wasm`, descriptor, scope version | Collector E2E must execute rebuilt artifact. Update observability, invocation-context and durable-stream docs, OTLP skill sources/generated guides, and docker collector example to agree with observed output |

### Decision-gate evidence and proposed implementation choices

1. **Opening construction:** prepare descriptor before immutable Start append;
   extend indexed construction for derived HTTP IDs and scope starts. Matching
   still claims execution identity before restoring metadata. No random ID
   requirement and no span-aware claim discriminator.
2. **Timestamps:** record explicit event time for closes; synchronous guest drop
   captures it before deferred drain. Replay reads recorded values. Incomplete
   cleanup records the captured drop time as its Start timestamp, then uses that
   value for its close after a crash between cleanup Start and End. Timestamps
   remain metadata, never request/claim identity.
3. **Trace context:** retain origin trace/state per span. A requested parent in a
   different trace becomes a link; select the nearest current same-trace ancestor
   as parent. This must change runtime propagation as well as exporter output.
4. **P3 abandonment:** pinned Wasmtime exposes
   `Accessor::register_terminal_observer(FnOnce(TerminalConsumption))`.
   `NotDelivered` is positive guest cancellation/abandonment; teardown drops the
   observer without invoking it. It can enqueue cleanup but cannot await or touch
   the Store. Compose it with existing completion delivery instead of replacing
   an armed observer. Drain owned cleanup before subsequent durable admission and
   invocation completion. No runtime API extension is established as necessary;
   test cancellation before/after End and handoff, supersession, and teardown.
5. **Export failures:** select best-effort output rather than a new persistent
   collector outbox. Preserve processed lifecycle state independently of sends,
   attempt enabled signals independently, and surface failures. This does not
   promise one physical HTTP attempt: crash repair/host retries can duplicate an
   ambiguous collector request. Source checkpoint confirmation means enqueued,
   not collector-acknowledged. Both local/remote paths in
   `services/oplog/plugin.rs::send` and `flush_one_plugin` establish this.
6. **Pending-state reclamation is limited by the existing plugin contract:** permanent source deletion
   removes the oplog after `stop_and_wait`, without notifying the processor of a
   definitive retirement. A permanently failed invocation may later resume or be
   updated. There is no current authoritative reclamation protocol for exporter
   state. A metadata lookup alone does not solve incarnation fencing, periodic
   cleanup, or state transfer when processor routing changes. Retain legitimate
   pending spans rather than inventing closes or silently evicting them. The
   owner explicitly accepts the pre-existing instance-switch limitation and
   excludes plugin-model changes; GOL-667 tracks continuity separately.

The deletion source is `worker/mod.rs::run_deletion_attempt`; forwarding disposal
is `services/oplog/plugin.rs::ForwardingOplog::retire`/`fence_forwarding`/`Drop`.
The processor export only exposes `process` batches. Its world does include host
imports such as `get-agent-metadata`; it is not correct to claim source lookup is
impossible. A lifecycle extension would still need ordering, source incarnation
fencing, plugin deactivation/reactivation, and crash recovery of notifications;
a best-effort callback is insufficient.

### Step 1 review: processor continuity explicitly excluded

Oracle found a more fundamental constraint than pending-state reclamation:
`wit/deps/golem-1.x/golem-oplog-processor.wit` explicitly allows different batches
from one source to be delivered to different processor instances.
`services/oplog/plugin.rs::try_locality_recovery` actually changes the target
while preserving the confirmed source index; it does not replay the prefix or
transfer plugin memory. This was checked directly in the implementation.

Thus the current plugin and the stateful design in sections 8.2–8.3 can lose the
opening when a different processor receives the close. Durable reconstruction of
one processor instance does not transfer its state to another logical processor.
The owner explicitly rejected solving this in GOL-458, including duplicating
full span information on close. The original stateful design in sections
8.2–8.3 stands. Same-instance split-batch and reconstruction tests remain required;
a successful split-across-different-instances test belongs to GOL-667, not this
changeset. Document the limitation without weakening the exactly-once source
batch delivery guarantee or claiming exactly-once collector delivery.

No plugin routing, state-transfer, retirement protocol, or self-contained-close
amendment is approved. A span without an authoritative close must still not be
exported as successfully completed. Required fixes to lifetime/context handling
and state advancement under collector failure remain in scope.

The same review identified a concrete P3 handoff obligation within the existing
plan: registration must follow the send Start because normal durable admission
clears terminal observers; guest cancellation drops the send future before the
observer runs. After End, transfer cleanup ownership to completion delivery so
NotDelivered records both discard and local response/span cleanup. A response
never handed to the guest cannot rely on the guest's response destructor.
Test pre-End cancellation, post-End discard, observer supersession and teardown.

### Localized service-name verification

- Plugin host tests: six passing with `cargo test --manifest-path
  plugins/otlp-exporter/Cargo.toml -p otlp_exporter`.
- Oracle found the initial parser assertion invalid for external-tool owners.
  A regression failed with the panic before the total extraction fix and passed
  afterward. Oracle follow-up accepted the fix.
- Bug-finder run 1 reported no bugs; run 2 proposed malformed non-agent names.
  That finding is rejected: component workers without an agent type cannot obtain
  plugins through `owner_plugins`/flush, while valid external-tool owners are
  already covered. Oracle independently confirmed this reachability analysis.
- Bug-finder run 3, covering the localized fix and Step 1 inventory, reported no
  bugs after that adjudication. The processor-continuity finding was subsequently
  explicitly excluded by the owner and tracked in GOL-667.
- Cargo refreshed the plugin lockfile to match current in-tree SDK dependencies.
  Changed-file formatting passes; unrelated exporter formatting drift is not
  part of this edit. WASM/version/docs are not yet updated, so this source fix is
  not a delivered plugin release.

### Approved amendment: persist local RPC denial before its terminal

The synchronous RPC review found that a locally rejected call recorded a dummy
request in `Start`, with the denial present only in `End`. A crash retaining only
that `Start` could enter ordinary incomplete-call dispatch without the original
authorization decision. Span metadata must not be used to distinguish denial
from dispatch.

The owner approved recording an optional local denial in the RPC request itself.
All synchronous, fire-and-forget, streaming, and asynchronous replay entrances
must inspect this recorded execution decision before dispatch. A completed denial
validates its recorded response; an incomplete denial completes from the recorded
decision without remote execution or reauthorization. Existing incomplete-call
eligibility remains unchanged, including fail-closed non-idempotent remote writes.
This is persisted request data, not transient schema metadata or tracing identity.

Asynchronous denials open and close their embedded span in the denial operation;
the baked future has no remaining span-cleanup obligation. Synchronous denials
remain spanless. Tests must destroy and reconstruct the executor after committing
the denial `Start`, assert zero dispatch and one terminal, and then reconstruct
the completed history as well. Validate request-payload binary roundtripping.

### P2 implementation verification, 2026-09-25

The P2 scope opening and closing now carry the span transitions. Oracle caught
an initial bypass of the coordinator's commit/checkpoint path; both ordinary and
span-aware closes now use that path. Deferred accessor closes preserve the same
commit level and checkpoint conditions. Session ownership retains snapshot
persistence provenance. Unread drops default to cancellation, observed body EOF
or successful trailers establish completion, and final observed errors or HTTP
error status establish failure that a subsequent EOF cannot erase.

Executed checks:

- `CARGO_INCREMENTAL=0 cargo test -p golem-worker-executor --lib --
  durable_host::http:: --report-time`: 29 passing.
- `CARGO_INCREMENTAL=0 cargo test -p golem-worker-executor --test integration --
  p2_http_span_terminal_commits_before_guest_continues --report-time`: passing
  against freshly rebuilt and validated `host-api-tests` WASM. This uses actual
  P2 imports, observes the span-bearing commit before a CPU-only guest loop can
  return, checks cancellation/completion/failure outcomes, and recreates the
  executor with a provider counter proving completed replay did not resend.
- Oracle accepted the corrected direct/deferred commit paths. Bug-finder run 1
  caught a missed RDBMS caller after the signature change; run 2 confirms it
  resolved and reports no new findings, with the above checks passing.

This is scoped verification, not completion of the combined changeset.

### Approved narrow direct-cleanup integration, 2026-09-25

The owner rejected a general host-internal replay category and approved reusing
the existing direct-call completion semantics through Store-releasing accessor
windows. The first implementation exposed that narrow path to deferred cleanup:
ordinary function/owner/request claims, no guest delivery marker or markerless
guest-result tail wait, and no access to the enclosing guest subtask's observer.
No replay cursor or claim algorithm changes were made. Span IDs remain outside
request matching. Live and completed replay both finish the in-memory span.
The actor-owned implementation below replaces the temporary direct-accessor
session wrappers; those wrappers have been removed.

Executed verification:

- `p3_unread_response_cleanup_reconstructs_before_next_accessor` passes against
  the rebuilt host-api fixture. It drops an unread response, makes a timer call,
  recreates the executor, and invokes again. The provider sees exactly two
  requests, not a replay resend. Cleanup has one Start/End and no guest delivery
  or discard marker.
- The concurrent-call library tests pass: 67 tests.
- Oracle accepted the direct-completion plumbing, but identified the separate
  cancellation-ownership blocker below. P3 is not a completed slice.

### Initial P3 blocker: cancellation of the call draining another resource's cleanup

Example: the guest drops response A, queuing its cleanup, then starts call B.
B's admission drains A's event and submits cleanup Start C. If the guest cancels
B while that drain awaits persistence, the drain guard requeues the original
event. A later drain can submit a second cleanup Start C2. After a replay claim,
requeueing can instead try to claim the same operation twice. This is a defect in
the new cleanup integration, not evidence that general replay needs span logic.

The existing terminal guard owns an End append once it is handed off. It does
not cover the full interval from Start submission through response preparation
and terminal handoff. Merely replacing the queued event after obtaining a
session misses cancellation while the oplog actor has accepted Start but its
receipt has not reached the caller. Changing NotCancellable to
LeaveIncompleteOnDrop avoids a debug panic in another interval but can lose
the close permanently when deterministic guest cancellation repeats on replay.
Neither shortcut meets this plan's closure requirement.

The proposed bounded solution is to give this deferred local operation retained
ownership from Start submission through terminal settlement. Cancellation of B
would return a receipt/progress state to the queue, not a fresh request to start
A's cleanup again. Use captured operation identity, ownership, persistence
provenance, and event timestamp. The operation must remain joinable from direct
and accessor drain sites without needing exclusive Store access held by its
waiter. A background Store task alone is not sufficient: a direct drain could
hold the Store while waiting for that task. The owner subsequently approved the
actor-owned implementation described below, without a Wasmtime fork change or
general replay redesign.

The alternative is to relax the span-close requirement on these cancellation
windows. That is not currently approved and is not recommended. Changing the
plugin model, restoring positional span tails, or teaching the cursor about
spans are not solutions under the agreed constraints.

Before accepting a fix, force cancellation before/after Start actor acceptance,
before/after End handoff, and during replay claim/resolution. Assert one cleanup
operation and one close after a later drain, then recreate the Store. Also finish
the still-pending timestamp, snapshot-provenance, pre-End NotDelivered observer,
post-End response ownership, and body-outcome work. These are not deferred out of
the changeset.

### Approved actor-owned cleanup implementation, 2026-09-25

The synchronous drop submits the complete live cleanup Start/End pair to the
existing oplog actor using `enqueue_add_pair`. This is the synchronous submission
form of the existing `add_pair` operation, not a second writer or a new actor.
Dropping its receipt cannot retract or repeat submission. The cleanup queue owns
a shared receipt and the captured drop timestamp; a later drain joins that same
operation and applies the in-memory close.

Replay uses an ordinary function/owner/request claim, retained as a shared
future rather than recreated after a cancelled drain. The producer starts a
Store-owned, Store-independent driver at the actual guest drop, so a previously
running call cannot block behind an unclaimed cleanup Start. Existing tail-work
accounting keeps that claim active through settlement. Guarded replay-to-live
transition remains in the existing drain path. Incomplete cleanup repairs only
the original End, using the original Start timestamp. Snapshot provenance is
captured before drop and prevents persistence after snapshot mode ends.

The actor layer's first Oracle review found the initially lazy replay-claim
deadlock; eager driving addresses it. Bug-finder's bounded actor-cleanup run
reported no bugs. The targeted executor unit run passed 74 tests, including
cancelled drains before/after append completion, cancelled replay claims,
eager claim progress, incomplete repair, snapshot suppression, body outcomes,
and ordered atomic pair submission. The real
`p3_unread_response_cleanup_reconstructs_before_next_accessor` reconstruction
test and the explicit RPC cancellation regression also pass. This is scoped
verification, not P3 acceptance.

The P3 producer draft still needs a single span-close owner at the terminal
handoff. Specifically, the send failure End, cancellable-call drop, and positive
guest `NotDelivered` observer must not each close the same span. The observer
must retain `CompletionDelivery::prepare_delivery` gating and suppress its token
when dropped during Store teardown. Those last two rules have been restored in
the local draft, and `send_observer_records_only_positive_guest_consumption`
passes. The body-finalizer concern was not substantiated: that finalizer already
runs in a Store-owned task, and guest cancellation does not destroy it.

### Approved P3 span-only ownership correction

The fresh review confirmed the terminal ownership race and exposed an earlier
startup window. The indexed Start append can be accepted by the oplog actor
before its awaited receipt returns a call session. Another await then reads the
span metadata before the current draft installs its observer. Positive guest
cancellation in either interval can leave an opening without a closing owner.
An End-only handoff hook therefore does not establish the whole invariant.

The owner approved extending the existing call-session admission and terminal
guards with span-only ownership notifications. Moving HTTP send execution to a
Store-owned task was rejected because it would change non-span cancellation
semantics. Network execution, retries, call terminals, drop policies, and guest
completion-delivery boundaries remain owned by their existing paths.

The implementation candidate registers the send's terminal observer before
durable admission, after rejecting a pre-existing observer. The indexed Start
builder transfers the opening metadata after fallible preparation and immediately
before the actor's nonfallible append. A generic accepted-claim callback inside
the existing owned replay transaction transfers recorded metadata even if its
waiter is cancelled. Neither callback changes matching or interprets spans in
the cursor. The blocked claim loop is not retained or restarted by the callback.
Existing tail-work accounting covers those notifications.

The span lifecycle stores a positive `NotDelivered` timestamp even when the
opening notification has not arrived. Once both exist, it submits the existing
actor-owned local cleanup. Failure-End submission and cancellable-call handoff
claim the span close synchronously, preventing a second abandonment close.
`LeaveIncompleteOnDrop` remains incomplete and uses local span cleanup only.
Teardown and observer supersession do not signal guest abandonment.

Replay cleanup reads the send's visible recorded terminal inside its retained
shared future. If that terminal already closes the span, it needs no separate
cleanup claim. Otherwise it claims the ordinary local cleanup operation. Both
the eager Store task and a Store-holding direct drain can run that same future;
there is no separate metadata task that a direct drain must wait for.

The bounded V4 ownership slice is accepted by Oracle inspection. Its prior
scope finding was corrected: terminal-guard runtime teardown now suppresses
only span metadata, preserving the existing non-span cancellation policy.
Opening timestamps are captured before admission awaits so a queued Start
cannot begin after its already-observed abandonment timestamp.

Verification passed 221 concurrent/replay unit tests and three real
cancellation/reconstruction integration tests (GET abandonment, POST cancellation,
and unread-response cleanup). They assert unchanged network/drop behavior,
exactly one span close, the expected outcome, and closure before invocation end.
Bug-finder run 2 resolved its test-compilation finding and found no new
reproducible bugs. Logs: `.amp/in/gol458-p3-final-unit.log` and
`.amp/in/gol458-p3-ownership-integration.log`.

This is bounded acceptance, not completion of the entire verification matrix.
The startup receipt/claim windows are exercised by deterministic unit gates and
state-machine tests; the real guest tests exercise pre-header cancellation and
unread response drop, not every actor acceptance/handoff schedule. Remaining
producer/contract removal, 1.6 telemetry, artifacts, and combined verification
are still outstanding.

### Rejected/baked RPC conversion candidate, 2026-09-25

Deterministic async validation failures now use one short `WriteLocal` operation
with embedded opening and failed close. Recorded activation denials put the
opening on the existing activation Start, whose authorization decision is
already known, and close that same operation with a denied outcome. No second
invocation is introduced for an activation denial. Baked futures contain the
already-produced error with no pending close; get/cancel/drop cannot export it
again. Ordinary matching excludes tracing data, and completed replay validates
the returned invocation metadata without remote dispatch.

The payload test, executor/test compilation, four focused RPC tests, and both
unused-future/reconstruction regressions passed against the rebuilt agent-rpc
fixture. Bug-finder `gol458-baked-rpc` run 1 reported no bugs, but the fresh
Oracle review found asymmetric logical idempotency-key consumption in activation
denials: span creation and baked metadata derived keys separately, and completed
replay did not consume the same number of keys. Two denied activations inside
an atomic region reproduced the resulting guest-result mismatch on reconstruction.

The corrected denial carries its reserved key through `OutboundRpcDenial` and
reuses it for both span attributes and baked metadata. Live, completed replay,
and incomplete replay reserve exactly one key for a denied activation. Allowed
activation paths reserve none at this point. The rebuilt fixture returns both
atomic-region denial keys, and the regression compares them with the recorded
span attributes before forcing reconstruction.

All four focused RPC integration tests pass after the fix: explicit cancellation,
synchronous local denial, unused-future validation rejection, and unused-future
activation denial. The log is `.amp/in/gol458-atomic-denial-fixed.log`.
The fresh Oracle follow-up accepted the fix by inspection, and bug-finder run 2
reported no bugs. Incomplete-replay denial and atomic sync/schedule denial remain
inspection-covered rather than directly exercised by this regression. This
bounded RPC acceptance does not cover the unresolved P3 ownership proposal.

The combined bounded rerun passed 75 executor unit tests and four integration
tests: explicit RPC cancellation, validation rejection, activation denial, and
unread P3 response cleanup across reconstruction. The log is
`.amp/in/gol458-combined-bounded-check.log`. These tests do not cover the remaining
P3 send startup/handoff windows described above.

### Bounded exporter processing progress, 2026-09-25

The exporter supports embedded transitions, per-span origin context and links,
retry/recovery diagnostics, Create resource identity, state reset on a new
incarnation, and inclusive pending-opening removal for Revert and Jump. Internal
span-management calls are excluded from user host-call counts. Source state
advances independently of best-effort collector export. Active-resource counts
are exported as absolute gauges rather than delta sums.

Oracle's two initial blockers (missing cleanup-name exclusions and missing Jump
handling) were fixed and tested. Bug-finder's mixed-incarnation-within-one-batch
report was rejected: source forwarding reads contiguous ranges from one oplog,
whose Create is the initial entry, not a splice across incarnations. Its metric
aggregation finding was accepted and corrected. Follow-up Oracle review could
not complete because the Oracle session exhausted its context; this is a review
limitation, not an approval of the final exporter candidate.

Final scoped checks for this iteration: all 23 exporter host tests pass,
WASI-target Clippy with warnings denied passes, and `git diff --check` passes.
Bug-finder run 3 confirms the metric finding is resolved and reports no new
findings. No further bug-finder run was requested for an unchanged candidate.

All changes remain local and uncommitted. The embedded plugin WASM/version,
remaining runtime conversions, additional 1.6 structured telemetry, generated
contracts/docs, and combined end-to-end verification are still outstanding.

### Combined candidate progress, 2026-09-26

This section supersedes the outstanding-work lists in the earlier progress
entries; those entries record the state at their respective review points.

The standalone span entry variants have been removed. RPC, P2/P3 HTTP, guest
spans, and entity/native-tool invocations now use embedded operation metadata.
Generic replay does not interpret span transitions, and request matching excludes
tracing identity. P3 changes remain bounded to span ownership: the existing
network, cancellation, terminal, and completion-marker policies are preserved.

Raw stream records persist safe optional event summaries independently of the
payload cache, and raw logs persist optional trace/span context. Binary,
protobuf, public oplog, WIT, and SDK consumers carry these fields. Exporter stream
session cancellation metrics count journal records, not unique logical
cancellations. P3 buffered stdout/stderr correlation is sampled at host emission,
not necessarily at the original guest write. These limits are documented.

The live entity opening passes its in-hand span metadata into the invocation
handle; only replay reads the recorded Start. This avoids reading an unrelated
oplog entry for an unpersisted snapshot call. Oracle accepted this fix, and
bug-finder `gol458-entity-spans` run 2 found no bugs. A direct snapshot/entity
execution fixture was not added; that boundary is inspection-covered. Existing
streaming RPC snapshot support is not expanded by this change: its unconditional
recorded-request read already predates span metadata.

The bounded producer/exporter Oracle review accepted the candidate within the
settled best-effort limits tracked in GOL-667. It specifically verified the
primary log caller passes the current span's origin and ID. Exporter acceptance
covers per-opening context/links/timestamps, attribute-before-close ordering,
retry-safe pending spans, agent-type-only service naming, absolute resource
gauges, internal cleanup filtering, inclusive Jump/Revert opening removal, and
independent signal export attempts after source-state advancement. The review
did not independently rerun the source tests. Telemetry bug-finder
`gol458-log-stream-telemetry` run 1 reported no bugs.

Completed source checks:

- Common contract/telemetry tests: 201 passed
  (`.amp/in/gol458-repair-common-tests.log`).
- Combined executor unit tests: 846 passed, 1 ignored with four test threads
  (`.amp/in/gol458-final-executor-unit-retry.log`). The initial high-concurrency
  run had one failure in the unchanged 20 ms timing assertion in
  `await_natural_tail_end_parks_only_after_owned_cursor_work`; the same test and
  complete selected scope passed on rerun. This is not evidence that the timing
  test is reliable under arbitrary contention.
- Seventeen focused runtime tests passed after rebuilding `host-api-tests`
  against the final WIT: guest completed/incomplete reconstruction, invalid
  post-finish attribute mutation, P3 unread response cleanup, RPC
  cancellation/denials, HTTP cancellation/live-tail continuation, and middleware
  acceptance/reconstruction
  (`.amp/in/gol458-final-runtime-rebuilt-fixture.log`). Earlier attempts failed
  on stale fixture interfaces, not replay mismatches.
- Nineteen additional runtime tests passed: RPC committed crash prefixes,
  suspension, fire-and-forget replay, streaming-result spans, polled async-get
  drop, memory recovery at span completion, live-tail operations, P2 HTTP, and
  trace-header propagation (`.amp/in/gol458-final-additional-runtime.log`).
  The existing oplog-reading and invocation-context integration tests also
  passed (`.amp/in/gol458-final-observability.log`). There is one overlapping
  RPC denial test between the seventeen- and nineteen-test runs.
- CLI output schema tests: 17 passed
  (`.amp/in/gol458-final-cli-schema-tests.log`), followed by two more successful
  generated-example property-test runs
  (`.amp/in/gol458-cli-schema-property-reruns.log`).
- Exporter host tests: 25 passed; WASI Clippy with warnings denied passed
  (`.amp/in/gol458-plugin-final-tests.log`,
  `.amp/in/gol458-plugin-final-clippy.log`).
- TypeScript: 987 passed, 20 skipped; Effect contracts/typechecking passed.
  Scala: 713 core tests and 47 targeted oplog tests passed. MoonBit pinned
  bindings regeneration, info, formatting, and wasm checking passed.
- OpenAPI, client, CLI schema, SDK bindings, generated guides, and embedded
  plugin artifacts were regenerated. OpenAPI/schema drift checks passed.
  The plugin descriptor and instrumentation scope use version 1.5.3; the
  embedded WASM validates and has SHA256
  `270616e75963892b6464811b5a4fc89b3564620d65568fdbb15926c91c8ff913`.
- Documentation built 622 pages; version/format checks passed. Existing
  link-check failures remain. The updated walkthrough was rendered and inspected
  (`.amp/in/artifacts/gol458-span-lifecycle-final.png`).

The collector E2E now checks type-only service naming while retaining full
agent identity, nested guest attributes, RPC kinds/parentage/timestamps, and
persisted log IDs. It waits for both RPC operations, each specific method span
(`test1`, `test2`, `test3`), and complete ancestry before asserting, since
different agents export asynchronously. Bug-finder identified that its trace
parser rejected the optional missing OTLP resource field. Oracle additionally
found that the collector's `status: {}` requires a default status code, and that
the leaf invocation could lag the original readiness predicate. A first
count-based correction was insufficient because initialization spans share the
same name. The final predicate matches method identity instead, with an executed
regression rejecting extra initializations, unrelated-trace leaves, and missing
parents while accepting the complete trace. Both parser regressions also pass.
Bug-finder run 4 reported no bugs; Oracle accepted this bounded final test slice
by inspection, without independently running it. The integration test binary
builds successfully (`.amp/in/gol458-final-collector-build.log`), and the readiness
regression passes (`.amp/in/gol458-collector-readiness-test.log`). This is not
whole-plan or collector E2E acceptance.

Completion is still gated on real collector execution of the rebuilt embedded
artifact. Docker Desktop's engine reports that it cannot start after an earlier
disk-full failure, despite disk space now being available. No Docker reset,
prune, or data deletion has been performed. Registry, executor, and worker-service
binaries were rebuilt after the final WASM copy; the collector fixture was also
rebuilt against the current SDK, retaining automatic manifest/guide migrations.
All changes remain local, uncommitted, and unpushed; GOL-458 remains In Progress.
