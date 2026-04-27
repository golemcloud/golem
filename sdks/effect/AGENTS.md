# effect-golem

A TypeScript library for writing [Golem](https://golem.cloud) agents on top of [Effect v4](https://effect.website/) (beta). Agents are declared via `defineAgent` + `method` (Effect Schema for params/return/error). Compiles to a Golem WASM component by injecting bundled user code into a prebuilt QuickJS-backed base WASM (mirrors `golemcloud/golem` `sdks/ts`).

## Layout

- `src/` library (`agent.ts`, `client.ts`, `method.ts`, `wit-codec.ts`, `wit-tree.ts`, `exports.ts`, `index.ts`); `src/effect-bundle.mjs` re-exports `effect` for the standalone runtime bundle.
- `src/http.ts` HTTP routes namespace (`Http.mount` / `Http.endpoint` + verb shorthands, path/segment IR, `validateAgentHttp`, `HttpRouteError`).
- `golem-types/` ambient `.d.ts` for `golem:*` / `wasi:*` / `agent-guest`. Mirror in `test/mocks/` + alias in `vitest.config.ts` if used at runtime in `src/`.
- `wit/main.wit` (world `agent-guest`) + vendored `wit/deps/` (from `golemcloud/golem` `sdks/ts/wit/deps/`).
- `scripts/` Node build helpers (`generate-agent-template.mjs`, `copy-agent-template.mjs`, `build-types-entry.mjs`).
- `agent-template/` (gitignored) generated Rust crate; `wasm/agent_guest.wasm` (gitignored) base WASM artifact.
- `integration-test/` standalone app with `golem.yaml` (custom `effect-golem-ts` componentTemplate), `rollup.config.component.mjs`, components under `components/<name>/src/main.ts`. Single-component agents are co-located so RPC reuses one `defineAgent`.

## Commands

`npm install` · `npm run build` (tsc + types entry) · `npm run build:bundle` (rollup → `dist/index.mjs` + `dist/effect.mjs`) · `WASI_SDK_PATH=/opt/wasi-sdk npm run build-agent-template` (bundle → wasm-rquickjs → cargo `wasm32-wasip2` → `wasm/agent_guest.wasm`, ~2 min) · `npm run typecheck` · `npm run lint` · `npm run format[:check]` · `npm test` · single test: `npx vitest run test/agent.test.ts -t "registers the Counter"`. Integration: `cd integration-test && npm install && golem -L build && golem -L -Y deploy && golem -L agent invoke -n 'Counter("x")' increment`.

## Required end-to-end testing after every change

Unit tests (`npm test`) only cover the SDK in isolation against host mocks. They cannot catch issues that only surface inside the real Golem WASM runtime — e.g. `JSON.stringify` on bigint UUIDs, missing exports from the embedded `effect-golem` bundle, type-mismatch between WIT bindings and our generated host stubs, runtime principal serialization, oplog/snapshot interaction, etc.

After **every** non-trivial change you must run a full local-deploy + invoke loop on a running `golem server run` instance. The minimum drill:

1. `npm test && npm run typecheck && npm run lint && npm run format:check` — gate the SDK changes.
2. `npm run build:bundle` — refresh `dist/index.mjs` (the integration-test consumes `effect-golem` via `file:..` symlink, so this is what the embedded code sees).
3. `WASI_SDK_PATH=/opt/wasi-sdk npm run build-agent-template` — rebuilds `wasm/agent_guest.wasm` so the new `dist/index.mjs` is embedded inside the base WASM. **Required whenever `dist/index.mjs` changes**, otherwise components will load against the old SDK and fail with "Could not find export X in module 'effect-golem'" or stale-behaviour bugs.
4. `cd integration-test && golem -L build && golem -L -Y deploy` — rebuild + deploy the test components.
5. Invoke at least one method per affected feature, e.g. `golem -L agent invoke -n 'Counter("x")' increment`.
6. For snapshotting changes specifically: drive enough invocations to trigger a save (the integration-test Counter uses `everyN(10)`), inspect with `golem -L agent oplog 'Counter("x")'` (look for a `SNAPSHOT` entry containing principal + state JSON, **no** `Exception during awaiting call result for saveSnapshot.save`), then exercise the load path with `golem -L -Y agent update --await 'Counter("x")' manual` and re-`invoke` to confirm state was preserved.
7. If anything fails inside the runtime, treat it as a real bug and fix the SDK — do **not** ship green unit tests + red integration runs.

## Conventions

- Strict TS (`noUnusedLocals`/`Parameters`, `noImplicitReturns`); ESM (`"type": "module"`); imports must end in `.js` (NodeNext); 2-space indent, no semicolons, double quotes, trailing commas (Prettier).
- Public surface re-exported from `src/index.ts`; runtime hooks (`guest`, `saveSnapshot`, `loadSnapshot`) declared in `src/exports.ts` with inlined types (no `import "agent-guest"` in published `.d.ts`).
- Errors as Effect typed failures (e.g. `UnsupportedSchemaError`, `InvalidDataValueError`, `RemoteCallError`); avoid throwing.
- The base WASM externalizes `effect`, `effect-golem`, `agent-guest`, all `golem:*`/`wasi:*`; user component bundles must externalize the same set so all components share one Effect runtime instance.
- When adding deps to user code, prefer importing types/runtime from `effect` and APIs from `effect-golem`.
- HTTP routing metadata is authored through the `Http.*` namespace re-exported from `src/http.ts` (mount on the agent, endpoints on each method).

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

Rules (enforced by `validateAgentHttp` in `src/http.ts`):

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

Schema evolution: snapshots are JSON-encoded under the auto variant; users are responsible for keeping their `schema` backward-compatible (or versioning the payload manually). The SDK does not migrate.

Errors: `InvalidSnapshotError` (from `registerAgent`, e.g. `everyN(0)`), `SnapshotNotBoundError`, `SnapshotAlreadyBoundError`, `SnapshotEnvelopeError`, `UnsupportedSnapshotFormatError`. All exported from the package barrel.
