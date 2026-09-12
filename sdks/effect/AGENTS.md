# effect-golem contributor guide

`@golemcloud/effect-golem` is the Effect 4 SDK for Golem 1.6. It compiles TypeScript into
QuickJS-backed WASI Preview 3 components.

## Current public model

- Agent metadata uses `id`, methods use `input`, and `defineAgent` returns a spec with `.client` and
  `.implement(...)`. Do not reintroduce `constructorParams`, `params`, or standalone old clients.
- Durable agents expose `client.get`, `getPhantom`, and `newPhantom`; ephemeral agents expose
  `newPhantom`. RPC call/trigger/schedule input is one object. Awaited calls are fiber-interruptible.
- Config secrets are `Schema.Redacted` opaque handles. They are uncached, excluded from overrides,
  and never snapshotted.
- Snapshotting metadata is `snapshotting`. A snapshotted definition requires distinct initialize
  and restore factories. Restore receives `SnapshotRestorationContext`; after factory construction,
  auto state and attached SQLite images are applied.
- Agent streams are affine, single-reader P3 streams. Transfer consumes ownership; interruption and
  early return close local endpoints. Effect-backed producers interrupt and join their fibers;
  arbitrary JavaScript producers must cooperate with iterator cancellation.
- `Tool` contains definition, guest, and typed client APIs. Standalone middleware is imported from
  `@golemcloud/effect-golem/middleware`; it supports universal and typed middleware.
- Permission cards are opaque affine schema capabilities. Successful encoding transfers them;
  transactional encoding must leave them retryable when preparation fails.
- Durability uses `golem:durability/durability@1.6.0`. Successful/typed-failure wrappers finish the
  live resource exactly once; defects, interruption, and encode failures drop it unfinished.
- Do not add Golem 1.5 compatibility shims or claims. Complete cross-language acceptance is still a
  final parity gate. Runtime discovery is in `Reflection`, immutable schema views in `SchemaRef`,
  and value-only identity binding in `DynamicClient`. Narrow reflected `mode` before selecting
  a lifecycle factory; ephemeral clients expose identity only in invocation metadata.

The CLI currently rejects middleware attachment pending GOL-39. Test and document SDK world
support, but do not claim middleware can be attached/deployed through current manifests.

## Layout and public modules

- `src/`: PascalCase public modules. `src/index.ts` exports namespace modules alphabetically.
- `src/internal/`: camelCase implementation modules. Never expose these as package imports.
- `src/host/`: WIT host `Context.Service` wrappers. This is a private injection seam.
- `src/{Sqlite,Postgres,Mysql,Ignite}/`: database adapter sources, published through `/sqlite`,
  `/postgres`, `/mysql`, and `/ignite2`.
- `wit/main.wit`: three worlds: `agent-guest`, `tool-middleware-guest`, and
  `agent-tool-middleware-guest`.
- `golem-types/`: generated ambient declarations for all worlds and host interfaces.
- `scripts/template-matrix.mjs`: source of truth for world/template/declaration/artifact names.
- `{agent,tool-middleware,agent-tool-middleware}-template/`: generated wrapper crates.
- `wasm/`: generated base WASMs for all three worlds.
- `test/`: unit, property, and compile-time tests with host fakes.
- `integration-test/`: real Golem components and harness.

Public code follows Effect package organization: users normally import namespaces (`Snapshot.*`,
`Tool.*`, `Durability.*`). Only `defineAgent`, `defineConfig`, and `method` are flat DSL aliases.
Every public export needs JSDoc with `@since` and `@category`. Package exports block `internal/*`
and `host/*`.

Preserve the existing Effect-native integrations when changing the core model: HTTP metadata,
webhooks, WebSockets, sagas, SQLite, Postgres/MySQL/Ignite SQL, key-value/blob storage, quota, retry,
oplog, logging, and tracing.

## Host injection seam

All WIT access goes through a service in `src/host`. `HostLive.ts` merges live services once and the
agent dispatcher provides that layer to initialization, restoration, invocation, and snapshot
effects. User-facing combinators must let host tags flow in their `R` channel; never provide a live
host layer inside a combinator or add mutable `__set*` production hooks. Tests replace services with
`Layer.succeed`, `Layer.scoped`, or shared fakes.

When adding a host wrapper:

1. Add a focused `XxxClient.ts`/`XxxHost.ts` service and live layer under `src/host`.
2. Merge the live layer in `HostLive.ts`.
3. Consume it with `yield* Xxx`; let the requirement reach the dispatcher.
4. Test with an alternate Layer.

## WIT and generated artifacts

Repository-root `wit/deps` is canonical. Never manually edit `sdks/effect/wit/deps` or generated
ambient declarations. From the repository root:

```nu
cargo make wit
cd sdks/effect
npm run generate-dts
npm run check:dts
npm run check:contracts
npm run check:artifacts
```

`generate-dts` runs `wasm-rquickjs generate-dts` for every entry in the template matrix and merges
shared host declarations while retaining one exports declaration per world. `check:dts` detects
drift. `check:artifacts` checks the provenance manifest for all bundles, wrappers, and WASMs.

WIT-derived wrapper behavior should be tied to declarations through one useful mechanism: a local
`satisfies Record<TagUnion, ...>` value witness, an exhaustive switch over the WIT union, or a
schema/WIT equality assertion in `test/wit-drift.ts`. Do not add inert null-valued shape snapshots.

## Commands

From `sdks/effect`:

```nu
npm ci
npm run lint
npm run format:check
npm run typecheck
npm test
npm run build
npm run build:bundle
$env.WASI_SDK_PATH = "/opt/wasi-sdk"
npm run build-agent-template
npm run check:dts
npm run check:contracts
npm run check:artifacts
```

`build-agent-template` builds all three worlds. A changed runtime bundle, WIT contract, generated
wrapper, or template input requires rebuilding the affected artifacts; verify the complete matrix,
not only `agent_guest.wasm`.

For one unit test:

```nu
npx vitest run test/agent.test.ts -t "registers the Counter"
```

Unit tests only exercise mocks. After non-trivial runtime, WIT, stream, capability, snapshot,
durability, tool, or middleware changes, build fresh bundles/templates and run the smallest relevant
real-runtime integration case. Broaden to the complete Effect harness when shared dispatch, codec,
or artifact behavior changes. Middleware deployment cannot be an acceptance requirement until the
CLI supports attachment.

## Package and release

The committed manifest version is `0.0.0`, matching the root TypeScript SDK convention. A root
workflow publishes trusted/provenance npm releases from `golem-effect-v<semver>` tags and derives
the npm version from the tag. Never add `sdks/effect/.github`.

The release gate performs a clean install, lint/format/typecheck/tests, all-world package and
template builds, `check:dts`, `check:contracts`, `check:artifacts`, and `check:package`. The latter
must create an npm tarball and smoke-test all public imports, declarations, and the three WASM
artifacts from a clean directory without checkout-relative files. Dry runs validate but do not
publish; publishing is separately authorized.

## Style

- Strict ESM TypeScript; relative imports end in `.js`.
- Two-space indentation, no semicolons, double quotes, trailing commas; use Prettier.
- Public files/modules are PascalCase; internal implementation files are camelCase.
- Prefer typed Effect failures to throws at public boundaries.
- Do not log or expose secrets or opaque capability handles.
- Keep user examples Effect-native rather than Promise-oriented.
- Inline cargo-make scripts are duckscript, never shell.
