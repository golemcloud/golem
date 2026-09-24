# `@golemcloud/effect-golem`

Effect-native TypeScript SDK for Golem 1.6 agents. It uses Effect 4 schemas and effects while
targeting Golem's WASI Preview 3 agent, tool, and tool-middleware contracts.

> The package currently uses Effect 4 beta APIs. Keep the `effect` version generated for an
> application aligned with the SDK.

## Install and define an agent

```console
npm install @golemcloud/effect-golem effect
```

The 1.6 vocabulary is `id` for agent identity, `input` for method input, and the client attached to
the definition. `defineAgent` creates an inert specification; `.implement({ init, methods })`
registers it.

```ts
import { Effect, Ref, Schema } from "effect"
import { defineAgent, method } from "@golemcloud/effect-golem"

export const Counter = defineAgent({
  name: "Counter",
  mode: "durable",
  id: { name: Schema.String },
  methods: {
    value: method({ input: {}, success: Schema.Number, readOnly: true }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
  },
})

Counter.implement({
  init: () => Ref.make(0),
  methods: (count) => ({
    value: () => Ref.get(count),
    add: ({ by }) => Ref.updateAndGet(count, (value) => value + by),
  }),
})
```

`method` optionally accepts `error`, `description`, `promptHint`, HTTP endpoints, and `readOnly`.
Read-only metadata may be `true` or `{ cache }`; principal-aware caching is derived from a declared
`PrincipalSchema` input. Public modules are namespaces from
the root (`Agent`, `Client`, `Config`, `Durability`, `Snapshot`, `Tool`, etc.); only
`defineAgent`, `defineAgentClient`, `defineConfig`, and `method` are flat DSL aliases. Database adapters and standalone
middleware use the documented sub-imports. `internal/*` and `host/*` are not public imports.

## Typed clients

Keep a definition in a module without calling `.implement` when another component only needs its
client. Durable definitions expose `client.get`, `getPhantom`, and `newPhantom`; ephemeral
definitions expose `getPhantom` and `newPhantom`, but not ordinary `get`. Calls, triggers, and
schedules take one typed input object.

```ts
const program = Effect.scoped(
  Effect.gen(function* () {
    const counter = yield* Counter.client.get({ name: "orders" })
    yield* counter.add({ by: 2 })
    yield* counter.add.trigger({ by: 3 })
    const scheduled = yield* counter.add.schedule("10 minutes", { by: 1 })
    // yield* scheduled.cancel()
  }),
)
```

Awaited RPC is interruptible: fiber interruption cancels the result handle. Cancellation is best
effort; remote side effects may already have happened. Triggering and scheduling methods whose
schema contains a live stream is rejected because streams require an awaited invocation. An
ephemeral call returns `{ metadata, value }`, and an ephemeral trigger/schedule exposes invocation
metadata. Config overrides are passed as the second argument to `client.get`/`newPhantom`.

Use `defineAgentClient` for caller-owned clients. A full client definition has a declared name,
constructor schema, lifecycle mode, and methods; it exposes `agentId` and `client` factories without
registering an implementation. A method-only `{ methods }` definition has neither factory nor
identity constructor; it binds without discovery and assumes durable result semantics. An
unimplemented `defineAgent` spec remains usable as a shared implementation/caller definition.
`identity.client(clientDefinition)` validates the declared name and constructor schema before opening RPC.
`Client.bind(identity, clientDefinition)` and `DynamicClient.bind(identity)` remain lower-level functions.

```ts
import { Effect, Schema } from "effect"
import { AgentIdentity, defineAgentClient, method } from "@golemcloud/effect-golem"
import type * as CoreTypes from "golem:core/types@2.0.0"

const Target = defineAgentClient({
  name: "Target",
  id: { name: Schema.String },
  methods: { echo: method({ input: { message: Schema.String }, success: Schema.String }) },
})

const calls = (inputTree: CoreTypes.SchemaValueTree) =>
  Effect.scoped(
    Effect.gen(function* () {
      const identity = yield* Target.agentId({ name: "main" })
      const full = yield* identity.client(Target)
      const first = yield* full.echo({ message: "full" })
      const methods = defineAgentClient({ methods: Target.methods })
      const second = yield* (yield* identity.client(methods)).echo({ message: "method only" })
      const parsed = yield* AgentIdentity.parse(identity.encoded)
      const dynamic = yield* parsed.dynamicClient()
      const raw = yield* dynamic.method("echo").invoke(inputTree)
      // Dynamic methods accept native SchemaValueTree inputs and return metadata plus native values.
      return { first, second, raw }
    }),
  )
```

Complete ephemeral specs require a phantom ID for `agentId(input, phantomId)` and reject generic
existing-ID binding. Address known or fresh phantoms through their factories instead.

For runtime-selected targets, use `Reflection.getAgentType(name)`,
`getAgentTypeByAgentId(parsedIdentity)`, or `getAllAgentTypes`. Each immutable registration exposes
constructor/method `SchemaRef` values with JSON/value validation and JSON Schema rendering.
Narrow `type.mode` to select its lifecycle factory:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const invokeCounter = Effect.scoped(
  Effect.gen(function* () {
    const type = yield* Reflection.getAgentType("Counter")
    if (type === undefined || type.mode !== "durable") return undefined
    const client = yield* type.client.get({ name: "main" })
    const add = yield* client.method("add")
    return yield* add.invoke({ by: 1 })
  }),
)
```

Reflected invocation results contain metadata and, for non-unit outputs, `value`.
Ephemeral `newPhantom` returns a client whose actual identity arrives in invocation metadata;
`getPhantom(input, phantomId)` addresses a known phantom without offering ordinary existing-ID
binding. Durable `newPhantom` returns `{ client, agentId, phantomId }`, with a parsed `agentId`.
Durable reflected types also bind through `Client.bind(identity, reflectedType)` after name and
constructor validation. Discovery misses remain `undefined`; host and malformed-schema failures
enter the typed error channel. `invokeValue`, `triggerValue`,
and `scheduleValue` accept native WIT schema-value trees when JSON cannot represent capabilities.
`parsedIdentity.dynamicClient()` binds without discovery and uses value-only methods; callers
must supply the correct remote method definitions. Both APIs use scopes and fiber interruption and expose
the same structured remote-call errors as typed clients.

The four client approaches are: Normal RPC through the ordinary client for a shared source
definition; method-only or full caller-defined static clients; discovered clients backed by an
immutable deployed schema snapshot; and fully dynamic schema-native clients for existing durable
identities. Method-only clients do not own
identity, lifecycle, mode, or config declarations. Full clients own those declarations and the
matching durable, phantom, or ephemeral factories. Dynamic clients neither discover nor create.

Normal RPC and caller-defined static clients encode through their local schemas before opening RPC;
full binding also checks the declared name and constructor shape. Discovered clients apply all
snapshot restrictions and validate
declared result cardinality and shape. The host remains authoritative for visibility,
authorization, effective configuration, durable identity resolution, and deployed input schemas.
Typed inputs use `Schema.optional(...)` normally. Canonical reflected JSON records contain every
field and use `null` for an absent option. Wide integers are decimal strings: duration is
`{ nanoseconds: "..." }` and quantity uses a decimal-string `mantissa`; integers through 32 bits
remain numbers. Streams and opaque capabilities require `*Value` APIs and transfer ownership once.

## Configuration and opaque secrets

`Schema.Redacted(inner)` declares a secret. The config wire value is an opaque host handle;
`config.apiKey.get` asks the secrets host to reveal and decode it only at read time, then returns an
Effect containing `Redacted<inner>`. Do not log, stringify, cache, snapshot, or put secret values in
RPC overrides.

```ts
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method } from "@golemcloud/effect-golem"

class ServiceConfig extends defineConfig("Service.Config", {
  endpoint: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
  tuning: Schema.Struct({ retries: Schema.Number }),
}) {}

const Service = defineAgent({
  name: "Service",
  id: { tenant: Schema.String },
  config: ServiceConfig,
  methods: { endpoint: method({ input: {}, success: Schema.String }) },
})

Service.implement({
  init: () =>
    Effect.gen(function* () {
      const config = yield* ServiceConfig
      const endpoint = yield* config.endpoint
      const apiKey = yield* config.apiKey.get
      // Reveal only at the boundary that needs plaintext:
      void Redacted.value(apiKey)
      return endpoint
    }),
  methods: (endpoint) => ({ endpoint: () => Effect.succeed(endpoint) }),
})
```

Plain leaves are invocation-local cached; secret reads are not cached. `Schema.optional(...)`
declares optional values. Nested structs map to path segments. RPC overrides include only non-secret
leaves:

```ts
yield *
  Service.client.get(
    { tenant: "acme" },
    { overrides: { endpoint: "https://example.test", tuning: { retries: 2 } } },
  )
```

Config is never stored in snapshots. Restoration reads current host values.

## Snapshots and restoration

Snapshotting agents initialize runtime state with `init(id)`. The implementation's snapshot
strategy restores that state before `methods(state)` constructs handlers. Custom restoration
receives a `SnapshotRestorationContext` containing `id`, `principal`, `phantomId`, `agentId`,
`parsedAgentId`, and current `config`; it must not perform fresh-instance initialization effects.

```ts
import { Effect, Ref, Schema } from "effect"
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem"

const definition = defineAgent({
  name: "SnapshotCounter",
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: { value: method({ input: {}, success: Schema.Number }) },
})

const handlers = (state: Ref.Ref<{ readonly count: number }>) => ({
  value: () => Ref.get(state).pipe(Effect.map((value) => value.count)),
})

definition.implement({
  init: () => Ref.make({ count: 0 }),
  methods: handlers,
  snapshot: Snapshot.ref<{ count: number }>(),
})
```

For SQLite snapshots, add `databases: state => ({ main: state.database })` to the snapshot strategy;
the SDK restores each declared image before constructing methods. DDL must be idempotent. A
`Snapshot.custom(...)` implementation uses `{ save(state), restore(saved, context) }` with raw
bytes. Snapshot schema evolution remains the application's responsibility.

Auto snapshots can declare `databases: ["main"] as const`; expose the corresponding SDK
`SqliteClient` or `node:sqlite` `DatabaseSync` from the implementation snapshot strategy. Every
declared database must be present, be in autocommit mode, and have no extra attached schemas.
External Postgres/MySQL/Ignite data is not part of a worker snapshot.

## Agent streams

Use `WitTypes.AgentStream(itemSchema)` inside any input or output schema and pass native Effect
`Stream<T>` values. Local streams are reusable. Streams received from Preview 3 endpoints are
demand-driven, single-reader, and affine: consuming or forwarding one transfers ownership, so a
received stream must not be reused.

Early `break`, `return()`, or Effect interruption closes the readable endpoint and awaits local
cleanup. A downstream drop stops future source pulls and eventually calls the producer iterator's
`return()` once, but P3 cannot interrupt an arbitrary pending `next()` or promise cleanup before a
later invocation. Terminal errors are not transported; model recoverable failures as stream items
(for example `Result<T, E>`). Streams cannot be snapshotted, triggered, or scheduled.

## External Durable Streams

`DurableStreams.readJson(schema, options)` and `readBytes(options)` return lazy native Effect
streams. They read finite host batches, starting with catch-up and following with long-poll
(default) or SSE. Offsets and cursors are opaque. `offset: "now"` is resolved once per stream
execution; an empty live batch is not EOF, and a closed batch emits its payload before EOF.
Each execution of a local stream starts at its configured offset. Native agent-stream forwarding
works without an iterator or Promise adapter.

Each Stream execution acquires one journaled reader resource and releases it at EOF, failure,
interruption, or downstream termination. Writers acquire one journaled resource in the caller's
Effect scope; keep that scope open through appends and retries. Constructors record immutable
connection descriptors and the borrowed secret's pinned identity without making HTTP requests
or revealing plaintext. Reads carry only checkpoint/transport/content-type; appends carry only
payload/sequence/close. Repeated attempts reuse the same resource.

Scope interruption returns promptly. If a finite WIT attempt is still in flight, actual resource
disposal waits for its Promise to settle, because the method still borrows the native handle.
No new operations may start after scope exit; interrupting an append within an open writer scope
still permits `retryPending` on the same resource.

Before passing a host-dependent stream to an agent method (or returning it through an agent-stream
schema), capture its services within the invocation:

```ts
const forward = Effect.gen(function* () {
  const source = DurableStreams.readBytes({ url })
  const context = yield* Effect.context<Stream.Services<typeof source>>()
  return yield* sink.collect({ bytes: source.pipe(Stream.provideContext(context)) })
})
```

```ts
import { Effect, Schema, Stream } from "effect"
import { DurableStreams, defineConfig } from "@golemcloud/effect-golem"

class StreamConfig extends defineConfig("Stream.Config", {
  bearer: Schema.Redacted(Schema.String),
}) {}

const roundtrip = Effect.gen(function* () {
  const cfg = yield* StreamConfig
  const auth = yield* cfg.bearer.borrow // opaque handle; does not reveal the token
  const options = { url: "https://streams.example/events", auth }
  const writer = yield* DurableStreams.makeJsonWriter(Schema.String, options)
  yield* writer.append(["first", "last"], { close: true })
  return yield* DurableStreams.readJson(Schema.String, options).pipe(Stream.runCollect)
}).pipe(Effect.scoped)
```

Writers serialize concurrent effects and retain one immutable producer ID/epoch/sequence, body,
and close flag after a failed or interrupted append. New data is rejected while `hasPending` is
true: explicitly run `writer.retryPending` to resolve the same request. `writer.close` consumes
a sequence too. The pending operation also retains its automatic retry budget and interrupted
backoff; an explicit retry does not reset either. A receipt's `nextOffset` may be absent on a duplicate acknowledgement; there is
no duplicate boolean. Finite host attempts have bounded retries (`maxRetries`, `retryDelayMs`,
`timeoutMs`) and honor Retry-After. Fiber cancellation stops waiting but does not prove the remote
append was cancelled; its retained request must still be resolved.

JSON uses the SDK's schema codecs and canonical JSON representation. Arrays inside a message
stay arrays. The default codec rejects unsafe integers rather than rounding them. For exact
unquoted 64-bit integers, use `WitTypes.Uint64` with deterministic `decode: BigInt` / `encode: String`
callbacks; callbacks receive one complete message and schema validation still applies.

Producer identity generation uses the runtime's durable randomness. Replay must reproduce the
same construction and operation order; a fork retains its producer identity and must not be
treated as an independent producer. These stateful writer objects and live readers are not
snapshot values. Unit-test host fakes validate SDK state transitions, not executor crash recovery.

## Generated Effect bridges

Effect components receive Effect-native guest clients for their manifest `dependencies.agents`
and `dependencies.tools` during `golem build`. These clients preserve the target's schema without
requiring a duplicate `defineAgent` or tool definition. Generated methods take positional arguments,
unlike definition-derived clients, which take an input object.

For example, a generated guest client for a peer with a nested streaming method is used inside an
agent handler as follows:

```ts
import { Effect, Stream } from "effect"
import { TsPeer } from "ts-peer-guest-client"

const program = Effect.scoped(
  Effect.gen(function* () {
    const peer = yield* TsPeer.get("peer-1")
    const input = Stream.make({ id: 1, values: [9, 12] })
    const output = yield* peer.nestedStream("fx", input)
    return yield* output.items.pipe(Stream.runCollect)
  }),
)
```

Guest bridges use the same affine received streams and capability handles as the SDK. Their host
service requirements flow through the Effect environment; the agent dispatcher supplies live
services. Stream-free methods also expose `.trigger(...)` and `.schedule(...)`, with cancelable
scheduling. Streaming methods cannot be triggered or scheduled.

Generated tool clients expose Effect stdin/stdout streams and an Effect result. Consume stdout and
the result concurrently when the tool can block writing its output:

```ts
import { Effect, Stream } from "effect"
import { TsCrossStreamingClient } from "ts-cross-streaming-tool-guest-client"

const program = Effect.scoped(
  Effect.gen(function* () {
    const invocation = yield* TsCrossStreamingClient.create().ts_cross_streaming(
      "label",
      Stream.make(new TextEncoder().encode("hello")),
    )
    return yield* Effect.all(
      [invocation.result, invocation.stdout.pipe(Stream.decodeText(), Stream.runCollect)],
      { concurrency: "unbounded" },
    )
  }),
)
```

Scope closure cancels the invocation and releases owned output streams. Tool failures retain their
declared payloads separately from RPC failures, and decoding validates the exact schema graph.

For an external Effect application, request an external bridge in the manifest and run
`golem generate-bridge --language effect`:

```yaml
bridge:
  effect:
    external:
      agents: my-app:producer
      outputDir: bridge/effect
```

Each generated `<agent>-client` directory is a buildable npm package. Its `configure` function uses
the same configuration as the TypeScript bridge. Constructors and calls return Effects, streaming
values are Effect `Stream`s (including nested streams), and streaming calls require a scope.
Fiber interruption aborts pending calls; successful streaming calls keep their transport alive
until scope closure. External stream-free trigger/schedule methods use generated suffixes such as
`echotrigger(...)` and `echoschedule(...)`. External bridges use the canonical TypeScript HTTP and
WebSocket transport and do not depend on guest WIT modules. As with the TypeScript generator,
external bridges support agents; tool bridges are guest-only.

## Tools and permission-card transfer

`Tool.toolDefinition(name)` builds nested, typed commands. `.implement(...)` registers a tool guest;
`Tool.client(definition)` derives a camel-cased Effect client. Bodies define positional/named input,
declared errors, a structured return, and optional stdin/stdout byte streams.

```ts
import { Effect, Schema } from "effect"
import { Tool } from "@golemcloud/effect-golem"

const Echo = Tool.toolDefinition("echo").body((body) =>
  body.positional("message", Schema.String).returns(Schema.String),
)

Echo.implement({
  echo: ({ message }) => Effect.succeed(message),
})

const echo = Tool.client(Echo)
const result = yield * echo({ message: "hello" })
```

Use `Tool.err(name, value)` for a declared tool error and `Tool.ok(value)` where an explicit success
carrier is needed. Tool clients cancel the host future and close owned streams when their scope
ends.

For a tool selected at runtime, `Reflection.getToolType(name)` and `getAllToolTypes` discover
caller-visible registrations. The selected command exposes its canonical path, aliases, ordered
arguments, input schema, result schema, and child commands. Namespace-only nodes remain visible
but have no callable body.

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const call = Effect.gen(function* () {
  const tool = yield* Reflection.getToolType("echo")
  if (tool) {
    const command = tool.client.command([])
    return yield* command.invokeJson({ message: "hello" })
  }
})
```

`invokeJson` and `invokeValue` validate inputs before opening RPC and check declared outputs.
Canonical JSON records include every argument key; use `null` for absent optional values.
`startJson` and `startValue` expose scoped stdout, result, concurrent collection, and cancellation
for pending calls. Use them when stdout is required. `Reflection.DynamicToolClient` accepts a
caller-packed value when the deployed schema is unavailable and does not infer validation rules.
Reflected failures are typed Effect errors, including `ToolReflectionError` for malformed output.

`WitTypes.PermissionCard({ polymorphic })` represents a permission card. Cards are opaque affine
capabilities: successful encoding transfers the exact handle across agent RPC, tool calls, and
middleware. The sender must not inspect or reuse a transferred card. Transactional graph encoding
does not consume it if validation or another sibling conversion fails, so a corrected retry remains
possible. The SDK intentionally has no card inspection, wallet, derivation, or installation API.

## Tool middleware

Standalone middleware imports only the middleware-safe entry point:

```ts
import { NoParameters, universal } from "@golemcloud/effect-golem/middleware"

universal({
  name: "audit",
  parameters: NoParameters,
  handler: (invocation, underlying) =>
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin),
})
```

`universal` transparently handles any tool using wire values. Every middleware declares an Effect
Schema for installation `parameters`; decoded values are available as `invocation.parameters` or
typed-handler `context.parameters`. `NoParameters` is the explicit empty-record schema.
`typed({ parameters, presented, expected?, handler })` projects typed input/output/error and a definition-derived `context.underlying` client;
it can present a different definition from the wrapped tool. Both support aliases, docs, and a
per-invocation Effect `layer`. Underlying access and streams are affine and valid only for that
invocation.

`parameters` must be a static Effect `Schema` (streams, secrets, and other installation-time-ineligible capabilities are rejected). `underlying.start(...)` and typed command `.start(...)` return scoped started invocations whose `get`, optional `stdout`, and `cancel` effects are independent. Calls may overlap; consume stdout and `get` concurrently when needed. Scope closure disposes the observer and owned streams but does **not** cancel the tool call—run `started.cancel` explicitly to cancel it.

When a handler returns, the underlying rejects new admissions but does not implicitly cancel calls already admitted. Cleanup releases their observers after pending observation is safe. For a command declaring stdout, return/select a stream with the typed `context.stdout` callback (or the universal result's `stdout`). The SDK forwards it into the host-provided writer, calls `finish` after clean EOF, and calls `fail` on forwarding failure; middleware never owns that writer directly.

The single `agent-guest` build world exports agents, tools, snapshots, and tool middleware. It
supports ordinary, standalone-middleware, and combined components; discovery returns empty lists
for categories a component does not define. Middleware authoring APIs remain available from
`@golemcloud/effect-golem/middleware`, and invocation-scoped underlying access still advances the
pinned middleware chain. The CLI accepts middleware metadata and manifest attachment; runtime
traversal and invocation behavior remain separate deployment concerns.

## Durability 1.6

`Durability.wrap` and `wrapInfallible` use
`golem:durability/durability@1.6.0` custom durable invocation resources. A live call begins with its
typed request and finishes exactly once with a typed response. `forcedCommit: true` asks the host to
commit after finish. Replay returns the recorded response without executing the body.

On typed success or typed failure, `wrap` encodes the response and calls
`finishCustomDurableInvocation`. On defect, interruption, response encoding failure, or any other
unfinished exit, scope cleanup **drops** the live invocation instead of finishing it. This is the
1.6 recovery contract: never manufacture a terminal result for work that did not complete.
`writeRemoteBatched(...)` and `writeRemoteTransaction(...)` are supported by the same wrapper; the
host resource owns their begin index and lifecycle.

Other durability controls remain under `Durability`: persistence and idempotence scopes, atomic
regions, checkpoint/revert/compensable helpers, oplog commits, and `FunctionType` constructors.
`Saga` provides fallible/infallible compensating transactions; compensations run in reverse order.

## Existing Effect integrations

- `Http`: agent HTTP mount/endpoint metadata, auth/CORS/header/query bindings, and verb helpers.
  The host serves requests. `Webhook.create` creates a host webhook and offers typed payload decode.
- `Websocket`: scoped host WebSocket client integrated with Effect streams.
- `KeyValue` and `Blobstore`: scoped WASI storage clients with schema-typed views; listing is a
  stream. Stored decode errors remain typed failures.
- `@golemcloud/effect-golem/sqlite`: `node:sqlite` adapter implementing Effect's unstable
  `SqlClient`. Native N-API drivers do not run in the WASM runtime.
- `@golemcloud/effect-golem/postgres`, `/mysql`, `/ignite2`: host-backed Effect SQL clients with
  tagged templates, transactions, streaming (for RDBMS), dialect helper values, and optional
  temporal-to-`Date` decoding. Postgres/MySQL nested transactions use savepoints; Ignite rejects
  nesting.
- `Quota`: scoped reservation/commit helpers and affine quota token split/merge.
- `Retry`, `Oplog`, `Logging`, and `Tracing`: Effect wrappers over their host APIs. Logging/tracing
  layers are applied automatically to constructors, restoration, handlers, and custom snapshots.

These APIs are covered individually, but complete cross-language 1.6 acceptance is not claimed
until the final parity matrix passes.

## Build and verify in the monorepo

The base WASM retains the single full `agent-guest` world and a shared Effect runtime.
The CLI's Rollup configuration uses `@golemcloud/effect-golem/build` to discover retained
capability modules without executing user code, then generates a static entrypoint. Agent,
tool, and middleware hooks import their implementations only when needed; absent capabilities
have explicit empty discovery and error bodies. No roles or world selection are required.
The component bundle includes only reachable SDK code and adapters, rather than importing
the full SDK from the base WASM. `capabilities.json` beside the bundle records the selection.
Selection is conservative: a retained definition can keep its capability even when its
registration is conditional or never executed.

Prerequisites are Node/npm, Rust with `wasm32-wasip2`, `wasm-rquickjs`, WASI SDK, and Golem's normal
build prerequisites. From `sdks/effect`:

```nu
npm ci
npm run lint
npm run format:check
npm run typecheck
npm test
npm run build
npm run build:bundle
$env.WASI_SDK_PATH = "/opt/wasi-sdk"
npm run build-agent-template # builds the default world
npm run check:dts
npm run check:contracts
npm run check:artifacts
```

`build-agent-template` creates/checks `agent_guest.wasm`. `check:artifacts` compares
committed/generated provenance and fails on stale artifacts.

Canonical WIT dependencies live at repository-root `wit/deps`; never edit `sdks/effect/wit/deps`
by hand. From the repository root:

```nu
cargo make wit          # mirror canonical WIT into every SDK
cd sdks/effect
npm run generate-dts    # regenerate declarations for the default world
npm run check:dts       # fail if generated declarations drift
npm run check:artifacts # fail if bundles/templates/WASM drift
```

For a focused real-runtime check, build the default template first, then run the relevant harness
case under `integration-test`. Unit tests use injectable host-service layers under `src/host`; they
do not replace a real WASM integration check.

### Durable Streams runtime fixture

The focused `durable-streams` harness case starts an in-process HTTP peer on an ephemeral local
port. Run it with a local worker executor exposing `golem:agent/durable-streams@2.0.0`; no Docker
or external stream server is needed. The peer commits the first JSON append but drops its
response, checks the retry's identical producer tuple/body/close, and acknowledges the duplicate
without an offset. The bytes writer appends and then closes on the same resource; the reader
follows an opaque checkpoint across two batches. The case verifies exact JSON/byte payloads
and eleven authenticated HTTP requests, including the cancellation check below.

`cancelRead` starts a scoped native read whose response the peer holds. A separate control read
confirms that the HTTP request arrived before the guest interrupts the fiber. Interruption must
finish within two seconds with no defect, before the guest asks the peer to release its response.
The peer bounds the hold to ten seconds. `readAfterCancel` then reads through a fresh resource in
the same agent instance, checking that delayed resource cleanup did not leave the guest unusable.

`durable-streams.golem.yaml` builds only the fixture and its native-stream sink. It provisions the
test-only secret `durableStreamToken`. Each method borrows that capability once and shares it
between append and read without revealing it. After building fresh SDK bundles and all templates,
run from `sdks/effect/integration-test` against a local Golem server:

```sh
npm ci
export GOLEM_APP_MANIFEST_PATH="$PWD/durable-streams.golem.yaml"
golem --local --yes build
golem --local --yes deploy
npm run test:integration -- --filter '^durable-streams$' --no-infra --no-server --no-build
```

To prepare the consumer without the Golem CLI, bundle only its entrypoint and inject it directly
(from `sdks/effect/integration-test`):

```sh
(cd components/agents && \
  GOLEM_APP_ROOT="$PWD/../.." GOLEM_TEMP="$PWD/../../golem-temp" \
  GOLEM_COMPONENT_NAME=effect-golem-durable-streams \
  GOLEM_COMPONENT_ENTRY=./src/durable-streams-agent.ts \
  npx --no rollup -- -c ../../rollup.config.component.mjs)
mkdir -p golem-temp/agents
wasm-rquickjs inject-js --input ../wasm/agent_guest.wasm \
  --js golem-temp/ts-dist/effect-golem-durable-streams/main.js \
  --output golem-temp/agents/effect_golem_durable_streams.dynamic.wasm
```

Expected results are `[["first", "a,b"], ["last"]]` and `[3, 249, 17]`. `forwardBytes` consumes
the external source in a separate agent through native Preview 3 stream RPC. The cancellation
and subsequent invocation return `[29]` and `[41, 203]`. The harness creates
fresh peer state and agent names on every run. This case exercises real host calls and uncertain
acknowledgements, not executor crash recovery. Restart/replay acceptance additionally needs peer
request counters and oplog inspection; returning the same values alone does not prove HTTP was
not repeated. The unit reconstruction test only checks the SDK's deterministic-call assumptions.

## Packaging and release convention

The committed package manifest follows the TypeScript SDK convention and stays at `0.0.0`; release
automation derives the publish version from a trusted root tag named
`golem-effect-v<semver>`. The root workflow (not an SDK-local `.github` workflow) performs a clean
install, all quality gates, all-world builds, declaration/artifact checks, and npm trusted
publishing with provenance.

Before publishing, `check:package` must pack the package and smoke-test the tarball from a clean
consumer directory, including every public entry point and all three WASM artifacts. A release dry
run validates the tarball without publishing. Publishing remains a separately authorized action.

## Host testing seam

Every WIT call is behind a `Context.Service` wrapper in `src/host`, assembled once by
`HostLive`. Public combinators let those requirements flow through their Effect environment; the
dispatcher provides the live layer and erases host services before user handlers are exposed.
Tests provide fake layers rather than mutating module globals. `src/host` is an internal seam, not a
supported package import.

## Compatibility

This is the direct Golem 1.6 API. There are no 1.5 source aliases, old wire fallbacks, or persisted
format compatibility promises.
