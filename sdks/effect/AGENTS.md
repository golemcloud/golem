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

`npm install` · `npm run build` (tsc + types entry) · `npm run build:bundle` (rollup → `dist/index.mjs` + `dist/effect.mjs`) · `WASI_SDK_PATH=/opt/wasi-sdk npm run build-agent-template` (bundle → wasm-rquickjs → cargo `wasm32-wasip2` → `wasm/agent_guest.wasm`, ~2 min) · `npm run typecheck` · `npm run lint` · `npm run format[:check]` · `npm test` · single test: `npx vitest run test/agent.test.ts -t "registers the Counter"`. Integration: `cd integration-test && npm install && golem -L build && golem -L -Y deploy && golem -L agent invoke -n 'Counter({ name: "x" })' increment`.

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
