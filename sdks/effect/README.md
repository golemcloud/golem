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
`defineAgent`, `defineConfig`, and `method` are flat DSL aliases. Database adapters and standalone
middleware use the documented sub-imports. `internal/*` and `host/*` are not public imports.

## Typed clients

Keep a definition in a module without calling `.implement` when another component only needs its
client. Durable definitions expose `client.get`, `getPhantom`, and `newPhantom`; ephemeral
definitions expose `newPhantom`. Calls, triggers, and schedules take one typed input object.

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

For runtime-selected targets, use `Reflection.getAgentType(name)`,
`getAgentTypeByAgentId(id)`, or `getAllAgentTypes`. Each immutable registration exposes
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
    const increment = yield* client.method("increment")
    return yield* increment.invoke({})
  }),
)
```

Reflected invocation results contain metadata and, for non-unit outputs, `value`.
Ephemeral `newPhantom` returns a client whose actual identity arrives in invocation metadata;
durable `newPhantom` returns `{ client, agentId, phantomId }`. `invokeValue`, `triggerValue`,
and `scheduleValue` accept native WIT schema-value trees when JSON cannot represent capabilities.
`DynamicClient.fromAgentId(id)` binds without discovery and uses value-only methods; callers
must supply the correct remote contract. Both APIs use scopes and fiber interruption and expose
the same structured remote-call errors as typed clients.

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
})
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

`WitTypes.PermissionCard({ polymorphic })` represents a permission card. Cards are opaque affine
capabilities: successful encoding transfers the exact handle across agent RPC, tool calls, and
middleware. The sender must not inspect or reuse a transferred card. Transactional graph encoding
does not consume it if validation or another sibling conversion fails, so a corrected retry remains
possible. The SDK intentionally has no card inspection, wallet, derivation, or installation API.

## Tool middleware

Standalone middleware imports only the middleware-safe entry point:

```ts
import { universal } from "@golemcloud/effect-golem/middleware"

universal({
  name: "audit",
  handler: (invocation, underlying) =>
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin),
})
```

`universal` transparently handles any tool using wire values. `typed({ presented, expected?,
handler })` projects typed input/output/error and a definition-derived `context.underlying` client;
it can present a different definition from the wrapped tool. Both support aliases, docs, and a
per-invocation Effect `layer`. Underlying access and streams are affine, sequential, and valid only
for that invocation.

There are three build worlds:

- `agent-guest`: agents plus tool guests (`@golemcloud/effect-golem`)
- `tool-middleware-guest`: standalone middleware (`@golemcloud/effect-golem/middleware`)
- `agent-tool-middleware-guest`: combined agent/tool/middleware component

The SDK and templates support all three worlds. The CLI accepts middleware metadata and manifest
attachment; runtime traversal and invocation behavior remain separate deployment concerns.

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
npm run build-agent-template # builds all three worlds
npm run check:dts
npm run check:contracts
npm run check:artifacts
```

`build-agent-template` creates/checks `agent_guest.wasm`, `tool_middleware_guest.wasm`, and
`agent_tool_middleware_guest.wasm`. `check:artifacts` compares committed/generated provenance and
fails on stale world artifacts.

Canonical WIT dependencies live at repository-root `wit/deps`; never edit `sdks/effect/wit/deps`
by hand. From the repository root:

```nu
cargo make wit          # mirror canonical WIT into every SDK
cd sdks/effect
npm run generate-dts    # regenerate declarations for all three worlds
npm run check:dts       # fail if generated declarations drift
npm run check:artifacts # fail if bundles/templates/WASM drift
```

For a focused real-runtime check, build all three templates first, then run the relevant harness
case under `integration-test`. Unit tests use injectable host-service layers under `src/host`; they
do not replace a real WASM integration check.

### Durable Streams runtime fixture

The focused `durable-streams` harness case starts an in-process HTTP peer on an ephemeral local
port. Run it with a local worker executor exposing `golem:agent/durable-streams@2.0.0`; no Docker
or external stream server is needed. The peer commits the first JSON append but drops its
response, checks the retry's identical producer tuple/body/close, and acknowledges the duplicate
without an offset. It also verifies exact JSON/byte payloads and authenticated request counts.

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
  npx --no rollup -- -c ../../rollup.config.component.mjs --input ./src/durable-streams-agent.ts)
mkdir -p golem-temp/agents
wasm-rquickjs inject-js --input ../wasm/agent_guest.wasm \
  --js golem-temp/ts-dist/effect-golem-durable-streams/main.js \
  --output golem-temp/agents/effect_golem_durable_streams.dynamic.wasm
```

Expected results are `[["first", "a,b"], ["last"]]` and `[3, 249, 17]`. `forwardBytes` consumes
the external source in a separate agent through native Preview 3 stream RPC. The harness creates
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
