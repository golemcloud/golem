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

The shared classification logic lives in `src/rdbms-shared.ts` (`sqlErrorFor`, `extractTaggedError`, `ParamEncodingError`, `READ_PREFIX_RE`, `RETURNING_RE`); each adapter wires its own dialect-specific `isReader`, `Pg|MySql|Ignite` helper namespace, and `DbValue` codec on top.

### Integration testing

`integration-test/test-infra/compose.yaml` brings up Postgres + MySQL containers with healthchecks. Apache Ignite is intentionally omitted — the host binding may be missing in some Golem environments and the IgniteCounter component is shipped as a separately-deployable artifact under `integration-test/components/ignite-agent/`. After running `npm run infra:up` and `golem -L build && golem -L -Y deploy`, drive each agent through the standard invocation matrix + snapshot drill (see `integration-test/test-infra/run-rdbms-tests.mjs`).
