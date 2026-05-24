# effect-golem

Write [Golem](https://golem.cloud) agents on top of [Effect v4](https://effect.website/) — declarative, schema-typed, end-to-end durable.

`effect-golem` is a TypeScript SDK that lets you author Golem agents the same way you would author any other Effect application: `Effect.gen`, `Schema`, `Layer`, `Scope`, `Stream`. The library compiles your agent module into a Golem WASM component (by injecting your bundled JS into a prebuilt QuickJS-backed base WASM) and wires every Golem host capability — durable execution, oplog, snapshots, sagas, RPC, HTTP, webhooks, websockets, KV/Blob, RDBMS, quotas, retry policies, logging, tracing — into idiomatic Effect surface.

```ts
import { Effect, Ref, Schema } from "effect"
import { defineAgent, Http, method, Snapshot } from "effect-golem"

export const Counter = defineAgent({
  name: "Counter",
  mode: "durable",
  constructorParams: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({ params: {}, success: Schema.Number, http: [Http.get("/value")] }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
      http: [Http.post("/add"), Http.get("/add?by={by}")],
    }),
  },
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    const state = yield* snap.init({ count: 0 })
    yield* Effect.logInfo("Counter constructed").pipe(Effect.annotateLogs({ name }))
    return {
      value: () => Ref.get(state).pipe(Effect.map((s) => s.count)),
      add: ({ by }) =>
        Ref.updateAndGet(state, (s) => ({ count: s.count + by })).pipe(Effect.map((s) => s.count)),
    }
  }),
)
```

## Table of contents

- [Installation](#installation)
- [Project layout](#project-layout)
- [Agents, methods, and config](#agents-methods-and-config)
  - [defineAgent](#defineagent)
  - [method](#method)
  - [defineConfig](#defineconfig)
- [Multimodal & unstructured inputs](#multimodal--unstructured-inputs)
- [Schema ↔ WIT interop](#schema--wit-interop)
- [Typed RPC clients](#typed-rpc-clients)
- [HTTP routing](#http-routing)
- [Principals](#principals)
- [Snapshotting](#snapshotting)
- [Durable functions](#durable-functions)
- [Sagas / multi-step transactions](#sagas--multi-step-transactions)
- [Webhooks](#webhooks)
- [WebSockets](#websockets)
- [KeyValue store](#keyvalue-store)
- [Blob store](#blob-store)
- [RDBMS adapters: Postgres / MySQL / Ignite](#rdbms-adapters-postgres--mysql--ignite)
- [SQLite (`node:sqlite`)](#sqlite-nodesqlite)
- [Quotas](#quotas)
- [Retry policies](#retry-policies)
- [Oplog](#oplog)
- [Self-agent metadata, fork, promises](#self-agent-metadata-fork-promises)
- [Logging & tracing](#logging--tracing)
- [Building & deploying](#building--deploying)
- [Why Effect?](#why-effect)
- [License](#license)

---

## Installation

```bash
npm install effect-golem effect
```

`effect-golem` ships its own bundled `effect` re-export so every component embedded in the base WASM shares one Effect runtime instance. The dependency on `effect` in your `package.json` is for type checking and IDE support.

The four database adapters live behind sub-imports so unused drivers do not bloat your bundle:

```bash
# pick the ones you need:
import { SqliteClient } from "effect-golem/sqlite"
import { PgClient }     from "effect-golem/postgres"
import { MySqlClient }  from "effect-golem/mysql"
import { IgniteClient } from "effect-golem/ignite2"
```

> **Important:** Inside Golem your component runs in `wasm-rquickjs`. Native N-API drivers (`pg`, `mysql2`, `better-sqlite3`, …) **cannot** load there. Always use the `effect-golem/*` adapters; they delegate the wire protocol to the Golem host and expose the canonical `effect/unstable/sql.SqlClient` interface so `SqlSchema` / `SqlResolver` / `Migrator` / tagged-template queries all just work.

## Project layout

A typical `effect-golem` component looks like this:

```
my-agent/
├── src/
│   ├── main.ts            ← imports each implementation module to register it
│   ├── counter-agent.ts   ← defineAgent(...).implement(...)
│   └── caller-agent.ts    ← defineAgent(...).implement(...)
├── tsconfig.json
└── package.json           ← depends on `effect-golem` and `effect`
```

Each top-level `defineAgent({...}).implement(...)` call registers the agent with the dispatcher; `main.ts` only needs to side-effect-import each implementation file. Components can host one agent or many.

## Agents, methods, and config

### defineAgent

```ts
import { Effect } from "effect"
import { defineAgent, method, Schema } from "effect-golem"

defineAgent({
  name: "Greeter", // unique within this component
  description: "Says hi", // surfaced in agent metadata
  mode: "durable" | "ephemeral", // mirrors the WIT mode field
  constructorParams: { name: Schema.String },
  methods: {
    /* ... */
  },
}).implement(({ name }) =>
  Effect.succeed({
    /* method handlers */
  }),
)
```

`.implement(...)` takes a function that returns an `Effect` producing an object whose keys match `methods`. Each handler is itself an Effect. Inside the implementation you can `yield*` any Effect service tag — the dispatcher provides the full host-binding bundle (config, principal, oplog, durability, …) plus the auto-snapshot binder.

### method

```ts
method({
  params: { id: Schema.String, count: Schema.Number },
  success: Schema.Number,
  error: NotFoundError, // optional typed E
  description: "Look up a record",
  promptHint: "Read by id", // surfaced to AI orchestrators
  http: [Http.get("/lookup/{id}?count={count}")],
})
```

Both the success and the optional `error` channel are encoded over the wire as a component-model `result<S, E>`, so typed failures cross RPC boundaries cleanly. Methods declared without `error:` default to `Schema.Never` (no typed E) and any thrown failure becomes a generic `RemoteCallError`.

```ts
// Typed-error round-trip across RPC:
fetch: method({
  params: { id: Schema.String },
  success: Schema.Number,
  error: Schema.Struct({ _tag: Schema.Literal("NotFoundError"), resource: Schema.String }),
})

// In the caller:
const lookup = yield * Lookup.client.get({ realm: "demo" })
yield *
  lookup
    .fetch({ id: "missing" })
    .pipe(Effect.catchTag("NotFoundError", (e) => Effect.succeed(`missing: ${e.resource}`)))
```

### defineConfig

`defineConfig` declares typed, host-managed configuration. Backed by `golem:agent/host.get-config-value` and the WasmRpc 4th `agent-config` argument:

```ts
import { defineAgent, defineConfig, method } from "effect-golem"
import { Effect, Redacted, Schema } from "effect"

export class CounterConfig extends defineConfig("Counter.Config", {
  greeting: Schema.String,
  apiKey: Schema.Redacted(Schema.String), // → secret leaf
  database: Schema.Struct({
    // → nested struct
    host: Schema.String,
    port: Schema.Number,
  }),
}) {}

defineAgent({
  name: "Counter",
  config: CounterConfig,
  // ...
}).implement(({ name }) =>
  Effect.gen(function* () {
    const cfg = yield* CounterConfig
    const greeting = yield* cfg.greeting // Effect<string, ConfigError>
    const apiKey = yield* cfg.apiKey.get // Effect<Redacted<string>, ConfigError>
    const dbHost = yield* cfg.database.host // recurses into nested struct

    return {
      greet: () => Effect.succeed(`${greeting}, ${name}`),
    }
  }),
)
```

- Plain leaves are memoised per host invocation.
- `Schema.Redacted` leaves are **never** cached — every read issues a fresh host call (so the host can rotate secrets between invocations).
- `Schema.Option(inner)` cleanly maps to "soft default" keys that may be missing.
- RPC callers can override non-secret leaves via `Counter.client.get(params, { overrides: { greeting: "..." } })`.

## Multimodal & unstructured inputs

`effect-golem` supports the full Wasm Component Model agent surface, including unstructured text/binary references and multimodal payloads (mixed text + binary parts, à la chat messages with attachments). `Multimodal`, `Unstructured`, and `Element` namespaces compose with the rest of the schema DSL.

```ts
import { Multimodal, Unstructured } from "effect-golem"

const ChatTurn = Multimodal.multimodal({
  text: Unstructured.UnstructuredText({ languageCode: "en" }),
  image: Unstructured.UnstructuredBinary({ mimeType: "image/png" }),
  metadata: Schema.Struct({ author: Schema.String }),
})

defineAgent({
  // ...
  methods: {
    ingest: method({ params: { turn: ChatTurn }, success: Schema.Void }),
  },
})
```

The `Unstructured` namespace also exposes the underlying wire-shape schemas as ordinary composable `Schema` values: `Unstructured.TextReference` and `Unstructured.BinaryReference` are tagged unions (`url` vs `inline`), with `TextSource` / `TextType` / `BinarySource` / `BinaryType` for nested record shapes. These compose anywhere in your own structured types when you need to embed an unstructured reference inside a regular record (rather than as a top-level multimodal slot).

For low-level integration, `Element.componentModelElement(witCodec)` lifts a `WitCodec<S>` into the `ElementCodec<S["Type"]>` that wraps a `WitValue` in `{ tag: "component-model", val: ... }` — this is what every ordinary `Schema.Top` collapses to inside a multimodal slot.

Multimodal / unstructured params are statically excluded from path / query / header bindings (HTTP routing only binds string-decodable params).

## Schema ↔ WIT interop

JavaScript only has `number` and `bigint`, but the WIT type system (and therefore the WasmRpc / `wit-value` wire format) distinguishes `u8` / `u16` / `u32` / `u64` / `s8` / … / `f32` / `f64` / `char`. The `WitTypes` namespace exposes annotated `Schema` values that pin a JS-side type to a specific WIT primitive so the SDK lowers it correctly:

```ts
import { Schema } from "effect"
import { WitTypes } from "effect-golem"

const Pixel = Schema.Struct({
  r: WitTypes.Uint8, // → WIT `u8`, range-checked
  g: WitTypes.Uint8,
  b: WitTypes.Uint8,
})

const Frame = Schema.Struct({
  index: WitTypes.Int32, // → WIT `s32`
  ts: WitTypes.Uint64, // → WIT `u64` (bigint-backed)
  pixels: WitTypes.Uint8ArraySchema, // → WIT `list<u8>` carried as Uint8Array
})
```

`WitTypes` also ships fixed-width typed-array schemas for every WIT primitive list (`Uint8ArraySchema`, `Int8ArraySchema`, `Uint16ArraySchema`, …, `Float32ArraySchema`, `BigUint64ArraySchema`), `WitTypes.Char` for the WIT `char` (single Unicode scalar) type, and `WitTypes.withVariantCaseName(name)` for overriding the wire-side case name on a `Schema.Union` arm when it must differ from its `_tag` value.

For lower-level work, `WitCodec.toWitCodec(schema)` returns the underlying `WitCodec<S>` — the value-level record carrying the `WitValue` encoder/decoder, the `WitType` lowering, and the agent-visible `ElementSchema`. This is the same machinery the SDK uses internally when it compiles `params` / `success` / `error` schemas. `WitCodec.UnsupportedSchemaError` is the typed failure raised when a schema cannot be lowered (e.g. `Schema.Any`, recursive schemas without a wrapper, or a multimodal in a position that doesn't accept one).

## Typed RPC clients

Every `defineAgent` call attaches a typed `client` namespace whose proxy methods come in three call shapes:

```ts
import { Effect, Fiber } from "effect"

// 1) Function-call shape — durable, fully interruptible.
const counter = yield * Counter.client.get({ name: "alice" })
const value = yield * counter.value({})

// 2) Fork + interrupt: the SDK propagates fiber-interrupt to
//    `future-invoke-result.cancel()` on the host.
const fiber = yield * Effect.forkChild(counter.slowValue({ seconds: 60 }))
yield * Effect.sleep("100 millis")
yield * Fiber.interrupt(fiber) // → host receives cancel

// 3) Fire-and-forget:
yield * counter.add.trigger({ by: 1 })

// 4) Scheduled (host returns a cancellation handle):
const sched = yield * counter.add.schedule(Date.now() + 5_000, { by: 1 })
yield * sched.cancel
```

Composes naturally with `Effect.race`, `Effect.timeout`, `Effect.retry`, etc.

## HTTP routing

`effect-golem` exposes routing metadata; the Golem host owns HTTP serving, decoding, auth and CORS. Authoring is declarative through the `Http` namespace:

```ts
import { defineAgent, Http, method, Schema } from "effect-golem"

defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"], auth: true }),
  methods: {
    value: method({ params: {}, success: Schema.Number, http: [Http.get("/value")] }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
      http: [
        Http.post("/add"), // by ← JSON body
        Http.get("/add?by={by}"), // by ← query
      ],
    }),
    upload: method({
      params: { id: Schema.String, payload: Schema.String },
      success: Schema.Void,
      http: [Http.put("/items/{id}", { headers: { "X-Upload-Token": "payload" } })],
    }),
  },
  // ...
})
```

Path syntax: `{var}` (path variable), `{*rest}` (catch-all, last-segment only), `{agent-type}` / `{agent-version}` (host-injected), `?key={var}&...` (inline query bindings on endpoints). Most invariants are enforced **at compile time** through type-level templates, so misconfigured agents fail to typecheck with the offending name baked into the diagnostic. The same checks also run at runtime for non-literal paths and `as any` callers.

Verbs: `Http.get` / `post` / `put` / `del` / `patch` / `head` / `options` / `trace` / `connect` are the standard shorthands. For custom or generic verb dispatch use `Http.custom("VERB", path, opts?)` or the verb-generic `Http.endpoint(verb, path, opts?)`:

```ts
http: [
  Http.custom("LINK", "/items/{id}/relations"),
  Http.endpoint("PURGE", "/cache"),
],
```

Header bindings on endpoint options bind a method param to an HTTP header (case-insensitive):

```ts
upload: method({
  params: { id: Schema.String, token: Schema.String },
  success: Schema.Void,
  http: [Http.put("/items/{id}", { headers: { "X-Upload-Token": "token" } })],
})
```

Mount and endpoint options also accept `auth?: boolean`, `cors?: string[]`, and the mount accepts a `webhookSuffix` string used by [Webhooks](#webhooks). Builders (`Http.withAuth`, `Http.withCors`, `Http.withHeader` / `withHeaders`, `Http.withWebhookSuffix`) compose the same options pipeline-style.

## Principals

When the `auth: true` option is set on a mount or endpoint, the host authenticates the caller and surfaces them as a `Principal` value. Both the principal that issued the current invocation and the principal that originally created the agent are accessible via `Context.Service` tags:

```ts
import { Effect } from "effect"
import { defineAgent, Principal, SelfAgentId } from "effect-golem"

defineAgent({
  // ...
}).implement(({ name }) =>
  Effect.gen(function* () {
    const owner = yield* Principal.Principal // who created me
    const self = yield* SelfAgentId.SelfAgentId // my own AgentId

    return {
      caller: () =>
        Effect.gen(function* () {
          const p = yield* Principal.Principal // who's calling me right now
          return p.tag === "oidc" ? `oidc:${p.val.sub}` : p.tag
        }),
    }
  }),
)
```

## Snapshotting

Opt-in via the `snapshot:` field. When set, the agent's WIT `snapshotting` metadata flips to `enabled(...)` and the SDK wires `golem:api/save-snapshot.save` / `load-snapshot.load`.

### Auto (schema-driven)

```ts
import { defineAgent, method, Schema, Snapshot } from "effect-golem"
import { Effect, Ref } from "effect"

defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10), // also: default / periodic("5 minutes")
  }),
  // ...
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    const state = yield* snap.init({ count: 0 })
    return {
      /* handlers reading/writing `state` */
    }
  }),
)
```

### Custom (user-managed)

```ts
defineAgent({
  // ...
  snapshot: Snapshot.custom({ policy: Snapshot.policy.periodic("1 minute") }),
}).implement((_p, snap) =>
  Effect.gen(function* () {
    const state = yield* Ref.make({ ... })
    snap.register({
      save: () => Ref.get(state).pipe(Effect.map(serialise)),
      load: (bytes) => Ref.set(state, deserialise(bytes)),
    })
    return { /* ... */ }
  }),
)
```

### SQLite databases inside snapshots

Auto-snapshot agents can attach one or more `node:sqlite` `DatabaseSync` handles. The envelope becomes `multipart/mixed` carrying the JSON state plus one `db:<name>` part per declared database (raw SQLite bytes).

```ts
defineAgent({
  // ...
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(10),
  }),
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    yield* snap.init({})
    const sql = yield* SqliteClient.make({ filename: ":memory:" })
    yield* sql.exec(`CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER)`)
    yield* sql`INSERT OR IGNORE INTO counters (id, count) VALUES (${name}, 0)`
    yield* snap.attachDatabase("counters", sql)
    return {
      /* ... */
    }
  }),
)
```

`load` replaces `initialize` on restore: the implementation runs first (DDL must be idempotent), then the SDK overwrites each in-memory database with `restoreDatabaseSync(handle, bytes)`, then the auto state Ref is restored from JSON.

Strict constraints (enforced at save and load time, surfaced as typed errors):

- Every declared database name must be attached **exactly once** (`SnapshotDatabaseDuplicateAttachError` / `SnapshotDatabaseMissingPartError`).
- At save time every attached DB must be in **autocommit mode** (`SnapshotDatabaseNotInAutocommitError`) and must not have ATTACHed schemas beyond `main` / `temp` (`SnapshotDatabaseHasAttachmentsError`).
- At load time the multipart envelope's `db:<name>` parts must match the declared set exactly (`SnapshotDatabaseUnknownPartError` / `SnapshotDatabaseMissingPartError`).
- Database names must match `/^[a-zA-Z_][a-zA-Z0-9_]*$/`.
- `Snapshot.custom(...)` agents do **not** support `databases` (custom is fully user-managed).

## Durable functions

`Durability.wrap` records the result of a side-effecting block to the oplog so it replays deterministically.

```ts
import { Durability, defineAgent, method, Schema } from "effect-golem"
import { Effect } from "effect"

defineAgent({
  // ...
  methods: {
    fetchQuote: method({
      params: { symbol: Schema.String },
      success: Schema.Struct({ symbol: Schema.String, price: Schema.Number }),
    }),
  },
}).implement(() =>
  Effect.succeed({
    fetchQuote: ({ symbol }) =>
      Durability.wrap(
        {
          iface: "myapp",
          function: "fetchQuote",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Schema.Struct({ symbol: Schema.String }),
          success: Schema.Struct({ symbol: Schema.String, price: Schema.Number }),
          // optional:
          // error: SomeErrorSchema,
        },
        { symbol },
        Effect.sync(() => fetchFromRemote(symbol)), // body — runs once, replays from oplog
      ),
  }),
)
```

Lower-level escape hatches are also exposed:

```ts
// Atomic region (single BeginAtomicRegion / EndAtomicRegion pair):
Durability.atomically(effect)

// Persistence level (`persistNothing`, `persistRemoteSideEffects`, `smart`):
Durability.withPersistenceLevel(Durability.PersistenceLevel.persistNothing, effect)
Durability.getPersistenceLevel // Effect<PersistenceLevelValue>
Durability.setPersistenceLevel(level) // imperative

// Idempotence mode (host treats every replay as the same logical call):
Durability.withIdempotenceMode(true, effect)
Durability.getIdempotenceMode / Durability.setIdempotenceMode

// Replay introspection:
Durability.isLive // Effect<boolean> — false during replay
Durability.currentDurableExecutionState // { isLive, persistenceLevel }
Durability.observeFunctionCall(iface, fn) // recorded but no value persisted

// Idempotency key generation (deterministic across replay):
Durability.generateIdempotencyKey // Effect<UUID>

// Force the host to flush the oplog up to the current index:
Durability.oplogCommit(level)
```

For multi-step durable calls (batched writes, multi-RPC transactions) where `wrap` would over-constrain the lifecycle, drop down to the manual `begin → persist → end` triplet. This is what the host's `FunctionType.writeRemoteBatched(begin?)` and `writeRemoteTransaction(begin?)` variants are designed for:

```ts
yield *
  Effect.gen(function* () {
    const begin = yield* Durability.beginDurableFunction(
      Durability.FunctionType.writeRemoteBatched(),
    )
    // …multiple host-side writes / RPC calls here…
    if (yield* Durability.isLive) {
      yield* Durability.persistDurableFunctionInvocation(
        "myapp::batchedWrite",
        requestVT,
        responseVT,
        Durability.FunctionType.writeRemoteBatched(begin),
      )
    } else {
      yield* Durability.readPersistedDurableFunctionInvocation
    }
    yield* Durability.endDurableFunction(begin)
  })
```

## Sagas / multi-step transactions

The `Saga` namespace ships an Effect-idiomatic saga combinator on top of the Golem oplog primitives. API mirrors `@effect/workflow.Workflow.withCompensation` shape.

```ts
import { Saga } from "effect-golem"
import { Effect } from "effect"

// Reusable execute+compensate pair:
const bookFlight = Saga.operation({
  execute: ({ flightId }: { flightId: string }) => Effect.succeed({ ref: `FLT-${flightId}` }),
  compensate: ({ flightId }, booking, _cause) => cancelFlight(booking.ref).pipe(Effect.ignore),
})

// Fallible: failure becomes a typed `Saga.TransactionFailure<E>` after rollback.
const result =
  yield *
  Saga.fallibleTransaction(
    Effect.gen(function* () {
      const flight = yield* bookFlight({ flightId: "AA1" })
      const hotel = yield* bookHotel({ hotelId: "H7" })
      return { flight, hotel }
    }),
  )
// → Saga.TransactionFailure can be `FailedAndRolledBackCompletely`
//   or `FailedAndRolledBackPartially` (with the compensation error).

// Infallible: drains compensations + setOplogIndex(checkpoint) + Effect.never.
//             The host preempts and replays from the checkpoint.
yield * Saga.infallibleTransaction(body) // body must be Effect<A, never, R>

// Building blocks:
Saga.withCompensation(eff, (value, cause) => Effect<void, never, R>)
Saga.withFallibleCompensation(eff, (value, cause) => Effect<void, E, R>)
```

Each step runs in its own atomic region, so the oplog shows balanced `BeginAtomicRegion` / `EndAtomicRegion` markers per step plus a `Jump` entry on infallible retry.

## Webhooks

The `Webhook` namespace bundles `Promises.create` + `golem:agent/host.create-webhook` and exposes a handle whose `await` durably suspends until the URL is POSTed to.

> **Prerequisite.** `Webhook.create` is only legal from an agent that is **currently deployed via an HTTP API**. Concretely the agent must declare `Http.mount(..., { webhookSuffix: "..." })` AND be listed under `httpApi.deployments.<env>.agents` in your `golem.yaml`. Calling `Webhook.create` from an agent without an active HTTP deployment traps with `WebhookHostError`.

```ts
import { Webhook, defineAgent, Http, method, Schema } from "effect-golem"

defineAgent({
  name: "PaymentWatcher",
  constructorParams: { name: Schema.String },
  http: Http.mount("/watchers/{name}", { webhookSuffix: "/payments" }),
  methods: {
    waitForPayment: method({
      params: {},
      success: Schema.Struct({ id: Schema.String, status: Schema.String }),
    }),
  },
}).implement(() =>
  Effect.succeed({
    waitForPayment: () =>
      Effect.gen(function* () {
        const hook = yield* Webhook.create
        // share `hook.url` with the payment provider...
        const payload = yield* hook.await
        return yield* payload.decode(PaymentEvent)
      }),
  }),
)
```

The `WebhookHandle` also exposes a non-blocking `poll`:

```ts
const ready = yield * hook.poll // Effect<WebhookPayload | undefined>
if (ready) yield * ready.decode(PaymentEvent)
```

## WebSockets

`Websocket.connect` returns the canonical `effect/unstable/socket.Socket`, so the same `runString` / writer / `Effect.scoped` patterns from a regular Effect application work against the host's `golem:websocket/client@1.5.0` binding.

```ts
import { Websocket } from "effect-golem"

yield *
  Effect.scoped(
    Effect.gen(function* () {
      const sock = yield* Websocket.connect("wss://echo.example.com")
      const write = yield* sock.writer
      yield* write("hello")
      yield* sock.runString((line) => console.log("got:", line))
    }),
  )
```

`Websocket.connect` accepts a `ConnectOptions` for customising headers, subprotocols, and the close-code policy:

```ts
yield *
  Websocket.connect("wss://api.example.com/v1/stream", {
    headers: { Authorization: `Bearer ${token}` },
    subprotocols: ["v1.json"],
    closeCodeIsError: (code) => code !== 1000 && code !== 1001,
  })
```

For deeper integration, `Websocket.makeChannel(...)` returns the canonical Effect `Channel` (consumable as a duplex `Stream`), and `Websocket.fromConnection(handle, opts)` adapts an already-acquired host connection resource into a `Socket`.

## KeyValue store

Wraps the eventually-consistent subset of `wasi:keyvalue@0.1.0` (`eventual` + `eventual-batch`).

```ts
import { KeyValue, defineAgent, method, Schema } from "effect-golem"

const User = Schema.Struct({ id: Schema.String, name: Schema.String })

defineAgent({
  name: "Users",
  constructorParams: { name: Schema.String },
  methods: {
    put: method({ params: { id: Schema.String, name: Schema.String }, success: Schema.Void }),
    get: method({ params: { id: Schema.String }, success: Schema.Option(User) }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    const bucket = yield* KeyValue.openBucket(name)
    const users = bucket.forSchema(User) // typed view
    return {
      put: ({ id, name }) => users.set(id, { id, name }),
      get: ({ id }) => users.get(id),
      // batch ops, raw bytes, etc. also available
    }
  }),
)
```

## Blob store

Wraps `wasi:blobstore/{blobstore,container,types}` — container CRUD, object I/O (writes chunked to 4 KB segments), object listing as a `Stream`, plus a `forSchema` typed view.

```ts
import { Blobstore } from "effect-golem"
import { Stream } from "effect"

const photos = yield * Blobstore.getOrCreateContainer("photos")
yield * photos.writeData("a.png", bytes)
const back = yield * photos.getData("a.png")
const list = yield * Stream.runCollect(photos.listObjects)

// typed view:
const meta = photos.forSchema(Schema.Struct({ filename: Schema.String, size: Schema.Number }))
yield * meta.writeData("a.json", { filename: "a.png", size: bytes.length })
```

## RDBMS adapters: Postgres / MySQL / Ignite

All three adapters expose the **official `effect/unstable/sql.SqlClient`** interface, so tagged-template queries, `withTransaction`, `executeStream`, `SqlSchema` / `SqlResolver` / `Migrator` all compose unchanged.

```ts
import { defineAgent, defineConfig, method, Schema, Snapshot } from "effect-golem"
import { PgClient } from "effect-golem/postgres"
import { Effect, Redacted } from "effect"

export class PgCounterConfig extends defineConfig("PgCounter.Config", {
  connectionAddress: Schema.Redacted(Schema.String),
}) {}

defineAgent({
  name: "PgCounter",
  config: PgCounterConfig,
  constructorParams: { name: Schema.String },
  // External DB state is NOT captured in Golem snapshots; declare an
  // empty schema purely to drive the snapshot oplog cadence:
  snapshot: Snapshot.define({ schema: Schema.Struct({}), policy: Snapshot.policy.everyN(10) }),
  methods: {
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
    transferAdd: method({
      params: { from: Schema.String, by: Schema.Number },
      success: Schema.Number,
    }),
  },
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    yield* snap.init({})
    const cfg = yield* PgCounterConfig
    const dsn = Redacted.value(yield* cfg.connectionAddress.get)
    const sql = yield* PgClient.make({ connectionAddress: dsn })

    yield* sql`CREATE TABLE IF NOT EXISTS counters (id text PRIMARY KEY, count int NOT NULL DEFAULT 0)`
    yield* sql`INSERT INTO counters (id, count) VALUES (${name}, 0) ON CONFLICT (id) DO NOTHING`

    return {
      add: ({ by }) =>
        sql`UPDATE counters SET count = count + ${by} WHERE id = ${name} RETURNING count`.pipe(
          Effect.map((rows) => Number((rows[0] as { count: number }).count)),
        ),
      transferAdd: ({ from, by }) =>
        sql.withTransaction(
          Effect.gen(function* () {
            yield* sql`UPDATE counters SET count = count - ${by} WHERE id = ${from}`
            yield* sql`UPDATE counters SET count = count + ${by} WHERE id = ${name}`
            return yield* sql`SELECT count FROM counters WHERE id = ${name}`.pipe(
              Effect.map((rows) => Number((rows[0] as { count: number }).count)),
            )
          }),
        ),
    }
  }),
)
```

Each adapter has a dialect helper namespace (`Pg.jsonb(...)`, `Pg.numeric(...)`, `Pg.uuid(...)`, `MySql.json(...)`, `Ignite.timestamp(...)`, …) for the rich types that don't fit into JS primitives. Decoded temporal columns default to the host's raw struct (no precision loss); set `decodeTemporal: "date"` on the client config to opt into JS `Date` decoding.

> **State outside the worker.** RDBMS state lives in the database, _outside_ the Golem worker, and is **not** captured in Golem snapshots. Treat the database as the durable source of truth and write idempotent DDL (`CREATE TABLE IF NOT EXISTS`, `INSERT ... ON CONFLICT DO NOTHING`).

## SQLite (`node:sqlite`)

The `effect-golem/sqlite` adapter wraps Node's built-in `node:sqlite` (the only SQLite available inside `wasm-rquickjs`) and implements the same `SqlClient` interface as the RDBMS adapters. Adds `export: Effect<Uint8Array, SqlError>` and an `exec(sql)` helper for parameter-less DDL.

```ts
import { SqliteClient } from "effect-golem/sqlite"

const sql = yield * SqliteClient.make({ filename: ":memory:" })
yield * sql.exec(`CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER)`)
yield * sql`INSERT OR IGNORE INTO counters (id, count) VALUES (${name}, 0)`
const rows = yield * sql`SELECT count FROM counters WHERE id = ${name}`
```

Pair with `Snapshot.define({ databases: [...] })` (above) to capture the on-disk image into the snapshot envelope.

## Quotas

Effect-typed wrapper over `golem:quota/types@1.5.0` for resource reservations.

```ts
import { Quota } from "effect-golem"

const token = yield * Quota.acquireQuotaToken("api-calls", 1n)

// RAII — body returns { used, value }; on success commits `used`,
// on failure / interrupt the surrounding scope commits 0 automatically:
const response =
  yield *
  Quota.withReservation(token, 4000n, () =>
    Effect.gen(function* () {
      const r = yield* callLlm(prompt, { maxTokens: 4000 })
      return { used: BigInt(r.tokensUsed), value: r }
    }),
  )

// Manual reserve + commit (run inside Effect.scoped so drop ≡ commit(0)):
yield *
  Effect.scoped(
    Effect.gen(function* () {
      const reservation = yield* Quota.reserve(token, 100n)
      yield* doWork()
      yield* Quota.commit(reservation, 73n)
    }),
  )

// Split tokens across RPC peers, merge them back:
const child = yield * Quota.split(token, 300n)
// ...send `child` over RPC (auto-encoded via the QuotaToken schema codec)...
yield * Quota.merge(token, child)
```

`reject`-policy reservations surface as a typed `FailedReservationError` carrying `estimatedWaitNanos`; `throttle` / `terminate` policies are handled inside the host before `reserve` returns.

For sending tokens across an RPC boundary as part of your own typed method schemas, embed the exported codecs directly:

```ts
import { Quota } from "effect-golem"

const TransferQuotaInput = Schema.Struct({
  worker: Schema.String,
  token: Quota.QuotaToken, // schema codec; round-trips host handle ↔ wire record
})

// Lower-level: the plain wire record (no resource handle, just the fields):
const Audit = Schema.Struct({ snapshot: Quota.QuotaTokenRecord })
```

`Quota.QuotaToken` is a `Schema.Codec` that round-trips the live host resource handle through its `QuotaTokenRecord` representation; `Quota.QuotaTokenRecord` is the bare wire shape (`{ resourceName, expectedUse, ... }`) for cases where you want to inspect or persist the data without binding it back to a resource handle.

## Retry policies

`golem:api/retry@1.5.0` exposed as a fluent DSL:

```ts
import { Retry } from "effect-golem"
import { Duration, Effect } from "effect"

const policy = Retry.NamedPolicy.named(
  "http-transient",
  Retry.Policy.exponential(Duration.seconds(1), 2)
    .maxRetries(5)
    .withJitter(0.1)
    .onlyWhen(
      Retry.Predicate.gte(Retry.Props.statusCode, 500).and(
        Retry.Predicate.neq(Retry.Props.statusCode, 501),
      ),
    ),
).priority(10)

// Imperative, persisted to oplog:
yield * Retry.setPolicy(policy)

// Scoped: only active for the wrapped Effect's lifetime:
yield * Retry.withPolicy(policy, doWork)
```

The same policy can also be **converted to an Effect `Schedule`** via `Retry.toSchedule(policy)`. The result composes with `Effect.retry` / `Effect.repeat` and any other `Schedule.*` combinator — so a policy declared once can drive both host-side, oplog-persisted retries (`Retry.withPolicy`) and Effect-side, in-process retries (`Effect.retry`):

```ts
import { Effect } from "effect"
import { Retry } from "effect-golem"

const schedule = Retry.toSchedule(policy, {
  // Project the failure into the property bag the predicate evaluates
  // against — only needed when the policy uses `onlyWhen(...)`.
  properties: (err: HttpError) => ({ statusCode: err.status }),
})

yield * doWork.pipe(Effect.retry(schedule))
```

Unlike `withPolicy`, the schedule runs entirely on the Effect side — the Golem host does NOT see retries as `RetryAttempt` oplog entries. Use `withPolicy` for host-driven retries that participate in the durability oplog (e.g. cross-`SUSPEND` survival); use `toSchedule` for fine-grained Effect-native retry behaviour.

## Oplog

Read and search the running agent's own oplog as a `Stream`:

```ts
import { Oplog, SelfAgentId } from "effect-golem"
import { Stream } from "effect"

const idx = yield * Oplog.currentIndex
const self = yield * SelfAgentId.SelfAgentId

const tags =
  yield *
  Stream.runCollect(
    Oplog.read({ agentId: self, start: 0n }).pipe(
      Stream.map((entry) => entry.tag),
      Stream.take(50),
    ),
  )

// Full-text search:
yield * Stream.runCollect(Oplog.search({ agentId: self, text: "increment" }))

// Force a checkpoint commit:
yield * Oplog.commit
```

## Self-agent metadata, fork, promises

```ts
import { Agents, SelfAgentId } from "effect-golem"
import { Stream } from "effect"

// --- Self / metadata ---
const meta = yield* Agents.getSelfMetadata
// → { agentId, componentRevision, status, retryCount, ... }

const otherMeta = yield* Agents.getAgentMetadata(otherAgentId)

// --- Resolution helpers ---
const componentId = yield* Agents.resolveComponentId({ componentName: "my-app" })
const agentId     = yield* Agents.resolveAgentId({ componentName: "my-app", agentName: "alice" })
const strict      = yield* Agents.resolveAgentIdStrict({ ... })  // fails if not found

// --- Fork the running agent into a sibling worker ---
const fork = yield* Agents.fork
// → { tag: "original" | "forked" }

// --- Targeted fork / update / revert ---
yield* Agents.forkAgent({ source: agentId, target: { agentName: "alice-shadow" } })
yield* Agents.updateAgent({ agentId, targetVersion: 7n, mode: "automatic" })
yield* Agents.revertAgent(agentId, Agents.RevertTarget.toOplogIndex(42n))

// --- Paged listing with the Filter DSL ---
const filter = Agents.Filter.and([
  Agents.Filter.componentName("my-app"),
  Agents.Filter.statusEq("running"),
])
const all = yield* Stream.runCollect(Agents.getAgents({ filter }))

// --- Promise rendezvous (the building block under Webhook) ---
const id     = yield* Agents.Promises.create
yield* Agents.Promises.complete(id, new TextEncoder().encode("payload"))
const bytes  = yield* Agents.Promises.await(id)            // suspend until completed
const ready  = yield* Agents.Promises.poll(id)             // non-blocking; undefined when pending
```

`Agents.RevertTarget.{toOplogIndex, toBeforeFunction, toBeginningOfRetry}` builds the typed revert anchors; `Agents.Filter.{componentName, statusEq, agentNameLike, and, or, not, ...}` composes the agent-listing predicates the host evaluates server-side.

## Logging & tracing

`effect-golem` automatically wires Effect's `Logger` and `Tracer` to the Golem host. **No setup required:**

- `Effect.log*` calls flow into `wasi:logging/logging.log(...)`. Annotations (`Effect.annotateLogs`), log spans (`Effect.withLogSpan`), and the active host trace/span ids are folded into a single logfmt-style line.
- `Effect.withSpan(...)` calls `golem:api/context.startSpan(...)` and chain under the host's invocation root. `Effect.annotateCurrentSpan` / `attributes:` map to host `setAttribute`. Span errors record `error.message`.

```ts
yield * Effect.logInfo("incremented").pipe(Effect.annotateLogs({ to: 7 }))

yield *
  Effect.gen(function* () {
    // ...
  }).pipe(Effect.withSpan("Counter.add", { attributes: { by: 1 } }))
```

The `Logging` and `Tracing` namespaces are re-exported for users who want to replace / augment the default loggers, log imperatively, read `currentContext` (trace/span ids + W3C headers), or toggle outgoing trace-context forwarding.

## Building & deploying

`effect-golem` integrates with the standard `golem` CLI. A typical `golem.yaml` declares a custom `effect-golem-ts` componentTemplate that:

1. Type-checks `src/` with `tsc`.
2. Bundles `src/main.ts` with Rollup, externalising `effect`, `effect-golem`, `agent-guest`, and all `golem:*` / `wasi:*` host bindings.
3. Injects the bundled JS into the prebuilt QuickJS-backed base WASM that ships with `effect-golem` (under `node_modules/effect-golem/wasm/agent_guest.wasm`) — no Rust/cargo toolchain required for downstream users.

Then:

```bash
golem --local build                                    # build the component
golem --local --yes deploy                             # deploy to a local Golem server

# Invoke a method (--no-stream disables live log streaming):
golem --local agent invoke --no-stream 'Counter("alice")' increment
golem --local agent invoke --no-stream 'Counter("alice")' add '{"by": 5}'

# Inspect the oplog:
golem --local agent oplog 'Counter("alice")'
```

A complete working app — including all the features showcased above — lives under `integration-test/` in this repo.

## Why Effect?

Because Golem agents are durable, replayable, principal-scoped, oplog-backed processes, they need exactly the things Effect already provides: typed errors and successes, structured concurrency, cancellation, scopes, schemas, layers. Rather than reinvent any of that for the WIT host bindings, `effect-golem` exposes Golem's capabilities as ordinary Effect services that compose with everything you already know — `Effect.race`, `Effect.timeout`, `Stream.run*`, `Schema.Struct`, `Layer.provide`, `Scope.make`. Your business logic stays portable and testable; the SDK only translates the boundary.

## License

Apache-2.0 — see [LICENSE](./LICENSE).
