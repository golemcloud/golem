# GOL-102: optional Durable Streams route customizations

## Status and approved scope

GOL-102 adds optional, code-first configuration for Durable Streams HTTP routes. A method that
uses streams remains a Durable Streams route without any additional annotation, and every omitted
option preserves today's schema-derived behavior.

Protocol baseline: official `durable-streams/durable-streams` `PROTOCOL.md`, Draft 1.0 on `main`,
including byte-based `Stream-Fork-Sub-Offset` semantics for every non-JSON content type.

Approved customizations:

1. Public slot-name overrides.
2. Content-type overrides for byte streams, with a deliberately constrained schema-to-wire
   mapping.
3. `allow_external_writes`.
4. `allow_stream_delete`.
5. `allow_invocation_delete`.
6. Per-route `max_concurrent_readers_per_stream`.
7. Per-route `max_append_requests_per_second_per_stream`.
8. Return the protocol-defined `429` when worker-service admission rejects a new reader.

`allow_forks` is not part of this change. The protocol draft defines fork creation without an
explicit unsupported-fork response, so an opt-out cannot currently be claimed as unambiguously
conformant. Fork support remains enabled and inherits the route's public slot names and content
types.

This is a direct contract change. Do not add compatibility aliases or parsing for an earlier
metadata shape.

The originally considered `text/plain` representation for `stream<string>` is deliberately
deferred. Durable Streams defines non-JSON fork sub-offsets in decoded entity-body bytes and
allows a fork inside a UTF-8 code point. No valid guest `string` can represent such a retained
prefix. Rejecting, rounding, or character-counting that cut would violate the protocol. Supporting
text therefore requires a separately designed byte-authoritative stream with a guest decoding
view, not an HTTP metadata override. GOL-102 keeps string streams in JSON mode.

## Protocol-compatibility decision table

| Customization | Protocol result | Constraint that keeps it compatible |
| --- | --- | --- |
| Public slot aliases | Compatible | Alias only the URL/manifest name; protocol resources remain internally consistent and executor calls use the canonical name. |
| Byte-stream MIME override | Compatible | Only direct `stream<u8>`; only non-JSON, non-`text/*` media types; framing remains raw bytes and SSE remains base64. |
| `allow_external_writes` | Compatible | The resource remains readable; denied POST and write-bearing fork creation return `405`/`403` as specified below without changing protocol framing. |
| `allow_stream_delete` | Compatible | DELETE is an optional operation; denial is `405` with an accurate `Allow`. |
| `allow_invocation_delete` | Compatible | DELETE is an optional operation; denial is `405` with an accurate `Allow`. |
| Reader admission override | Compatible | It changes capacity only; admitted requests retain normal semantics and rejected pre-header requests return protocol-defined `429`. |
| Append-rate override | Compatible | It changes capacity only; rejected appends return protocol-defined `429`. |
| Reader exhaustion status | Compatible with a stated boundary | Worker-service admission returns `429`. Executor-origin failures remain transport/internal errors until a typed cross-layer error is designed. |

`allow_forks` remains excluded because the current draft does not define a clear conformant
unsupported-fork response. `text/plain` remains excluded for the byte-sub-offset reason above.

## Protocol and runtime invariants

- A public slot name is an HTTP alias. The worker executor, invocation session, stream mappings,
  oplog, and durable stream session journal continue to use the canonical schema slot name.
- HTTP policy is static route metadata. It is not copied into the oplog or persisted as executor
  stream state.
- Worker-service translates public slot names and representations at the HTTP boundary before
  using the existing executor RPCs.
- Disabled operations are still routable so worker-service can return `405 Method Not Allowed`
  with an accurate `Allow` header. They are omitted from OpenAPI and CORS preflight advertising.
- Forks use the same public aliases and representations as their origin route. Fork RPCs continue
  to receive canonical slot names and canonical executor payloads.
- Canonical input/output slot names remain globally unique. Direction in metadata makes selectors
  explicit; it does not legalize duplicate canonical names because executor lookup accepts one
  name and searches inputs before outputs.
- Aliases do not weaken canonical-name validation. An invalid or reserved canonical name is still
  a deployment error even when its public alias would be valid.
- Operation switches are route-local gateway policy, not a persisted stream ACL. If another route
  exposes the same underlying stream with different policy, that route's policy applies.
- The per-route reader limit controls live long-poll and SSE admissions for one route and stream.
  Catch-up reads retain the existing global catch-up request-rate limit.
- Both the route limit and the node-wide limit are worker-service-node-local, not cluster-wide
  reservations. A route override does not bypass the node-wide limit.
- Route reader overrides are integers in `1..=16`, where 16 is
  `MAX_LIVE_READERS_PER_STREAM`, the worker executor's existing shared hard ceiling. Executor
  catch-up readers use that same bus, so executor exhaustion can occur before a gateway budget is
  reached.
- Append limits are positive integers. Disabling writes uses `allow_external_writes = false`, not
  an append limit of zero.
- Worker-service reader admission and append exhaustion return `429 Too Many Requests`. Do not use
  `503` for those policy decisions. Once SSE headers have been sent, later exhaustion is an SSE
  stream failure and cannot be converted into an HTTP `429`.

## Metadata contracts

### Agent metadata

Extend `HttpEndpointDetails` with an optional Durable Streams block. The equivalent Rust model is:

```rust
pub struct HttpEndpointDetails {
    // existing fields
    pub durable_streams: Option<DurableStreamRouteOptions>,
}

pub struct DurableStreamRouteOptions {
    pub slots: Vec<DurableStreamSlotOptions>,
    pub allow_external_writes: Option<bool>,
    pub allow_stream_delete: Option<bool>,
    pub allow_invocation_delete: Option<bool>,
    pub load: Option<DurableStreamRouteLoadOptions>,
}

pub enum DurableStreamSlotSource {
    Input(String),
    Output(String),
}

pub struct DurableStreamSlotOptions {
    pub source: DurableStreamSlotSource,
    pub name: Option<String>,
    pub content_type: Option<String>,
}

pub struct DurableStreamRouteLoadOptions {
    pub max_concurrent_readers_per_stream: Option<u32>,
    pub max_append_requests_per_second_per_stream: Option<u32>,
}
```

`Input(name)` identifies a user-supplied top-level stream parameter. `Output("$result")`
identifies a direct stream result or the synthetic result stream for a non-stream result on an
otherwise streaming method. `Output(field)` identifies a direct stream field of an output record.
Direction makes declarations explicit and improves diagnostics, but canonical names must still be
globally unique because the executor's slot lookup is name-only.

Represent this shape in `golem:agent/common@2.0.0`, the component protobuf model, serde/schema
derivations, and all conversion layers. The route block remains absent for ordinary REST routes
and for Durable Streams routes that use defaults.

### Compiled route metadata

Add an optional resolved Durable Streams policy to `CallAgentBehaviour` and its custom-API
protobuf representation. The resolved policy contains:

- one entry per discovered canonical slot, not only overridden slots;
- canonical slot name and public slot name;
- direction/writability;
- public MIME type;
- wire representation: `Json` or `Bytes`;
- concrete operation booleans after applying defaults;
- optional route load limits, because worker-service must layer them over its local global config.

Registry compilation is the only place that resolves SDK declarations against the method schema.
Worker-service must consume the resolved policy and must not repeat schema-selector validation.

## Defaults and validation

Run validation during registry deployment compilation, after resolving schema references and
discovering all slots but before expanding the Durable Streams route family.

1. Reject a Durable Streams block on a method that does not use streams.
2. Resolve every slot selector exactly once. Reject unknown selectors and duplicate declarations.
   Before applying aliases, reject duplicate canonical names across input and output slots and run
   the existing reserved-name rules on every canonical name.
3. Derive unmentioned public names from the canonical names: input parameter, output field, or
   `$result`.
4. Validate all final public names after overrides:
   - 1–64 ASCII characters;
   - only `[A-Za-z0-9._~-]`;
   - not `.` or `..`;
   - not `$result`, `invocations`, `streams`, or `forks`, except that the built-in `$result` public
     default is permitted only for its own direct or synthetic result slot;
   - not prefixed with `__ds`;
   - unique across all input and output slots on the route.
5. Parse content types with the existing `mime` crate, reject parameters, and store the normalized
   essence in lowercase.
6. Resolve content type and wire representation as follows:
   - direct `stream<u8>`: default `application/octet-stream`; an override may be any valid
     non-JSON, non-`text/*` MIME type and uses `Bytes`;
   - `stream<string>`, every other public JSON element, and every synthetic non-stream `$result`:
     only `application/json`, using `Json`;
   - reject `application/json` or `text/*` on bytes and reject all non-JSON MIME labels on
     JSON-backed values.
7. Default all three operation switches to `true`.
8. Reject `allow_external_writes` when the route has no writable input slot. When it is `false`,
   reject an append-rate override because that limit could never be exercised.
9. Require reader limits in `1..=16` and append-rate limits greater than zero.
10. Ensure the resolved policy is included wherever the agent/route fingerprint hashes HTTP route
    metadata; add a regression if this is not already structural.

The first version intentionally excludes native text, NDJSON, protobuf framing for structured
values, content-type parameters, per-slot permission switches, catch-up RPS overrides, node-wide
limits, and fork opt-out.

## SDK surfaces

All SDKs emit the same WIT metadata and defaults. SDK-side checks may improve errors, but registry
validation remains authoritative.

### Rust

Keep configuration nested in the exact `#[endpoint]`, because a method can expose more than one
HTTP endpoint:

```rust
#[endpoint(
    post = "/process",
    durable_streams(
        input("input", name = "messages"),
        output("$result", name = "results", content_type = "application/vnd.golem.events"),
        allow_external_writes = true,
        allow_stream_delete = false,
        allow_invocation_delete = false,
        max_concurrent_readers_per_stream = 8,
        max_append_requests_per_second_per_stream = 25,
    )
)]
async fn process(&mut self, input: AgentStream<String>) -> AgentStream<u8>;
```

Extend the macro parser, generated call to `get_http_endpoint_details`, SDK HTTP metadata builder,
and macro/runtime tests. Reject duplicate keys and malformed nested entries at compile time where
the attribute syntax provides enough information. To disable writes, set
`allow_external_writes = false` and omit `max_append_requests_per_second_per_stream`.

### TypeScript

Add `durableStreams` to endpoint options and compile it as plain data:

```typescript
http.post("/process", {
  durableStreams: {
    slots: [
      { source: "input", slot: "input", name: "messages" },
      {
        source: "output",
        slot: "$result",
        name: "results",
        contentType: "application/vnd.golem.events",
      },
    ],
    allowExternalWrites: true,
    allowStreamDelete: false,
    allowInvocationDelete: false,
    load: {
      maxConcurrentReadersPerStream: 8,
      maxAppendRequestsPerSecondPerStream: 25,
    },
  },
})
```

Add public option types, WIT compilation, runtime tests, and type tests. Keep values plain and
serializable; do not add a second decorator-style API. To disable writes, set
`allowExternalWrites: false` and omit `maxAppendRequestsPerSecondPerStream`.

### Effect TypeScript

Expose the same object shape in `Http.post`/`Http.endpoint` options and add the pipeable
`Http.withDurableStreams(options)` combinator:

```typescript
Http.post("/process", {
  durableStreams: {
    slots: [
      { source: "input", slot: "input", name: "messages" },
      {
        source: "output",
        slot: "$result",
        name: "results",
        contentType: "application/vnd.golem.events",
      },
    ],
    allowExternalWrites: true,
    allowStreamDelete: false,
    allowInvocationDelete: false,
    load: {
      maxConcurrentReadersPerStream: 8,
      maxAppendRequestsPerSecondPerStream: 25,
    },
  },
})
```

Both construction styles must produce identical endpoint metadata. Extend public type tests,
runtime compilation tests, and generated declarations. To disable writes, set
`allowExternalWrites: false` and omit `maxAppendRequestsPerSecondPerStream`.

### Scala

Use method annotations that compile into the method's HTTP endpoint metadata:

```scala
@endpoint(method = "POST", path = "/process")
@durableStreamSlot(
  endpointMethod = "POST",
  endpointPath = "/process",
  source = "input",
  slot = "input",
  name = "messages"
)
@durableStreamSlot(
  endpointMethod = "POST",
  endpointPath = "/process",
  source = "output",
  slot = "$result",
  name = "results",
  contentType = "application/vnd.golem.events"
)
@durableStreams(
  endpointMethod = "POST",
  endpointPath = "/process",
  allowExternalWrites = true,
  allowStreamDelete = false,
  allowInvocationDelete = false,
  maxConcurrentReadersPerStream = 8,
  maxAppendRequestsPerSecondPerStream = 25
)
def process(input: AgentStream[String]): AgentStream[Byte]
```

Scala already collects multiple `@endpoint` annotations. The DS annotations therefore identify
their endpoint by method and path; those selector fields may be omitted only when the method has
exactly one endpoint. Reject a missing or ambiguous endpoint match. Extend annotation models,
macro extraction/emission, compile-time validation tests, and test-agent linking. To disable
writes, set `allowExternalWrites = false` and omit the append-rate argument.

### MoonBit

Follow the existing derive-attribute style:

```moonbit
#derive.endpoint(post="/process")
#derive.durable_stream_slot(
  input="input",
  name="messages",
)
#derive.durable_stream_slot(
  output="$result",
  name="results",
  content_type="application/vnd.golem.events",
)
#derive.durable_streams(
  allow_external_writes=true,
  allow_stream_delete=false,
  allow_invocation_delete=false,
  max_concurrent_readers_per_stream=8,
  max_append_requests_per_second_per_stream=25,
)
pub async fn process(
  self : Processor,
  input : AgentStream[String],
) -> AgentStream[Byte] {
  // ...
}
```

MoonBit currently permits at most one `#derive.endpoint`, so bind the DS attributes to that
endpoint and reject them when it is absent. Extend parsed method metadata, validation, generated
WIT-record source, snapshots, and generated package interfaces. To disable writes, set
`allow_external_writes=false` and omit the append-rate field.

## Implementation sequence

### 1. Establish shared agent metadata

1. Add the option, selector, slot, and load types to the root WIT source.
2. Run the repository WIT synchronization/generation workflow rather than editing SDK dependency
   copies by hand.
3. Add the equivalent `golem-common` model and component protobuf messages.
4. Update WIT, protobuf, schema, serde, and binary conversion tests with asymmetric input/output
   examples and omitted-option defaults.
5. Verify a metadata round trip preserves direction, `$result`, explicit `false`, and absent
   values distinctly.

### 2. Implement registry resolution and validation

1. Refactor current slot discovery into one function returning canonical selector, schema,
   direction, default MIME type, and default public name.
2. Apply slot overrides and route options according to the validation rules above.
3. Return a resolved policy alongside the non-stream HTTP input from Durable Streams route
   validation.
4. Store that policy in `CallAgentBehaviour`.
5. Add deployment tests for every valid representation and each invalid boundary: unknown source,
   duplicate canonical names across directions, duplicate declaration, post-override collision,
   invalid canonical name despite a valid alias, the valid built-in `$result` default, an illegal
   `$result` alias, other reserved names, 64/65-character names, invalid MIME, invalid schema/MIME
   pairing, meaningless write options, zero limits, and reader limits 16/17.

### 3. Carry resolved policy through compiled routes

1. Add the compiled policy types to `golem-service-base` and custom-API protobuf.
2. Update both protobuf conversion directions and compiled-route binary encoding.
3. Update all `CallAgentBehaviour` constructors and fixtures.
4. Verify compiled route serialization preserves public-to-canonical mappings and explicit policy.

### 4. Add public/canonical slot translation in worker-service

1. Centralize lookup of a public slot into the resolved slot descriptor.
2. Reject unknown public names without probing the executor with them.
3. Pass canonical names to slot PUT, read, append, delete, and fork RPCs.
4. Translate executor slot lists back to public names in session and fork manifests.
5. Keep canonical names out of public URLs, manifests, OpenAPI extensions, operation IDs, and
   problem responses.
6. Update fork source-path parsing so it validates public aliases while sending canonical source
   and target slot names to the executor.
7. Test different input/output aliases, `$result` aliases, unknown aliases, manifests, tombstones,
   lazy POST, and fork reads/writes.

### 5. Implement operation policy

1. Compute `Allow` from resource kind, slot direction, and resolved route switches instead of
   using the current fixed string.
2. Before reading a request body or calling worker-service/executor APIs:
   - deny input-slot POST when external writes are disabled;
   - deny slot DELETE when stream deletion is disabled;
   - deny session DELETE when invocation deletion is disabled.
3. Return `405` with the computed `Allow` header for each denial.
4. Fork creation remains available. When external writes are disabled, allow an empty, open fork
   creation but reject nonempty initial content or `Stream-Closed: true` with the existing
   read-only `403` semantics before calling the executor.
5. Keep denied method routes compiled so requests reach this policy boundary.
6. Add tests proving denied requests do not invoke the executor-facing worker service and that a
   second route to the same stream applies its own policy rather than inheriting an ACL.

### 6. Implement HTTP representations and MIME handling

1. Replace binary-vs-JSON inference in worker-service encoding with the compiled `Json`/`Bytes`
   representation and public MIME type.
2. For JSON, preserve existing one-level array flattening, schema validation, batching, and JSON
   array reads.
3. For bytes, preserve packed-u8 append/read behavior while emitting the configured public MIME.
4. Compare normalized request MIME essence against the public MIME. Preserve close-only empty-body
   behavior, which ignores Content-Type.
5. Emit the public MIME in slot PUT, HEAD, GET, manifests, and fork responses.
6. SSE sends JSON directly; byte representations use base64 and
   `Stream-SSE-Data-Encoding: base64`. SSE response Content-Type remains `text/event-stream`.
7. Validate public fork Content-Type at worker-service, then translate to the executor's canonical
   typed/packed representation so no executor contract changes.
8. Add encoding tests with Unicode JSON strings, custom binary bytes that are not UTF-8, content
   mismatch, empty close, SSE JSON, SSE binary base64, fork inheritance, and a fork sub-offset
   inside a UTF-8-looking byte sequence to prove raw-byte preservation.

### 7. Apply route load overrides

1. Include route identity in live-reader and append-rate accounting keys so different routes do
   not share per-route budgets accidentally.
2. Pass the effective per-stream reader limit into `try_acquire_reader`; retain the global
   node-wide limit as a second independent check.
3. Pass the effective append rate into `check_append`; retain the existing one-second window.
4. Use the worker-service global per-stream value when the route override is absent.
5. Return `429` for worker-service per-route/per-node reader admission and append exhaustion. Do
   not infer executor `ReaderLimit` from an error string; its type is currently lost across RPC.
   A typed executor exhaustion response is separate follow-up work if product requirements demand
   a universal `429` guarantee.
6. Test default inheritance, lower and higher route overrides within the 16-reader ceiling,
   route-key isolation, permit release, node cap, append windows, and all gateway rejection
   statuses. Also test two routes targeting the same underlying stream and executor live/catch-up
   reader overlap so the distinction between gateway admission and the shared executor bus remains
   explicit.

### 8. Align OpenAPI and browser behavior

1. Extend `StreamSlotSchema` with public name, public MIME type, representation, and effective
   operation policy from compiled metadata.
2. Emit concrete public slot paths and `x-golem-stream-slot.name` values.
3. Use byte or JSON schemas and media types appropriate to each representation.
4. Omit disabled POST and DELETE operations while retaining runtime denial routes.
5. Advertise `429` for gateway live-reader admission as well as catch-up and append limiting.
6. Generate CORS preflight routes for each concrete public slot path rather than relying on the
   current wildcard `/streams/{slot}` grouping. This permits POST only for writable public slots
   and omits route-disabled operations while retaining runtime denial routes.
7. Add `Allow` to `Access-Control-Expose-Headers` and expose only the other response headers needed
   by enabled methods.
8. Update OpenAPI and CORS snapshots plus generated-client coverage for aliases, custom binary
   MIME, disabled operations, and different input/output method sets.

### 9. Implement and verify each SDK

After WIT synchronization, implement the five surfaces above. For every SDK, add one metadata
round-trip/golden test containing all options and focused invalid-syntax tests. Also verify the
zero-annotation path still emits no route block and therefore retains all defaults.

### 10. End-to-end acceptance

Add one deployed annotated streaming agent fixture and cover these externally observable cases:

1. The public alias works and the canonical name returns 404.
2. A Unicode string input retains JSON message semantics; `text/plain` for that slot is rejected at
   deployment.
3. A custom-MIME byte output returns exact bytes, exact Content-Type, and base64 SSE data; a fork
   can retain a byte prefix that ends inside a UTF-8-looking sequence.
4. Session manifests expose public names and public MIME types only.
5. External POST, stream DELETE, and invocation DELETE opt-outs each return 405 with exact `Allow`
   and make no state change.
6. OpenAPI omits all three disabled operations and describes the aliased representations.
7. Gateway reader admission and append route limits reject with 429 while a second route retains
   its own budget; shared executor reader exhaustion remains distinguishable.
8. Fork creation and reads use public aliases and inherit the public content type. With external
   writes disabled, an empty open fork is accepted and a fork carrying initial content or closure
   is rejected with 403.
9. An unannotated route retains current names, JSON/octet-stream defaults, enabled operations,
   and global load defaults.

Use the reference Durable Streams client where it supports the operation; use direct HTTP only
for Golem route-policy assertions not represented by that client.

## Verification commands

Start narrow and expand after each layer:

```shell
cargo test -p golem-common --lib -- agent --report-time
cargo test -p golem-registry-service --lib -- durable_streams --report-time
cargo test -p golem-service-base --lib -- custom_api --report-time
cargo test -p golem-worker-service --lib -- durable_streams --report-time
cargo test -p golem-worker-service --lib -- openapi --report-time
cargo test -p golem-worker-executor --lib -- stream_cut --report-time
```

Rust SDK:

```shell
cargo test -p golem-rust-macro -- agentic --report-time
cargo test --manifest-path sdks/rust/Cargo.toml -p golem-rust --features export_golem_agentic
```

TypeScript SDK:

```shell
cd sdks/ts
npx pnpm --filter @golemcloud/golem-ts-sdk run test
npx pnpm --filter @golemcloud/golem-ts-sdk run lint
```

Effect SDK:

```shell
cd sdks/effect
npm run typecheck
npm test
npm run lint
npm run format:check
```

Scala SDK:

```shell
cd sdks/scala
sbt "++3.8.2; modelJVM/test; modelJS/test; macros/test; testAgents/fastLinkJS"
sbt scalafmtCheckAll
```

MoonBit generator and SDK:

```shell
cd sdks/moonbit/golem_sdk_tools
moon check --warn-list +unnecessary_annotation
moon test
moon fmt
moon info

cd ../golem_sdk
moon check --target wasm
moon test --target wasm
moon info
```

Build the selected cross-SDK test components using their documented targeted workflows, then run
the focused CLI integration filter for Durable Streams. Do not run the full root test suite by
default. Finish with package-scoped formatting and lint checks, inspect all generated artifacts,
and run the repository pre-PR checklist appropriate to the final changed paths.

## Expected worker/executor impact

The estimates below are focused implementation days after shared metadata and registry policy are
available. They overlap, so they are sizing guidance rather than additive commitments.

| Customization | Worker-service work | Worker-executor work |
| --- | --- | --- |
| Slot aliases | Medium, roughly 2–3 days: translate every slot/fork path and manifest in both directions and update OpenAPI. | None: continue receiving canonical names. |
| Byte MIME override | Small/medium, roughly 1–2 days: use compiled MIME for request checks, responses, manifests, SSE/base64, forks, and OpenAPI. | None: representation remains existing `PackedU8`. |
| External-write switch | Small, roughly 1 day: POST denial, fork initial-content/closure policy, `Allow`, OpenAPI, and CORS. | None: denied requests stop at the gateway. |
| Stream/invocation delete switches | Small, roughly 1 day together: route-local denial, dynamic `Allow`, OpenAPI, and CORS. | None. |
| Reader override and gateway 429 | Medium, roughly 1–2 days: route-aware permits, node-local semantics, and pre-header status mapping. | None for the approved guarantee. A universal executor-origin 429 would be separate typed-RPC error work. |
| Append-RPS override | Small, roughly 0.5–1 day: route-aware key and effective rate. | None. |
| Shared CORS/OpenAPI work | Medium, roughly 1–2 days: concrete public-slot preflight routes, enabled method sets, schemas, and exposed `Allow`. | None. |

Expected worker-service runtime total: approximately **6–9 focused days**, including tests and
overlapping CORS/OpenAPI work. Worker-executor production work for the approved boundary:
**none**. Rerun focused executor fork/read tests because GOL-102 relies on existing packed-byte
semantics, but do not add RPC fields, oplog entries, or session-journal state.

The excluded native-text design is materially larger executor work: it needs a byte-authoritative
durable representation and a specified guest decoding contract. It is not a hidden extension of
this estimate.

## Completion criteria

- Every SDK emits semantically identical optional metadata.
- Registry rejects all illegal selectors, names, MIME mappings, option combinations, and limits at
  deployment time.
- Public names and MIME types are used consistently across all normal and fork HTTP operations,
  manifests, OpenAPI, and SSE.
- Disabled operations return 405 with accurate `Allow`, are absent from OpenAPI/CORS advertising,
  and perform no executor call or state change.
- Route limits layer over worker-service defaults, remain below the executor ceiling, are isolated
  per route, and return 429 when gateway admission rejects them. Tests document that executor
  catch-up/live-reader exhaustion is a separate shared limit.
- Unannotated streaming methods retain today's behavior.
- `text/plain` on a string stream is rejected rather than implemented with nonconformant fork
  semantics.
- No worker-executor, oplog, durable stream session, or persistence contract changes are present.
