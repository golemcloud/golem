# effect-golem

A TypeScript library for writing [Golem](https://golem.cloud) agents on top of [Effect v4](https://effect.website/) (beta). Agents are declared via `defineAgent` + `method` (Effect Schema for params/return/error). Compiles to a Golem WASM component by injecting bundled user code into a prebuilt QuickJS-backed base WASM (mirrors `golemcloud/golem` `sdks/ts`).

## Layout

- `src/` library (`Agent.ts`, `Client.ts`, `Method.ts`, `WitCodec.ts`, `index.ts`, …); `src/internal/` impl-only modules (`pipeable.ts`, `multipart.ts`, `witTree.ts`, `rdbmsShared.ts`, `snapshotEnvelope.ts`, `durableFunction.ts`, `durabilityMode.ts`, `agent.ts`, `method.ts`, `guest.ts`); `src/effect-bundle.mjs` re-exports `effect` for the standalone runtime bundle.
- `src/Http.ts` HTTP routes namespace (`Http.mount` / `Http.endpoint` + verb shorthands, path/segment IR, `validateAgentHttp`, `HttpRouteError`).
- `golem-types/` ambient `.d.ts` for `golem:*` / `wasi:*` / `agent-guest`. Mirror in `test/mocks/` + alias in `vitest.config.ts` if used at runtime in `src/`.
- `wit/main.wit` (world `agent-guest`) + vendored `wit/deps/` (from `golemcloud/golem` `sdks/ts/wit/deps/`).
- `scripts/` Node build helpers (`generate-agent-template.mjs`, `copy-agent-template.mjs`, `build-types-entry.mjs`).
- `agent-template/` (gitignored) generated Rust crate; `wasm/agent_guest.wasm` (gitignored) base WASM artifact.
- `integration-test/` standalone app with `golem.yaml` (custom `effect-golem-ts` componentTemplate), `rollup.config.component.mjs`, components under `components/<name>/src/main.ts`. Single-component agents are co-located so RPC reuses one `defineAgent`.

## Commands

`npm install` · `npm run build` (tsc + types entry) · `npm run build:bundle` (rollup → `dist/index.mjs` + `dist/effect.mjs`) · `WASI_SDK_PATH=/opt/wasi-sdk npm run build-agent-template` (bundle → wasm-rquickjs → cargo `wasm32-wasip2` → `wasm/agent_guest.wasm`, ~2 min) · `npm run typecheck` · `npm run lint` · `npm run format[:check]` · `npm test` · single test: `npx vitest run test/agent.test.ts -t "registers the Counter"`. Integration: `cd integration-test && npm install && golem -L build && golem -L -Y deploy && golem -L agent invoke -n 'Counter("x")' increment`.

## Required end-to-end testing after every change

Unit tests (`npm test`) only cover the SDK in isolation against host mocks. They cannot catch issues that only surface inside the real Golem WASM runtime — e.g. `JSON.stringify` on bigint UUIDs, missing exports from the embedded `effect-golem` bundle, type-mismatch between WIT bindings and our generated host stubs, runtime principal serialization, oplog/snapshot interaction, etc.

After **every** non-trivial change you must run the integration suite. The canonical drill:

1. `npm test && npm run typecheck && npm run lint && npm run format:check` — gate the SDK changes.
2. `npm run build:bundle` — refresh `dist/index.mjs` (the integration-test consumes `effect-golem` via `file:..` symlink, so this is what the embedded code sees).
3. `WASI_SDK_PATH=/opt/wasi-sdk npm run build-agent-template` — rebuilds `wasm/agent_guest.wasm` so the new `dist/index.mjs` is embedded inside the base WASM. **Required whenever `dist/index.mjs` changes**, otherwise components will load against the old SDK and fail with "Could not find export X in module 'effect-golem'" or stale-behaviour bugs.
4. `cd integration-test && npm install && npm run test:integration` — runs the harness under `test-infra/`. The harness brings up `docker compose` (Postgres + MySQL + Ignite), starts a fresh `golem server run`, runs `golem build && deploy`, then iterates the registered cases sequentially. **Preflight FAILS if a server is already on port 9881** — stop the existing server first, or pass `--no-server` to reuse it.

The harness exposes per-feature flags via `tsx test-infra/run.ts ...`:

- `--list` — print the case registry.
- `--filter <regex>` — run a subset (e.g. `--filter '^(counter|kv)$'`).
- `--no-build` — skip `golem build && deploy`.
- `--no-infra` — skip docker compose lifecycle (use when DBs are already up).
- `--no-server` — assume an external `golem server run` (use when iterating).
- `--stop-on-first-failure`.

The current registry covers: `counter`, `caller`, `host-features`, `booking-saga`, `quota`, `kv`, `blob`, `webhook`, `websocket`, `sqlite-counter`, `pg-counter`, `mysql-counter`, `ignite-counter`, `inventory-saga`. Each case is a self-contained Effect program under `test-infra/cases/<name>.ts`; add new cases by registering them in `test-infra/cases/index.ts`.

If anything fails inside the runtime, treat it as a real bug and fix the SDK — do **not** ship green unit tests + red integration runs.

## Host injection seam (`src/host/`)

All access to the WIT host bindings (`golem:*`, `wasi:*`, `node:sqlite` host extensions, etc.) flows through tagged `Context.Service`-class wrappers under `src/host/`. Each service is a thin 1:1 mirror of a single WIT interface (sometimes split by concern within an interface — e.g. `golem:agent/host` is split into `AgentHostClient` for parse/metadata/webhook and `RpcClient` for the RPC subset). Every service has a `XxxLive` Layer that calls the real WIT specifier; tests provide alternative Layers (Layer.succeed, Layer.scoped, or shared fakes under `test/host/`) instead of mutating module-level `__setX/__resetX` indirection.

The complete bundle is composed in `src/host/HostLive.ts` as a single `Layer.mergeAll(...)`. The dispatcher in `src/Agent.ts` builds this once at module load (see `userRuntimeLayer`) and provides it to every user-effect path: `dispatchInitialize`, `dispatchInvoke`, `dispatchSaveSnapshot`, and `dispatchLoadSnapshot`. **This is the dispatcher-erasure pattern**: SDK combinators are free to thread host-service tags through their `R` channel (e.g. `Durability.checkpoint` requires `OplogClient | AgentHostClient | SelfAgentId`), and the dispatcher's `Effect.provide(userRuntimeLayer)` strips them all before user code observes its own `R`. Consequence: **never** put `Effect.provide(layer)` inside a user-facing combinator — that defeats the seam (it forces a Layer rebuild per call and shadows any test-side override). Always let the host-service tag bubble out to the dispatcher.

When adding a new host-binding wrapper:

1. Drop a new `Xxx{Client,Host}.ts` under `src/host/` exporting a `Context.Service` tag plus a `XxxLive` Layer that calls the real WIT specifier.
2. Add `XxxLive` to `Layer.mergeAll(...)` in `src/host/HostLive.ts`.
3. Consume the service via `yield* Xxx` in the SDK module — let the tag flow into `R`. Do NOT add a module-level `let xxxImpl = ...` indirection or `__setXxxForTest` export.
4. Tests substitute via `Effect.provide(Layer.succeed(Xxx, fake))` (or a shared `test/host/<XxxFake>.ts` factory) — never via mutation of a module-level `__setX/__resetX` hook.

## WIT-drift detection

When a `src/<feature>.ts` module introduces a richer JS / Effect / Schema surface that mirrors a WIT-originated d.ts type, add an assertion that ties the wrapper's actual code to the WIT type — so that WIT regeneration fails the build only when the wrapper diverges from what WIT now says, never as a "the d.ts changed, please mirror it again" snapshot exercise.

Three complementary patterns are in use; pick the one whose value-side actually carries weight, and DO NOT add `null`-keyed witnesses (a `Record<TagUnion, unknown>` whose values are all `null` is just a type-level comment that says "we believe the union is X" — it has no link back to wrapper code, so it does not prove the wrapper handles each tag):

1. **Local `satisfies Record<TagUnion, unknown>` exhaustiveness witness, alongside the wrapper.** Variant constructor namespaces (`PersistenceLevel`, `FunctionType`, `RevertTarget`, `Filter`, …) emit values tagged with a discriminating literal (`{ tag: "..." }`). Add a `void (... satisfies Record<<RawWitType>["tag"], unknown>)` clause directly under the wrapper that maps each WIT tag to its constructor. If WIT regeneration adds a new variant, the `satisfies` clause fails to compile at the wrapper file, naming the missing tag.

   ```ts
   export const PersistenceLevel = {
     persistNothing: { tag: "persist-nothing" } as const,
     persistRemoteSideEffects: { tag: "persist-remote-side-effects" } as const,
     smart: { tag: "smart" } as const,
   } as const

   void ({
     "persist-nothing": PersistenceLevel.persistNothing,
     "persist-remote-side-effects": PersistenceLevel.persistRemoteSideEffects,
     smart: PersistenceLevel.smart,
   } satisfies Record<RawPersistenceLevel["tag"], unknown>)
   ```

   Use `void (… satisfies …)` (not `const _name = … satisfies …`) — the `_`-prefix exemption from `noUnusedLocals` does not apply inside ES modules, so the `void` form is the cleanest way to keep the witness as a pure type-level check.

2. **Switch-internal exhaustiveness** (no separate witness). When a wrapper does `switch (x.tag) { case "a": ... }` and `x` is typed directly as the WIT discriminated union (e.g. `DbValue` from `golem:rdbms/postgres@1.5.0`), the switch itself is the proof: under `noImplicitReturns` (enabled in `tsconfig.json`) a missing case fails compilation with `TS7030: Not all code paths return a value` — pointing directly at the function that needs updating. If the function returns `void` / `undefined` (so `noImplicitReturns` doesn't fire), add an explicit `default: { const _exhaustive: never = x.tag; ... }` arm to force the same check.

   ```ts
   const decodeDbValue = (value: DbValue, ...): unknown => {
     switch (value.tag) {
       case "null": return null
       case "int4": return value.val
       // ... every WIT tag handled ...
     }
     // No `default:` needed — `noImplicitReturns` catches a missing
     // case as soon as a new tag is added on the WIT side.
   }
   ```

   This is preferred over a separate witness for any case where the wrapper is a single switch on a WIT-typed discriminator: there is exactly one place to update, and the type system already knows about it. Don't duplicate that with a `Record<Tag, null>` shadow.

3. **`StructEqual<typeof X.Type, WitType>` pin in `test/wit-drift.ts`.** Schema codecs (`Schema.Struct({...})` / `Schema.Union(...)`) mirroring a WIT `record` are pinned by deriving _both_ sides from real code: the SDK side from `typeof Codec.Type`, the WIT side from the imported d.ts type. The `StructEqual` helper folds away the `readonly` modifier difference between `Schema`'s output and the WIT bindings.

   ```ts
   "Quota.QuotaTokenRecord": StructEqual<
     typeof Quota.QuotaTokenRecord.Type,
     QuotaHost.QuotaTokenRecord
   >
   ```

   `test/wit-drift.ts` is type-only (consumed via `tsc --noEmit`; vitest skips it because the name does not end in `.test.ts`).

Wrappers whose drift is caught by any of these mechanisms do **not** also need a hand-written shape mirror — that is just a host-API snapshot under another name, which this convention explicitly rejects.

Direct host-call signatures (raw `(a, b) => Host.fn(a, b)` wrappers in `src/host/*Client.ts`) are not pinned anywhere — drift in those is caught at the `XxxLive` factory's call site (the call to `Host.fn(a, b)` no longer type-checks), and pinning them defensively would slide the suite into the host-API snapshot it must not become.

## Conventions

- Strict TS (`noUnusedLocals`/`Parameters`, `noImplicitReturns`); ESM (`"type": "module"`); imports must end in `.js` (NodeNext); 2-space indent, no semicolons, double quotes, trailing commas (Prettier).
- Public surface re-exported from `src/index.ts`; runtime hooks (`guest`, `saveSnapshot`, `loadSnapshot`) declared in `src/internal/guest.ts` with inlined types (no `import "agent-guest"` in published `.d.ts`) and flat-re-exported from the barrel.
- Errors as Effect typed failures (e.g. `UnsupportedSchemaError`, `InvalidDataValueError`, `RemoteCallError`); avoid throwing.
- The base WASM externalizes `effect`, `effect-golem`, `agent-guest`, all `golem:*`/`wasi:*`; user component bundles must externalize the same set so all components share one Effect runtime instance.
- When adding deps to user code, prefer importing types/runtime from `effect` and APIs from `effect-golem`.
- HTTP routing metadata is authored through the `Http.*` namespace re-exported from `src/Http.ts` (mount on the agent, endpoints on each method).

## Module organisation conventions

`effect-golem` follows the same module-organisation conventions as the official `effect` / `@effect/*` packages. The conventions are inferred from the source layout of `Effect-TS/effect` and `Effect-TS/effect-smol` (neither repo writes them down — the only documented rule is "do not hand-edit `index.ts`"); we mirror them here so this codebase keeps the same shape.

- **PascalCase filenames**, one module per file. Every public source file under `src/` is named `<ModuleName>.ts` (`Agent.ts`, `Http.ts`, `Quota.ts`, `WitCodec.ts`, `SelfAgentId.ts`, …). Compound names use PascalCase concatenation, no separators (`DurabilityMode`, `WitCodec`, not `Durability-Mode` or `Wit_Codec`). Files under `src/host/` follow the same rule (`AgentHostClient.ts`, `RpcClient.ts`).
- **`src/index.ts` is a namespace-only barrel**. Each public module is surfaced via `export * as <Ns> from "./<Ns>.js"` — never `export * from "./<Ns>.js"` and never named hoists like `export { FooError } from "./Foo.js"`. Consumers always reach the API through its namespace: `Quota.acquireQuotaToken`, `Snapshot.define`, `Webhook.WebhookPayload`, `Http.mount`. This matches `effect`'s `Effect.map` / `Layer.provide` / `SqlClient.make` style. The barrel is hand-edited in this repo (we do not have `pnpm codegen`); keep its entries alphabetised inside each section.
- **Flat DSL aliases — only three.** `defineAgent`, `defineConfig`, and `method` are also re-exported at the package root, in addition to being reachable through `Agent.defineAgent` / `Config.defineConfig` / `Method.method`. They are the canonical authoring constructors used in every `defineAgent({ ... })` call site, so keeping them un-namespaced matches the precedent set by `effect`'s flat `pipe` / `flow` re-exports. **No other symbol gets a flat alias.** When you add a new module, do not add a flat re-export of its error class, its constructor, or its types — make consumers reach them through the namespace (`Snapshot.InvalidSnapshotError`, `Quota.FailedReservationError`, `Webhook.WebhookPayload`).
- **Sub-imports live in their own subtree.** The four RDBMS / SQLite adapters live under `src/<Adapter>/<AdapterName>Client.ts` (mirroring `@effect/sql-pg`'s `src/PgClient.ts` layout): `src/Sqlite/SqliteClient.ts`, `src/Postgres/PgClient.ts`, `src/Mysql/MySqlClient.ts`, `src/Ignite/IgniteClient.ts`. Each subtree is bundled separately by Rollup into `dist/{sqlite,postgres,mysql,ignite}.mjs` and exposed under the `effect-golem/{sqlite,postgres,mysql,ignite2}` package.json export names. The export name (kebab/lowercase, dictated by the npm convention) is independent of the source directory name (PascalCase). The three RDBMS adapters (Postgres / Mysql / Ignite) are each further split into three files within their subtree: `src/<Adapter>/<Helper>.ts` (the public helper namespace — `Pg.ts` / `MySql.ts` / `Ignite.ts` — exposing the `<Adapter>.<helper>(...)` tagged-value constructors plus the `<Adapter>ParamTag` symbol, `<Adapter>Param<T,V>` interface, `is<Adapter>Param` guard, and any helper-only types like `PgRange<T>` / `PgIp` / `IgniteUuid`), `src/<Adapter>/internal/codec.ts` (param encoding + row decoding — `encodeAllParams`, `decodeRows`, `decodeRowsValues` plus all per-type encoders/decoders and value-range constants; consumes the helper module's tag machinery and types), and `src/<Adapter>/<AdapterName>Client.ts` (the facade — TypeId, public types/interfaces, `Context.Service` class, `Connection` / `Target` / `TxTarget` abstractions, `buildConnection`, `makeImpl`, escape function, `make` / `layer`, the public `<Adapter>Client` namespace export, `is<Adapter>Client`, AND a top-of-file re-export of the helper namespace + helper types so consumers still get `import { Pg, PgClient } from "effect-golem/postgres"` unchanged). The Sqlite adapter stays as a single cohesive `src/Sqlite/SqliteClient.ts` file because it has no helper namespace and no `DbValue` codec — `node:sqlite` accepts plain JS values directly. When adding a new sub-import, follow the same shape: source under `src/Foo/FooClient.ts` (and, if it has a tagged-helper namespace + `DbValue` codec, also `src/Foo/Foo.ts` + `src/Foo/internal/codec.ts` from the start), rollup input `src/Foo/FooClient.ts`, output `dist/foo.mjs`, package.json `"./foo": { "types": "./dist/effect-golem-foo.d.ts", "import": "./dist/foo.mjs" }`, plus a matching `body` line in `scripts/build-types-entry.mjs`.
- **Mandatory guest hooks** (`guest`, `saveSnapshot`, `loadSnapshot`) are the WIT protocol bindings the generated `agent-guest` shim imports by name. They live in `src/internal/guest.ts` and are flat re-exported from the package barrel because the host requires them at the package root — they are not API. This is the only case where the barrel reaches into `internal/`. Do not rename them and do not move them into a namespace.
- **Naming collisions are accepted.** A namespace and a value can share a name (e.g. `Principal.Principal` is the `Context.Service` class inside the `Principal` namespace). This matches `effect`'s `Effect.Effect`, `Cause.Cause`, `Schema.Schema` pattern. When `yield* X` would have worked before namespacing, it becomes `yield* X.X` after — that is the expected idiom, not a workaround.
- **JSDoc on every public export.** Every `export` in a public module gets at minimum `@since 1.5.0` and `@category <…>` (use `models`, `constructors`, `errors`, `dsl`, `host services`, `modules`, `symbols`, etc.). The barrel itself documents each `export * as Ns` with one short paragraph plus `@since` + `@category modules`. This matches the rule documented in [`Effect-TS/effect`'s contributing section](https://github.com/Effect-TS/effect#contributing-via-pull-requests) — the only piece of the convention that is officially written down.
- **`src/internal/` for impl-only modules.** Files that exist only to be consumed by other SDK modules — no user-facing surface — live under `src/internal/` with **camelCase** filenames, matching `effect`'s convention (public `Effect.ts` facade + private `internal/core.ts` implementation). The current internals are `pipeable.ts` (the `withPipe` helper), `multipart.ts` (multipart/mixed codec used only by `snapshotEnvelope.ts`), `witTree.ts` (graph codec consumed by `WitCodec`), `rdbmsShared.ts` (error classifier shared by the three RDBMS adapters), `snapshotEnvelope.ts` (snapshot wire-format codec used by the dispatcher), `durableFunction.ts` (the 850-line implementation behind the 30-line `Durability.ts` facade), `durabilityMode.ts` (the persistence-level / atomic-region surface, also re-exported through `Durability`), `agent.ts` (the dispatcher, the registry, and the `userRuntimeLayer` host-services seam — implementation behind the 20-line `Agent.ts` facade), `method.ts` (the `MethodSpec` / `MethodCodec` machinery and `compileMethodSpec` / `invokeDataValue` — implementation behind the 20-line `Method.ts` facade), and `guest.ts` (the WIT `agent-guest` protocol bindings: `guest`, `saveSnapshot`, `loadSnapshot`). When you add a new helper that has no public surface, drop it under `src/internal/` from the start — do not park it at the top level. Public modules that re-export an internal symbol go through their namespace (e.g. `Snapshot.SnapshotEnvelopeError` is re-exported from `Snapshot.ts` even though the class lives in `internal/snapshotEnvelope.ts`); the package barrel never reaches into `internal/` directly except for the three guest hooks (which are flat-exported because the WIT shim resolves them by name).
- **Per-module deep imports are open by default; `internal/` and `host/` are blocked.** `package.json` has a `"./*": { "types": "./dist/src/*.d.ts", "import": "./dist/src/*.js" }` wildcard so consumers can deep-import any public module: `import { Quota } from "effect-golem/Quota"` resolves to `dist/src/Quota.js`, `import { PgClient } from "effect-golem/Postgres/PgClient"` resolves to `dist/src/Postgres/PgClient.js`, and so on. The four bundled sub-imports (`./sqlite`, `./postgres`, `./mysql`, `./ignite2`) keep their explicit entries pointing at the rolled-up `.mjs` bundles — exact matches in the exports map win over the wildcard, so the WASM-targeted bundles stay reachable for components embedded into the base WASM. Two prefixes are explicitly **blocked** with `null`: `"./internal/*": null` (impl-only modules; mirrors `effect`'s convention) and `"./host/*": null` (the WIT host-binding wrappers under `src/host/` are SDK-internal even though they have PascalCase names). Consumers attempting to import them get `ERR_PACKAGE_PATH_NOT_EXPORTED`.
- **Facade / impl split for large modules.** When a top-level module's implementation grows past a comfortable read length, follow the `effect` pattern: keep the public types and re-exports in `src/<Module>.ts`, move the implementation to `src/internal/<module>.ts` (camelCase), and import siblings inside `internal/` as `type *` where needed to avoid runtime cycles. The first example of this in effect-golem is `Durability.ts` (30-line facade re-exporting `* from "./internal/durabilityMode.js"` + `* from "./internal/durableFunction.js"`); `Agent.ts` (20-line facade re-exporting `* from "./internal/agent.js"`) and `Method.ts` (20-line facade re-exporting `* from "./internal/method.js"`) follow the same shape.

## HTTP routes

`effect-golem` only advertises route metadata via `discoverAgentTypes()`; the Golem host owns HTTP serving, auth, CORS, and decoding path/query/header values into a `DataValue` before calling `invoke` — agent code never sees raw HTTP requests.

Authoring uses `Http.mount(...)` on the agent and `Http.<verb>(...)` (or `Http.endpoint(verb, ...)`) on each method:

```ts
import { defineAgent, Http, method, Schema } from "effect-golem"

defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  methods: {
    value: method({ params: {}, success: Schema.Number, http: [Http.get("/value")] }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
      http: [Http.post("/add"), Http.get("/add?by={by}")],
    }),
  },
  impl: ...,
})
```

Path syntax:

- `{var}` — path variable; must reference a constructor param (mount) or method param (endpoint).
- `{*rest}` — catch-all; only valid as the last segment, never in mount paths.
- `{agent-type}` / `{agent-version}` — host-injected system variables (also available as `Http.agentType()` / `Http.agentVersion()`; raw IR via `Http.literal` / `Http.pathVar` / `Http.restVar`).
- `?key={var}&…` — inline query bindings; endpoint-only (mount paths reject `?`).
- `headers: { "X-Foo": "paramName" }` on endpoint options — binds an HTTP header to a method param (case-insensitive).

Rules (enforced by `validateAgentHttp` in `src/Http.ts`):

- Every constructor param must appear as a `{var}` in the mount path.
- Every endpoint binding (`{var}` in path/query, or in `headers`) must reference a method param.
- A method param can be bound from at most one source (path, query, or header).
- Path/query/header bindings are only allowed for "string-bindable" schemas — `Schema.String`, `Schema.Number`, `Schema.BigInt`, `Schema.Boolean`, literals, and refined/branded variants thereof (see `isStringBindableSchema`). Multimodal and unstructured params are body-only.
- `GET` / `HEAD` endpoints may not have unbound (= body) parameters.
- If any method declares `http`, the agent must declare `http: Http.mount(...)`.
- Body convention is host-defined: unbound params on body-allowing verbs become JSON body fields keyed by name.

Verbs: `Http.get` / `post` / `put` / `del` / `patch` / `head` / `options` / `trace` / `connect`, plus `Http.custom("VERB", path, opts?)` for non-standard verbs. `Http.endpoint(verb, path, opts?)` is the generic form.

Auth & CORS: both `Http.mount(...)` and individual endpoints accept `auth?: boolean` and `cors?: string[]`. Merge semantics are host-defined; the SDK emits both verbatim into `HttpMountDetails` / `HttpEndpointDetails`.

Errors: validation failures surface as `HttpRouteError` Effect typed failures from `registerAgent` (alongside `UnsupportedSchemaError`); the synchronous `defineAgent` re-throws them at module-import time so misconfigurations fail fast.

## RPC clients — cancellation

Every `defineAgent` call attaches a typed `client` namespace whose generated proxy methods come in three call shapes:

| Shape                                  | Host call                              | Cancel surface                                                                                         |
| -------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `remote.method(input)` (function call) | `WasmRpc.asyncInvokeAndAwait`          | **Fiber-interrupt** is wired to `future-invoke-result.cancel()`. No explicit handle.                   |
| `remote.method.trigger(input)`         | `WasmRpc.invoke`                       | Fire-and-forget; nothing to cancel after `invoke` returns.                                             |
| `remote.method.schedule(at, input)`    | `WasmRpc.scheduleCancelableInvocation` | Returns `ScheduledInvocation.cancel: Effect<void>` backed by the host's `cancellation-token` resource. |

The bare function-call shape is **fully interruptible**:

```ts
import { Effect, Fiber } from "effect"

const fiber = yield * Effect.forkChild(remote.compute({ size: 10_000_000 }))
yield * Effect.sleep("100 millis")
yield * Fiber.interrupt(fiber) // → host receives `future-invoke-result.cancel()`
```

Composes naturally with `Effect.raceFirst`, `Effect.timeout`, etc. The SDK uses `Effect.acquireUseRelease` so `fut.cancel()` runs on **every** exit path (success, failure, defect, interrupt) — the WIT contract guarantees post-completion cancel is a no-op.

**Best-effort caveat (must be respected by callers):** `future-invoke-result.cancel()` is best-effort by idempotency key — if the remote side has already started executing, the host removes the queued result but the remote work continues. **Treat the side effects of an interrupted remote call as possibly-already-applied.**

The Scala-style "future + cancel" pattern is just the `forkChild` + `Fiber.interrupt` snippet above; no separate `invokeWithCancel` API is needed because fiber-interrupt already propagates to the host.

## Config (`defineConfig` / `Config.*`)

Effect-typed wrapper around `golem:agent/host@1.5.0.get-config-value(name, expected-type) → wit-value` plus the `WasmRpc` constructor's 4th `agent-config: list<typed-agent-config-value>` argument. Wire-compatible with `golem-ts-sdk` `Config<T>` / `Secret<T>`, `golem-rust` `#[derive(ConfigSchema)]`, Scala's `ConfigLoader.createLazyConfig`, and MoonBit's `#derive.config` — same `AgentConfigDeclaration[]` is emitted into `AgentType.config[]` for `golem deploy` to render.

Authoring uses `defineConfig(name, fields)` (a `Context.Service`-class) on the agent's `config:` field:

```ts
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method } from "effect-golem"

export class CounterConfig extends defineConfig("Counter.Config", {
  greeting: Schema.String,
  apiKey: Schema.Redacted(Schema.String), // → secret leaf
  database: Schema.Struct({
    // nested struct → multi-segment paths
    host: Schema.String,
    port: Schema.Number,
  }),
}) {}

defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  config: CounterConfig,
  methods: { greet: method({ params: {}, success: Schema.String }) },
  impl: ({ name }) =>
    Effect.gen(function* () {
      const cfg = yield* CounterConfig // yield the Context.Service tag
      const greeting = yield* cfg.greeting // Effect<string, ConfigError>
      const apiKey = yield* cfg.apiKey.get // Effect<Redacted<string>, ConfigError>
      const dbHost = yield* cfg.database.host // recursive struct
      void apiKey
      void dbHost
      return {
        greet: () => Effect.succeed(`${greeting}, ${name}`),
      }
    }),
})
```

Field shape rules (compiled by `compileConfig` in `src/Config.ts`):

- A bare `Schema.Top` becomes a **local** leaf accessed as `Effect<T, ConfigError>`.
- `Schema.Redacted(inner)` becomes a **secret** leaf accessed as `{ get: Effect<Redacted<T>, ConfigError> }`. Use `Redacted.value(r)` to extract the underlying string only at the moment you actually need it.
- `Schema.Struct({...})` recurses into the children, prefixing each field's path with the parent's name. An empty `Schema.Struct({})` materialises as `{}` in the shape (no leaves declared).
- `Schema.Option(inner)` is the canonical "soft-defaulting" pattern: when the host is given an undeclared key with an `option<T>` `valueType`, it returns `none` instead of trapping. Use this for keys that may be missing in some environments.
- Anything `toWitCodec` can't represent (e.g. `Schema.Any`) is rejected at registration time with `UnsupportedSchemaError`.

Caching:

- Plain (non-secret) leaves are memoized via `Effect.cached` for the duration of **one host invocation only** (i.e. one call into `dispatchInitialize` or one call into `dispatchInvoke`). Two reads of the same field inside the same handler share a single host call.
- Secret leaves are **never cached** — every read of `cfg.apiKey.get` issues a fresh `getConfigValue` call. This matches all four official SDKs (TS / Rust / Scala / MoonBit) and lets the Golem host rotate secrets between invocations.
- Reads inside `impl` (the constructor) and reads inside method handlers each get their own `Effect.cached` cell; nothing crosses the impl ↔ handler boundary.

`AgentType.config` propagation: `registerAgent` calls `def.config.__compile()` exactly once per agent type and copies the resulting `AgentConfigDeclaration[]` into `agentType.config`. `dispatchDiscoverAgentTypes` ships the unchanged metadata to the host. Each declaration carries `{ source: "local" | "secret"; path: string[]; valueType: WitType }`; the WIT ADT has no description / default-value field (the four official SDKs match this).

RPC overrides (typed):

```ts
yield *
  Counter.client.get(
    { name: "alice" },
    {
      overrides: {
        greeting: "rpc-override",
        database: { host: "db.override" },
        // apiKey is NOT in the type — Schema.Redacted leaves are stripped
        // from `NonSecretOverride<F>` by both compile-time and runtime
        // guards in `encodeOverrides`.
      },
    },
  )
```

`encodeOverrides` walks the user-supplied object alongside the compiled leaf table, encodes each leaf's value through its `WitCodec`, and emits a `TypedAgentConfigValue[]` array. The result is concatenated to any explicit `opts.agentConfig` and passed to the `WasmRpc` constructor. Unknown override paths and overrides on secret leaves both fail with `ConfigError({_tag: "Unsupported"})`.

Snapshot interaction: **config values are never embedded in the snapshot envelope.** Match all four official SDKs. After a snapshot load, `dispatchLoadSnapshot` re-runs the constructor (`impl`) and per-invocation reads pick up whatever the host currently returns — host-side rotation / overrides are visible immediately. The dispatcher provides the config service to user-managed `Snapshot.custom({ ... })` `save` and `load` handlers too, so they can read the current config when serialising / restoring.

Errors: `ConfigError` carries a tagged `reason` (`HostTrap | DecodeFailure | WireMismatch | Unsupported`) and the failing path. Registration-time validation produces `UnsupportedSchemaError` (e.g. `Schema.Any` as a leaf, malformed `Schema.Redacted`, or — defence-in-depth — duplicate compiled paths). Both are exported from the package barrel.

Test mocks: `test/mocks/golem-agent-host.ts` exposes a settable `getConfigValueImpl` plus `__set/__resetGetConfigValueForTest` hooks; `src/Config.ts` mirrors this with module-local `getConfigValueImpl` indirection so unit tests can drive arbitrary `WitValue` returns without touching the real host.

## Webhooks (`Webhook.*`)

`effect-golem` exposes the host's webhook integration via a small `Webhook` namespace re-exported from the package barrel. Wire-compatible with `golem-ts-sdk.createWebhook()` / `golem-rust.create_webhook()`: both call `golem:api/host.create-promise` followed by `golem:agent/host.create-webhook(promise-id)` and return the host-minted URL verbatim.

```ts
import { Effect, Schema } from "effect"
import { defineAgent, Http, method, Webhook } from "effect-golem"

const PaymentEvent = Schema.Struct({ id: Schema.String, status: Schema.String })

defineAgent({
  name: "PaymentWatcher",
  constructorParams: { name: Schema.String },
  http: Http.mount("/watchers/{name}", { webhookSuffix: "/payments" }),
  methods: {
    waitForPayment: method({
      params: {},
      success: PaymentEvent,
      http: [Http.post("/wait")],
    }),
  },
  impl: () =>
    Effect.gen(function* () {
      return {
        waitForPayment: () =>
          Effect.gen(function* () {
            const hook = yield* Webhook.create
            // ... share `hook.url` with the payment provider via an outgoing API call ...
            const payload = yield* hook.await
            return yield* payload.decode(PaymentEvent)
          }),
      }
    }),
})
```

API:

- `Webhook.create: Effect<WebhookHandle, AgentsHostError | WebhookHostError>` — allocates a fresh host promise, then mints a public POST URL bound to it. Two host calls under the hood (`create-promise` + `create-webhook`); failure between the two leaves an unused promise in the host's table — the SDK does NOT garbage-collect it.
- `WebhookHandle` — `{ url, promiseId, await: Effect<WebhookPayload, AgentsHostError>, poll: Effect<WebhookPayload | undefined, AgentsHostError> }`. `await` durably suspends until the URL is POSTed to (visible in the oplog as `SUSPEND` between `pollable.ready` calls); `poll` is non-blocking.
- `WebhookPayload` — wraps the raw POST body bytes. `bytes` (raw `Uint8Array`), `text()` (UTF-8), `json<T>()` (synchronous, throws on bad JSON), `decode(schema): Effect<A, WebhookDecodeError | Schema.SchemaError, R>` (recommended Effect-typed path).
- `WebhookHostError` / `WebhookDecodeError` — exported from the package barrel.

Constraints (host-enforced, surfaced as `WebhookHostError`):

- The agent type must be currently deployed via an HTTP API at the moment of the call — i.e. the agent declares `Http.mount(...)` AND the deployment lists the agent under `httpApi.deployments.<env>.agents`. Calling `Webhook.create` from an agent that is not deployed via HTTP traps.
- The promise must have been created by the same component as the one calling `create-webhook`. Cross-component promise reuse is rejected by the host.

`webhookSuffix` syntax (in `Http.mount({ webhookSuffix })`):

- Same parser as the mount path itself: literals + `{constructor-param}` + `{agent-type}` / `{agent-version}` system variables.
- No `?key={var}` query bindings (parser rejects `?`).
- No `{*rest}` catch-all (parser rejects).
- Every `{var}` segment must reference a constructor parameter on the agent (validated at registration time inside `validateAgentHttp`); otherwise registration fails with `HttpRouteError`.
- Falls back to the agent type name in kebab-case at deployment time when omitted.

URL anatomy at deployment time (host-built, no SDK involvement):

```
https://<domain>/<webhooksPrefix>/<webhookSuffix>/<base64url(AgentWebhookId)>
                  golem.yaml         Http.mount        signed by host
```

The trailing `<base64url(AgentWebhookId)>` is HMAC-SHA256-signed by the host on `create-webhook` and verified on inbound POST — the SDK never forges or verifies it. POSTing to the URL returns `204 No Content` on success and completes the underlying promise atomically with the request body bytes.

The integration test ships a `WebhookAgent` (under `integration-test/components/agents/src/webhook-agent.ts`) deployed against `effect-golem.localhost:9006` with `webhookSuffix: "/inbox"`. Drive a round-trip with:

```
URL=$(golem -L agent invoke -n 'WebhookAgent("demo")' prime | grep -oE 'http://effect-golem[^"]+')
golem -L agent invoke -n 'WebhookAgent("demo")' waitForEvent &
sleep 2
curl -X POST -H 'Content-Type: application/json' -d '{"hello":"world"}' "$URL"
wait
```

The oplog (`golem -L agent oplog 'WebhookAgent("demo")'`) shows the canonical `CALL golem::api::create_promise` → `CALL golem::agent::create_webhook` pair on `prime`, then `INVOKE waitForEvent` → `CALL golem::api::get_promise_result` → `CALL io::poll::pollable::ready` → `SUSPEND` (durable wait) → wakeup → `INVOKE COMPLETED` on the wait side.

## Snapshotting

Snapshotting is opt-in via a per-agent `snapshot` field on `defineAgent`. When set, the agent's WIT `snapshotting` metadata becomes `enabled(...)` (instead of the default `disabled`) and the SDK wires up `golem:api/save-snapshot.save` / `golem:api/load-snapshot.load`. The wire envelope is bit-for-bit compatible with the official `golem-ts-sdk` (so components can be cross-loaded).

`load` _replaces_ `initialize` on restore: the host calls one or the other, never both. The SDK reads `GOLEM_AGENT_ID` from `wasi:cli/environment`, calls `golem:agent/host.parse-agent-id` to recover constructor params, runs `impl` with the principal embedded in the snapshot envelope, then applies the restored state on top.

Two variants:

- **Auto (schema-driven)** — `Snapshot.define({ schema, policy })`. The SDK manages a `Ref.Ref<State>`; `impl` receives a second arg `snap` whose `init(initial)` allocates and registers the Ref. Encodes via `Schema.encodeUnknown` → JSON envelope (`mimeType: "application/json"`).
- **Custom** — `Snapshot.custom({ policy })`. `impl` receives `snap.register({ save, load })`; the user owns the bytes. Wire format is the binary v2 envelope (`mimeType: "application/octet-stream"`), with the principal embedded in a 5-byte header (`u8 version=2 + u32-be princLen + princJson + userBytes`).

```ts
import { defineAgent, method, Schema, Snapshot } from "effect-golem"
import { Effect, Ref } from "effect"

defineAgent({
  name: "Counter",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number, owner: Schema.String }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ count: 0, owner: name })
      return {
        value: () => Ref.get(state).pipe(Effect.map((s) => s.count)),
        add: ({ by }) =>
          Ref.updateAndGet(state, (s) => ({ ...s, count: s.count + by })).pipe(
            Effect.map((s) => s.count),
          ),
      }
    }),
})
```

Policy constructors (in `Snapshot.policy`):

- `default` (alias `manual`) — host-default cadence; mapped to WIT `enabled(default)`.
- `periodic(d)` — `Duration.Input` (e.g. `"5 minutes"` or `Duration.seconds(30)`); mapped to `enabled(periodic(<u64-nanos>))`.
- `everyN(n)` — positive integer in `1..=65535`; mapped to `enabled(every-n-invocation(n))`.

Rules (enforced inside `dispatchInitialize` / `dispatchLoadSnapshot`):

- Inside `impl`, `snap.init` (auto) / `snap.register` (custom) must be called exactly once. Forgetting raises `SnapshotNotBoundError` at init/load time; calling twice raises `SnapshotAlreadyBoundError` from the second call.
- `initialize` and `load` are mutually exclusive — calling either while an agent is already active throws.
- `load` rejects mismatched envelopes (auto agent + binary envelope, or custom agent + JSON envelope) with `SnapshotEnvelopeError`.
- `multipart/mixed` envelopes (the official SDK uses these when SQLite databases are present) are rejected with `UnsupportedSnapshotFormatError`; `effect-golem` does not yet have a SQLite story.

`multipart/mixed` envelopes ARE now supported when an auto-snapshot agent declares one or more SQLite databases via `Snapshot.define({ databases: ["..."] as const })`. See the dedicated section below.

Schema evolution: snapshots are JSON-encoded under the auto variant; users are responsible for keeping their `schema` backward-compatible (or versioning the payload manually). The SDK does not migrate.

Errors: `InvalidSnapshotError` (from `registerAgent`, e.g. `everyN(0)`), `SnapshotNotBoundError`, `SnapshotAlreadyBoundError`, `SnapshotEnvelopeError`, `UnsupportedSnapshotFormatError`, plus the SQLite-aware errors `SnapshotDatabaseDuplicateAttachError`, `SnapshotDatabaseMissingPartError`, `SnapshotDatabaseUnknownPartError`, `SnapshotDatabaseNotInAutocommitError`, `SnapshotDatabaseHasAttachmentsError`. All exported from the package barrel.

### SQLite databases (`Snapshot.define({ databases: [...] as const })`)

The auto variant can also capture one or more `node:sqlite` `DatabaseSync` handles. Pre-declare the names with a `const`-typed tuple and attach each one inside `impl`:

```ts
import { defineAgent, method, Schema, Snapshot } from "effect-golem"
import { SqliteClient } from "effect-golem/sqlite"
import { Effect } from "effect"

defineAgent({
  name: "SqliteCounter",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({ params: {}, success: Schema.Number }),
    add: method({ params: { by: Schema.Number }, success: Schema.Number }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      yield* snap.init({})
      const sql = yield* SqliteClient.make({ filename: ":memory:" })
      yield* sql.exec(`CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER)`)
      yield* sql.exec(`INSERT OR IGNORE INTO counters (id, count) VALUES ('${name}', 0)`)
      yield* snap.attachDatabase("counters", sql)
      // ...handlers...
    }),
})
```

Wire format: when `databases` is non-empty the snapshot envelope becomes `multipart/mixed`. The `state` part carries `{ version: 1, principal, state }` JSON; one `db:<name>` part per declared database carries the raw SQLite file bytes (`application/x-sqlite3`). Bit-compatible with `golem-ts-sdk`.

Restore order: the constructor (`impl`) runs first — meaning DDL must be idempotent (`CREATE TABLE IF NOT EXISTS`, `INSERT OR IGNORE`, etc.). Once `impl` returns, the SDK calls `restoreDatabaseSync(handle, bytes)` for each `db:<name>` part, which **overwrites** the in-memory database in place. Finally the auto state Ref is restored from the JSON `state` part.

Constraints (enforced strictly):

- Every declared database name must be attached exactly once (`SnapshotDatabaseMissingPartError` / `SnapshotDatabaseDuplicateAttachError`).
- At save time, every attached DB must be in autocommit mode (`SnapshotDatabaseNotInAutocommitError`) and must not have ATTACHed schemas beyond `main`/`temp` (`SnapshotDatabaseHasAttachmentsError`).
- At load time, the multipart envelope's `db:<name>` parts must match the declared set exactly (`SnapshotDatabaseUnknownPartError` / `SnapshotDatabaseMissingPartError`).
- Database names must match `/^[a-zA-Z_][a-zA-Z0-9_]*$/`.
- `Snapshot.custom(...)` agents do **not** support `databases` (custom is fully user-managed).

`attachDatabase` accepts both the SDK's own `SqliteClient` (probed via `SqliteClientTypeId`) and a raw `node:sqlite` `DatabaseSync` handle.

### SQLite caveat: use `node:sqlite`, not `better-sqlite3` / `@effect/sql-sqlite-node`

The Golem runtime is `wasm-rquickjs`. It exposes Node's built-in `node:sqlite` (`DatabaseSync`/`StatementSync`) plus three host extensions (`serializeDatabaseSync`, `restoreDatabaseSync`, `isAutocommitDatabaseSync`). Native N-API addons such as `better-sqlite3` (which `@effect/sql-sqlite-node@4.0.0-beta.57` still depends on) cannot run inside the WASM runtime. The `effect-golem/sqlite` sub-import is a hand-rolled adapter targeting `node:sqlite` directly — use it instead.

The adapter **implements the official `effect/unstable/sql` `SqlClient` interface** (in Effect v4 the `@effect/sql` core was merged into the main `effect` package and lives at `effect/unstable/sql/{SqlClient,Statement,SqlError,SqlConnection,Migrator,SqlSchema,SqlResolver,SqlStream,SqlModel}`), so users get `SqlSchema` / `SqlResolver` / `Migrator` / tagged-template queries (`yield* sql\`SELECT ...\``) for free against our adapter. Snippet:

```ts
import { SqliteClient } from "effect-golem/sqlite"

const sql = yield * SqliteClient.make({ filename: ":memory:" })
yield *
  sql.exec(
    `CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER NOT NULL DEFAULT 0)`,
  )
yield * sql`INSERT OR IGNORE INTO counters (id, count) VALUES (${name}, 0)`
const rows = yield * sql`SELECT count FROM counters WHERE id = ${name}`
```

`SqliteClient.make`/`SqliteClient.layer`/`SqliteClient.fromDatabase` return / provide both the `effect-golem/sqlite` `SqliteClient` extension (which adds `export: Effect<Uint8Array, SqlError>` for snapshotting and `exec(sql)` for parameter-less DDL/seed batches) and the canonical `Client.SqlClient` tag. `executeStream` is unimplemented (`Stream.die`) because `node:sqlite`'s `StatementSync` has no native cursor; everything else is wired through `Statement.makeCompilerSqlite` so the SQL dialect, placeholders, identifier escaping, and result-column transforms match the rest of the Effect SQL ecosystem.

## Durable function wrapper (`Durability.wrap` / `wrapInfallible`)

`effect-golem` ships an Effect-idiomatic wrapper around the `golem:durability/durability@1.5.0` host interface, mirroring the Rust SDK's `Durability::new + is_live + persist + replay` triplet that every `golem-ai` library uses. The high-level entry point is `Durability.wrap` (and `Durability.wrapInfallible`):

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
  impl: () =>
    Effect.gen(function* () {
      return {
        fetchQuote: ({ symbol }) =>
          Durability.wrap(
            {
              iface: "myapp",
              function: "fetchQuote",
              functionType: Durability.FunctionType.writeRemote,
              requestSchema: Schema.Struct({ symbol: Schema.String }),
              success: Schema.Struct({ symbol: Schema.String, price: Schema.Number }),
              // optional: error: SomeErrorSchema
            },
            { symbol },
            // body — runs once in live mode, skipped on replay
            Effect.sync(() => fetchFromRemote(symbol)),
          ),
      }
    }),
})
```

Live vs replay protocol (matches Rust bit-for-bit):

- **Live mode** (`is_live` true OR persistence-level is `persist-nothing`): runs `body` inside `withPersistenceLevel(persistNothing, …)` to suppress nested oplog writes, encodes `(request, Result.succeed(value) | Result.fail(error))` to a `ValueAndType`, calls `persistDurableFunctionInvocation(qualifiedName, requestVT, responseVT, functionType)`, then `endDurableFunction(...)`.
- **Replay mode**: skips `body`, calls `readPersistedDurableFunctionInvocation()`, validates the entry's `functionName` AND `functionType`, decodes the recorded value through `Schema.Result(success, error)`, calls `endDurableFunction(...)`, returns the original value or fails with the recorded typed error.
- **Defects / interruption**: skip BOTH `persist` and `endDurableFunction`. The bracket stays open, mirroring Rust's "panic = abnormal termination" behavior.

Key guarantees:

- **Bit-compat with `golem-rust`**: function names are emitted as `${iface}::${function}`; responses use WIT `result<ok, err>` (via `Schema.Result`), not `Either`. Verified via the `host-features::wrappedQuote` integration component — live oplog entries display as `CALL host-features::wrappedQuote / input: {symbol: "AAPL"} / result: {symbol, price}` (success) or `err({code, symbol})` (typed fail).
- **Nesting is allowed**: a typical pattern is to use `wrap` to mark a higher-level persisted block whose body itself contains other custom or host-side durable calls (including a nested `wrap`). In live mode `wrap` explicitly wraps the body with `withPersistenceLevel(persist-nothing, ...)`, so inner host I/O does not double-record into the outer block's oplog. In replay mode the body is skipped entirely.
- **Error ergonomics**: SDK-internal failures (`DurabilityHostError`, `DurabilityReplayMismatchError`, `DurabilityDecodeError`, `UnsupportedSchemaError`) are routed into the **defect** channel, NOT the typed `E`. Method authors only declare their own typed errors; infrastructure errors propagate as panics through the dispatcher's normal failure path.
- **Schema services**: `wit-codec` services (`EncodingServices` / `DecodingServices`) flow through `wrap`'s `R` channel, so schemas with services compose normally.

`Durability.FunctionType.writeRemoteBatched(begin?)` and `writeRemoteTransaction(begin?)` are NOT accepted by `wrap` — they imply a multi-step lifecycle the unary combinator does not model. Use the lower-level escape hatches (`beginDurableFunction`, `endDurableFunction`, `persistDurableFunctionInvocation`, `readPersistedDurableFunctionInvocation`, `currentDurableExecutionState`, `isLive`, `observeFunctionCall`) to compose those flows manually.

`Durability.wrapInfallible` is the same combinator for `Effect<A, never, R>` bodies; the response envelope is a bare `success` value (no `Result` wrapping), useful for stream "begin" markers and other never-failing durable points.

## Sagas / multi-step transactions (`Saga.*`)

`effect-golem` ships an Effect-idiomatic saga / compensating-transaction module on top of the Golem oplog primitives. The wire layout matches the official `golem-ts-sdk` / `golem-rust-sdk` saga implementation: each step runs in its own atomic region (`mark-begin-operation` / `mark-end-operation`); successful steps register a compensation effect that fires on transaction failure; the infallible variant additionally calls `set-oplog-index(checkpoint)` to ask the host to replay from the captured checkpoint.

The API mirrors `@effect/workflow`'s `Workflow.withCompensation` shape — the canonical Effect-TS compensation combinator — with one Golem-specific extension (`withFallibleCompensation`) for surfacing partial-rollback failures.

```ts
import { Effect, Schema } from "effect"
import { defineAgent, method, Saga } from "effect-golem"

// Reusable execute+compensate pair (parity with official Golem SDKs).
const bookFlight = Saga.operation({
  execute: ({ flightId }: { flightId: string }) =>
    Effect.gen(function* () {
      /* ... */
      return { ref: `FLT-${flightId}` }
    }),
  compensate: ({ flightId }, booking, _cause) => cancelFlight(booking.ref).pipe(Effect.ignore),
})

// fallible: returns Saga.TransactionFailure<E> on body failure.
const result =
  yield *
  Saga.fallibleTransaction(
    Effect.gen(function* () {
      const flight = yield* bookFlight({ flightId: "AA1" })
      const hotel = yield* bookHotel({ hotelId: "H7" })
      return { flight, hotel }
    }),
  )
// result : Effect<{ flight, hotel }, Saga.TransactionFailure<E> | DurabilityHostError | OplogHostError | NestedSagaError, R>

// infallible: drains compensations + setOplogIndex(checkpoint) + Effect.never.
const value = yield * Saga.infallibleTransaction(body) // body must be Effect<A, never, R>
```

Combinators (re-exported from the package barrel as `Saga`):

- `Saga.withCompensation(effect, (value, cause) => Effect<void, never, R>)` — primary, idiomatic combinator. Compensation cannot fail. Mirrors `@effect/workflow.Workflow.withCompensation` exactly.
- `Saga.withFallibleCompensation(effect, (value, cause) => Effect<void, E, R>)` — Golem-specific extension. The first compensation failure surfaces as `TransactionFailure.FailedAndRolledBackPartially { error, compensationError }`. Subsequent compensations still run on a best-effort basis.
- `Saga.operation({ execute, compensate })` — paired-step factory. Returns a function `(input) => Effect<…>`; internally uses `withCompensation`. Matches the `Operation` type from the official Golem SDKs.
- `Saga.fallibleTransaction(body)` — entry point. Body returns `Effect<A, E, R>`; failure becomes `TransactionFailure<E>`. Defects propagate unchanged. Interruption propagates unchanged (compensations still run via the surrounding scope).
- `Saga.infallibleTransaction(body)` — entry point. Body returns `Effect<A, never, R>`; on operation-level failure or interruption, drains compensations in reverse order, calls `setOplogIndex(checkpoint)`, and parks the fiber via `Effect.never` until the host preempts. Defects propagate unchanged.

Wire mechanics:

- On entry the SDK captures `Oplog.currentIndex` as the checkpoint. NO outer atomic region is opened — each step runs in its own.
- Each `Saga.operation` / `Saga.withCompensation` wraps the step body in `Durability.atomically(...)`, so the host oplog shows balanced `BeginAtomicRegion` / `EndAtomicRegion` markers per step.
- Compensations are registered via `Scope.addFinalizer(scope, …)`. The transaction wrapper signals "now drain" by stashing the failure cause in a fiber-local `CauseStoreRef`; the finalizers read this and gate themselves on `cause !== null`.
- Reverse-order drain is guaranteed by Scope's LIFO finalizer semantics. Drains run sequentially, uninterruptibly (Scope close is uninterruptible).
- Infallible failure path: after the drain, `setIndex(checkpoint) >> Effect.never`. The host preempts and replays from the checkpoint. The fiber's `R` channel is `Exclude<R, Scope>` — Scope is internal.

Failure-cause classification:

| Cause class             | Fallible saga                             | Infallible saga                          |
| ----------------------- | ----------------------------------------- | ---------------------------------------- |
| `Effect.fail` (typed E) | compensate reverse → `TransactionFailure` | compensate reverse → `setIndex >> never` |
| `Effect.interrupt`      | propagate unchanged (comps still run)     | compensate reverse → `setIndex >> never` |
| `Effect.die` (defect)   | propagate unchanged (NO comps)            | propagate unchanged (NO rewind)          |

Errors: `Saga.NestedSagaError` (raised when a saga is started inside an already-active saga in the same fiber tree — the host's atomic-region bracketing and the in-fiber checkpoint stack are single-frame); `TransactionFailure<E>` is a tagged union (`FailedAndRolledBackCompletely { error }` / `FailedAndRolledBackPartially { error, compensationError }`). Both are exported from the package barrel.

Caveats:

- `infallibleTransaction` only meaningfully retries when the oplog is being persisted. Calling it under `Durability.withPersistenceLevel(persistNothing, …)` weakens the rewind guarantee (there is nothing for the host to replay).
- `Retry.withPolicy` / `withIdempotenceMode` / `withPersistenceLevel` should be composed externally — they are NOT bundled into the saga module.
- Nested sagas raise `NestedSagaError` because the host's atomic-region bracketing and the in-fiber checkpoint stack are inherently sequential. The check uses `Context.Reference` (Effect 4's fiber-local primitive), so concurrent unrelated fibers correctly each get their own saga frame.

The integration test suite includes `BookingSaga` (drives `fallibleTransaction` with happy-path, `FailedAndRolledBackCompletely`, and `FailedAndRolledBackPartially` outcomes) and `InventorySaga` (drives `infallibleTransaction` with a deliberate first-attempt failure that rewinds via `set-oplog-index`). Inspect the oplog with `golem -L agent oplog 'BookingSaga("demo")'` to see balanced `BeginAtomicRegion` / `EndAtomicRegion` per step plus a `Jump` entry on infallible retry.

## Logging & Tracing

`effect-golem` automatically wires Effect's `Logger` and `Tracer` to the Golem host:

- `Effect.log*` calls flow into `wasi:logging/logging.log(level, "", "level=… ts=… trace_id=… span_id=… key=value :: message")`. Annotations (`Effect.annotateLogs`), log spans (`Effect.withLogSpan`), and the active host trace/span ids are folded into a single logfmt-style line. Zero trace/span ids are suppressed.
- `Effect.withSpan` calls `golem:api/context.startSpan(name)` and chain under the host's invocation root (via `Tracer.externalSpan` injected by the dispatcher's `withInvocationParent` helper). `Effect.annotateCurrentSpan` / `attributes:` map to host `setAttribute`. On `Exit.failure` the SDK records `error="true"` + `error.message=Cause.pretty(...)` on the host span.
- All host failures inside the logger / tracer are swallowed. Telemetry never breaks user code.

The dispatcher applies the combined `Logging.layer + Tracing.layer + withInvocationParent` to: `impl` (constructor), every method handler in `dispatchInvoke`, and the user-managed snapshot save/load handlers. Auto snapshot save/load runs without it (no user-effect to instrument).

The `Logging` and `Tracing` namespaces are re-exported from the package barrel for users who want to:

- replace / augment the default loggers (`Logging.layer` to replace, `Logging.mergeLayer` to add alongside the defaults)
- imperative log: `yield* Logging.log("warn", "context", "message")` returning `Effect<void, LoggingHostError>`
- read host invocation context: `yield* Tracing.currentContext` returning `{ traceId, spanId, traceContextHeaders }`
- toggle outgoing W3C header forwarding: imperative `Tracing.allowForwardingTraceContextHeaders(true)` or scoped `Tracing.withForwardedHeaders(true, body)`

Limitations (intentional):

- Effect span events (the implicit `Logger.tracerLogger` path) are NOT replayed as host span events — the host has no event API. Span-event state is kept locally on the `GolemSpan` so reading code keeps working; for delivery, log lines go through `wasi:logging` instead.
- `Effect.withSpan` requests with an explicit `parent` whose `(traceId, spanId)` does NOT match `currentContext()` fall back to `Tracer.NativeSpan` (no host span emitted) — this is the cross-fiber / drift safety net; for the typical sequential `withSpan` nesting the host stack matches Effect's view exactly.
- `Effect.withSpan(..., { root: true })` is treated as "child of current host invocation root" because Effect normalises every top-level span to `root: true` and Golem's invocation context is the canonical root. There is no way to detach in the Golem model.

WIT / mocks: the integration imports `wasi:logging/logging` and `golem:api/context@1.5.0`. Both are listed in `rollup.config.mjs` as externals and aliased to `test/mocks/wasi-logging.ts` / `test/mocks/golem-api-context.ts` in `vitest.config.ts`.

## RDBMS clients (Postgres / MySQL / Ignite)

`effect-golem` ships three sub-imports that wrap Golem's `golem:rdbms/*@1.5.0` host bindings as official `effect/unstable/sql/SqlClient` instances. Each adapter is a separate sub-import (NOT re-exported from `effect-golem`):

- `effect-golem/postgres` — Postgres 14+ (`golem:rdbms/postgres@1.5.0`)
- `effect-golem/mysql` — MySQL 8 / MariaDB (`golem:rdbms/mysql@1.5.0`)
- `effect-golem/ignite2` — Apache Ignite 2.x (`golem:rdbms/ignite2@1.5.0`)

All three expose the full `SqlClient` API (tagged-template queries, `withTransaction`, `executeStream`, `SqlSchema` / `SqlResolver` / `Migrator` integration), brand themselves with a `Symbol.for`-keyed TypeId, and provide both a `make(config)` factory (`Effect<…, SqlError, Scope>`) and a `layer(config)` `Layer` that registers the client under both the adapter-specific `Context.Service` tag and the canonical `Client.SqlClient` tag.

```ts
import { PgClient } from "effect-golem/postgres"
import { MySqlClient } from "effect-golem/mysql"
import { IgniteClient } from "effect-golem/ignite2"

const sqlPg = yield * PgClient.make({ connectionAddress: "postgres://user:pw@host:5432/db" })
const sqlMy = yield * MySqlClient.make({ connectionAddress: "mysql://user:pw@host:3306/db" })
const sqlIg =
  yield * IgniteClient.make({ connectionAddress: "ignite://user:pw@host:10800?pool_size=4" })
```

Address formats (passed verbatim to the host's `DbConnection.open(...)`):

- Postgres — `postgres://[user[:pass]@]host[:port]/dbname[?param=…]`. Default port 5432.
- MySQL — `mysql://[user[:pass]@]host[:port]/dbname[?param=…]`. Default port 3306.
- Ignite — `ignite://[user:pass@]host:port[?pool_size=N&tls=true]`. Default port 10800.

### Why not `@effect/sql-pg` / `@effect/sql-mysql2` / native drivers?

Inside Golem the user component runs in `wasm-rquickjs`. **Native N-API addons cannot load there** — `@effect/sql-pg` depends on `pg`, `@effect/sql-mysql2` depends on `mysql2`, etc. All three of those packages will throw at import time. The `effect-golem/{postgres,mysql,ignite2}` adapters delegate the actual wire protocol to the Golem host (which speaks each protocol itself) and only expose the JS-side `SqlClient` shim. **Always use the `effect-golem/*` adapters; never add `@effect/sql-pg` etc. as a dependency.**

### State is external — NOT covered by Golem snapshots

RDBMS state lives in the database, **outside** the Golem worker. Golem snapshots (the `Snapshot.define(...)` / `Snapshot.custom(...)` machinery) only persist what the SDK manages (the auto-state Ref + any attached `node:sqlite` `DatabaseSync` handles). Postgres / MySQL / Ignite tables are **not** captured into snapshots and are **not** restored on load. This is intentional: a Golem update should re-attach to the same external database, not duplicate its state.

Practical consequences:

- Treat the database as the durable source of truth. Use `CREATE TABLE IF NOT EXISTS` + `INSERT … ON CONFLICT DO NOTHING` (Postgres) / `INSERT IGNORE` (MySQL) / `MERGE INTO` (Ignite) so re-running `impl` after a load is idempotent.
- Per-agent snapshots can still be enabled — they will only snapshot the JS-side state. The integration-test counters use `Snapshot.define({ schema: Schema.Struct({}), policy: Snapshot.policy.everyN(10) })` purely to drive the snapshot oplog entry.
- Don't try to mix `Snapshot.define({ databases: [...] })` (which is for SQLite) with an RDBMS adapter — the SQLite-databases feature only knows how to serialize `node:sqlite` handles.

### Param encoding & row decoding

Each adapter accepts plain JS values in tagged-template parameters and maps them conservatively to the host's `DbValue`:

| JS value             | Postgres                             | MySQL                     | Ignite                     |
| -------------------- | ------------------------------------ | ------------------------- | -------------------------- |
| `string`             | `text`                               | `varchar`                 | `db-string`                |
| `boolean`            | `boolean`                            | `boolean`                 | `db-boolean`               |
| safe int32 `number`  | `int4`                               | `int`                     | `db-int`                   |
| larger int `number`  | `int8`                               | `bigint`                  | `db-long`                  |
| non-integer `number` | `float8`                             | `double`                  | `db-double`                |
| `bigint`             | `int8` (range-checked)               | `bigint` (range-checked)  | `db-long` (range-checked)  |
| `Uint8Array`         | `bytea`                              | `blob`                    | `db-byte-array`            |
| `Date`               | `timestamptz` (UTC)                  | `datetime` (UTC)          | `db-date` (epoch-ms UTC)   |
| `null` / `undefined` | `null`                               | `null`                    | `db-null`                  |
| **NaN / ±Infinity**  | rejected (`SqlSyntaxError`)          | rejected                  | rejected                   |
| Anything else        | use the dialect's `Pg.<helper>(...)` | use `MySql.<helper>(...)` | use `Ignite.<helper>(...)` |

For richer types — `json`, `uuid`, `decimal/numeric`, `interval`, `array`, `range`, `composite`, `vector`, etc. — call the dialect's helper namespace explicitly: `Pg.jsonb({...})`, `MySql.json({...})`, `Ignite.uuid("…")`, `Pg.numeric("12.345")`, etc. The Postgres helper namespace is the largest (Postgres has the richest type system); MySQL covers JSON / DECIMAL / YEAR / SET / BIT / temporal variants; Ignite covers UUID / DECIMAL / TIMESTAMP-with-sub-ms-nanos / CHAR / BYTE-ARRAY plus all integral overrides. See the per-module JSDoc and unit tests under `test/{postgres,mysql,ignite}.test.ts` for the full list.

### Opt-in temporal decoding

By default rows decode temporal columns to the host's raw struct (`{date,time}` for Postgres/MySQL, `bigint` for Ignite `db-date`, `[bigint, number]` for Ignite `db-timestamp`) so no precision is lost. Set `decodeTemporal: "date"` in the client config to opt in to JS `Date` decoding (UTC). For Postgres and MySQL this affects `timestamp` / `timestamptz` / `datetime` / `date`; for Ignite it affects `db-date` and `db-timestamp`. `time` / `timetz` columns always stay raw because JS `Date` cannot represent them faithfully.

### Transactions, locks, streaming

All three adapters use the same two-lock pattern as the SQLite adapter: a client-wide semaphore (1 permit) guards the underlying `DbConnection`, plus a per-`DbTransaction` semaphore for fibers inside a `withTransaction` body. `executeStream` opens a `DbResultStream`, holds the per-connection lock for the stream's lifetime via `Stream.unwrap` over a scoped effect, and releases the permit on completion AND on early interruption.

`withTransaction` is wired through `Client.makeWithTransaction(...)` and supports nesting via savepoints on Postgres and MySQL (`SAVEPOINT effect_sql_<id>` issued through the same `DbTransaction` resource). **Ignite explicitly fails nested `withTransaction`** with a `SqlSyntaxError` because Apache Ignite has no savepoint primitive — there is no way to make nested transactions safe.

### Errors

All adapters classify the host's tagged `Error` variants into the standard `effect/unstable/sql/SqlError` hierarchy:

- `connection-failure` → `ConnectionError`
- `query-parameter-failure` / `query-execution-failure` / `query-response-failure` → `SqlSyntaxError`
- `other` → `UnknownError`
- Param-encoding failures (NaN, out-of-range bigint, malformed UUID, unsupported JS type) → `SqlSyntaxError`
- Authentication-looking thrown `Error` instances (regex on `authent|password|role|access denied`) → `AuthenticationError`

The shared classification logic lives in `src/RdbmsShared.ts` (`sqlErrorFor`, `extractTaggedError`, `ParamEncodingError`, `READ_PREFIX_RE`, `RETURNING_RE`); each adapter wires its own dialect-specific `isReader`, `Pg|MySql|Ignite` helper namespace, and `DbValue` codec on top.

### Integration testing

`integration-test/test-infra/compose.yaml` brings up Postgres + MySQL containers with healthchecks. Apache Ignite is intentionally omitted — the host binding may be missing in some Golem environments and the IgniteCounter component is shipped as a separately-deployable artifact under `integration-test/components/ignite-agent/`. After running `npm run infra:up` and `golem -L build && golem -L -Y deploy`, drive each agent through the standard invocation matrix + snapshot drill (see `integration-test/test-infra/run-rdbms-tests.mjs`).

## Quotas / resource reservations (`Quota.*`)

`effect-golem` ships an Effect-idiomatic façade over `golem:quota/types@1.5.0`, mirroring the `quota` surface of the official `golem-ts-sdk` / `golem-rust-sdk`. Authoring uses the `Quota` namespace re-exported from the package barrel:

```ts
import { Quota } from "effect-golem"
import { Effect } from "effect"

const useApi = Effect.gen(function* () {
  const token = yield* Quota.acquireQuotaToken("api-calls", 1n)

  // RAII-style — body returns { used, value }; on success commit `used`,
  // on failure / interrupt commit 0 via the surrounding scope finalizer:
  const response = yield* Quota.withReservation(token, 4000n, (_r) =>
    Effect.gen(function* () {
      const r = yield* callLlm(prompt, { maxTokens: 4000 })
      return { used: BigInt(r.tokensUsed), value: r }
    }),
  )

  // Manual reserve + commit (must run inside `Effect.scoped` so the
  // reservation's drop-≡-commit(0) finalizer fires deterministically):
  yield* Effect.scoped(
    Effect.gen(function* () {
      const reservation = yield* Quota.reserve(token, 100n)
      const result = yield* doWork()
      yield* Quota.commit(reservation, BigInt(result.actualUsage))
    }),
  )

  // Token combinators — split a child token to pass to an RPC peer,
  // merge it back when the peer returns:
  const child = yield* Quota.split(token, 300n)
  // ...send `child` over RPC (auto-encoded via the QuotaToken schema codec)...
  yield* Quota.merge(token, child)
})
```

API:

- `Quota.acquireQuotaToken(name, expectedUse): Effect<QuotaToken, QuotaHostError>` — manifests-declared resource name + per-reservation hint.
- `Quota.reserve(token, amount): Effect<Reservation, FailedReservationError | QuotaHostError, Scope>` — scope-bound; drop-without-commit ≡ host `commit(0)` (per the WIT contract). Failure with `FailedReservationError.estimatedWaitNanos` is the typed `reject`-policy outcome; `throttle` / `terminate` policies are handled inside the host before `reserve` returns.
- `Quota.commit(reservation, used): Effect<void, QuotaHostError>` — explicit commit. Calling twice fails with `QuotaHostError(commit, ...)`; the surrounding scope finalizer becomes a no-op.
- `Quota.withReservation(token, amount, body): Effect<A, E | FailedReservationError | QuotaHostError, R>` — RAII helper. `body` returns `Effect<{ used: bigint; value: A }, E, R>`; on success the wrapper commits `used`, on failure / defect / interrupt it falls back to `commit(0)`.
- `Quota.split(token, childExpectedUse): Effect<QuotaToken, QuotaHostError>` — split a child token off `token`. Host TRAPS surface as `QuotaHostError` (the WIT contract is not `Result`-returning here).
- `Quota.merge(token, other): Effect<void, QuotaHostError>` — merge `other` back. Host TRAPS surface as `QuotaHostError`. After a successful merge, `other` is consumed.

Schema codec for sending tokens across RPC stays unchanged: `QuotaToken` (the schema codec value, also `Quota.QuotaToken`) round-trips through `QuotaToken.toRecord()` / `fromRecord()`. The TypeScript value `QuotaToken` is the codec; the TypeScript type `QuotaToken` is the host-class instance handle returned by `acquireQuotaToken` — these coexist by name in the value/type namespaces.

Errors (exported from the package barrel and from `Quota.*`):

- `FailedReservationError` — typed domain failure for `reject`-policy reservations. `.estimatedWaitNanos` is `bigint | undefined` (only present for rate-limited resources). `toJSON()` is overridden to render the bigint as a string so a `Cause` containing this error is JSON-safe.
- `QuotaHostError` — anything else thrown from `golem:quota/types@1.5.0` (split overflow, merge resource mismatch, host invariant violations, etc.). `.operation` records the originating host call.

## KeyValue store (`KeyValue.*`)

`effect-golem` ships an Effect-typed wrapper around the eventually-consistent subset of `wasi:keyvalue@0.1.0` (`eventual` + `eventual-batch`). The `atomic` (`increment` / `compare-and-swap`) and `cache` interfaces are intentionally NOT wrapped — they are currently `unimplemented!` in the Golem host and would trap the worker on call. They will be added when the host gains support.

```ts
import { defineAgent, KeyValue, method, Schema } from "effect-golem"
import { Effect } from "effect"

const User = Schema.Struct({ id: Schema.String, name: Schema.String })

defineAgent({
  name: "Users",
  constructorParams: { name: Schema.String },
  methods: {
    put: method({
      params: { id: Schema.String, name: Schema.String },
      success: Schema.Void,
    }),
    get: method({ params: { id: Schema.String }, success: Schema.Option(User) }),
  },
  impl: ({ name }) =>
    Effect.gen(function* () {
      const bucket = yield* KeyValue.openBucket(name) // scoped resource
      const users = bucket.forSchema(User)
      return {
        put: ({ id, name }) => users.set(id, { id, name }),
        get: ({ id }) => users.get(id),
      }
    }),
})
```

API (re-exported from the package barrel as `KeyValue`):

- `KeyValue.openBucket(name): Effect<Bucket, KeyValueHostError, Scope>` — scoped acquire. The WIT bucket resource has no `.close()`; releasing the scope drops the JS handle and lets GC reclaim it.
- `Bucket` — `get` / `set` / `delete` / `exists` (single-key, `wasi:keyvalue/eventual`), `getMany` / `setMany` / `deleteMany` / `keys` (`wasi:keyvalue/eventual-batch`), plus `forSchema(schema)` for a typed view. Single-key get returns `Option.Option<Uint8Array>`; batch get returns `ReadonlyArray<Option.Option<Uint8Array>>` with positional alignment to the request keys (host contract).
- `Bucket.forSchema(schema)` returns `SchemaBucket<S>` — same shape, but values are JSON-encoded via `Schema.fromJsonString(schema)` (UTF-8 bytes ↔ JSON string ↔ schema). Decode failures surface as typed `Schema.SchemaError` (parse + validation) or `KeyValueDecodeError` (invalid UTF-8 only).

Errors (exported from the package barrel):

- `KeyValueHostError` — any host-side trap from `wasi:keyvalue/*`. `.operation` records the originating host call (`eventual.get`, `eventual-batch.set-many`, `openBucket`, etc.); `.trace` is the host's driver-supplied opaque error trace (Redis / SQLite / Postgres / in-memory all produce different formats — do not parse).
- `KeyValueDecodeError` — only raised by `SchemaBucket` when the stored bytes are not valid UTF-8. JSON syntax + schema validation failures bubble out as `Schema.SchemaError`.

Identity: `KeyValue.BucketTypeId` is a `Symbol.for(...)`-keyed stamp; `KeyValue.isBucket(u)` reliably checks cross-bundle.

Caveats:

- Batch operations are all-or-nothing at the host level: a storage-layer error fails the whole call, not individual entries.
- Method authors who use `forSchema` MUST handle the `Schema.SchemaError` typed channel (e.g. via `Effect.catchTag`) — undeclared typed errors bubble up to the dispatcher and become host traps. The SDK does not silently swallow schema mismatches.

The integration test suite ships a `KvAgent` (`integration-test/components/agents/src/kv-agent.ts`) that exercises the full surface — bytes round-trips, schema-typed values, batch ops, and key listing — against the live Golem `wasi:keyvalue` backend.

## Blob store (`Blobstore.*`)

`effect-golem` ships an Effect-typed wrapper around `wasi:blobstore/{blobstore,container,types}`. Container CRUD, object I/O (sync `Uint8Array`), object listing as a `Stream`, and a `forSchema(schema)` typed view per container are exposed. Object writes chunk into 4096-byte segments via `wasi:io/streams.blocking-write-and-flush`.

```ts
import { Blobstore, defineAgent, method, Schema } from "effect-golem"
import { Effect, Stream } from "effect"

const Photo = Schema.Struct({ filename: Schema.String, takenAtMillis: Schema.Number })

defineAgent({
  name: "Photos",
  constructorParams: { name: Schema.String },
  methods: {
    upload: method({
      params: { key: Schema.String, body: Schema.Uint8Array },
      success: Schema.Void,
    }),
    list: method({ params: {}, success: Schema.Array(Schema.String) }),
    putMeta: method({
      params: { key: Schema.String, filename: Schema.String, takenAtMillis: Schema.Number },
      success: Schema.Void,
    }),
  },
  impl: ({ name }) =>
    Effect.gen(function* () {
      const photos = yield* Blobstore.getOrCreateContainer(name) // scoped
      const meta = photos.forSchema(Photo)
      return {
        upload: ({ key, body }) => photos.writeData(key, body),
        list: () => Stream.runCollect(photos.listObjects).pipe(Effect.map((c) => c.slice())),
        putMeta: ({ key, filename, takenAtMillis }) =>
          meta.writeData(key, { filename, takenAtMillis }),
      }
    }),
})
```

API (re-exported from the package barrel as `Blobstore`):

- `Blobstore.createContainer(name): Effect<Container, BlobstoreHostError, Scope>` — fresh container; fails if the name exists.
- `Blobstore.getContainer(name): Effect<Container, BlobstoreHostError, Scope>` — open existing; fails if absent.
- `Blobstore.getOrCreateContainer(name): Effect<Container, BlobstoreHostError, Scope>` — idempotent (safe to call from `impl` after a snapshot load).
- `Blobstore.containerExists(name) / deleteContainer(name) / copyObject(src, dest) / moveObject(src, dest)` — top-level helpers.
- `Container` — `info`, `clear`, `getData(name)` / `getData(name, range)`, `writeData(name, bytes)`, `hasObject` / `objectInfo` / `deleteObject` / `deleteObjects`, `listObjects: Stream.Stream<string, BlobstoreHostError>`, plus `forSchema(schema)` for a typed view.

Identity: `Blobstore.ContainerTypeId` is a `Symbol.for(...)`-keyed stamp; `Blobstore.isContainer(u)` reliably checks cross-bundle.

Errors:

- `BlobstoreHostError` — any host-side trap from `wasi:blobstore/*`. `.operation` records the originating host call; `.trace` is the host's `string` error verbatim (the WIT error type is `string`, unlike keyvalue's resource).
- `BlobstoreDecodeError` — only raised by `SchemaContainer` when the stored bytes are not valid UTF-8. JSON syntax + schema validation failures bubble out as `Schema.SchemaError`.

Caveats (host-side, NOT fixable in the SDK):

- **Backend divergence on `getData(name, range)`.** The WIT spec says `start..=end` is inclusive on both ends. Golem's in-memory + filesystem backends implement it as Rust-exclusive (`start..end`); the S3 backend follows the WIT spec (inclusive). Whole-object reads (`getData(name)` without a range) work around this by first trying inclusive end (`size - 1`) and replaying with `end = size` if the backend short-changed by one byte — so whole-object reads are portable. **Explicit ranged reads are not portable** until the host fixes the in-mem/fs implementations.
- **`created-at` is actually `last-modified-at`.** The WIT field name is misleading: object storage backends (S3 `LastModified`, filesystem `mtime`) have no separate creation timestamp, and the Golem host populates `created-at` from `last-modified-at`. Both `ContainerMetadata.createdAt` and `ObjectMetadata.createdAt` reflect last-modified time.
- **`container.clear()` on the filesystem backend.** Calling `clear()` on a filesystem-backed container deletes the underlying directory; subsequent `listObjects()` calls trap with "Backend error: No such file or directory". Upstream bug — workaround is to `deleteContainer + createContainer` instead. Not surfaced by the in-memory or S3 backends.
- `listObjects` is eager: the host fetches the full object name list at the moment `list-objects` is called and pins it to the oplog. The returned `Stream` only consumes from that in-memory snapshot. Order is undefined (host pops from the tail of its internal `Vec`). Page size: 256.
- Method authors who use `forSchema` MUST handle the `Schema.SchemaError` typed channel — same caveat as `KeyValue.SchemaBucket`.

The integration test suite ships a `BlobAgent` (`integration-test/components/agents/src/blob-agent.ts`) that exercises the full surface — small/large writes (10 000-byte payload to verify the > 4096 chunking), schema-typed objects, listing, deletion, and the exclusive-end recovery path — against the live Golem `wasi:blobstore` backend.
