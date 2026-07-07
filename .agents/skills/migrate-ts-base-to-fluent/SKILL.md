---
name: migrate-ts-base-to-fluent
description: "Migrating a TypeScript Golem agent from the OLD decorator/base API (@agent + BaseAgent) to the fluent (Standard Schema) defineAgent API. Use when porting an existing decorator-based TS agent, when you see @agent()/extends BaseAgent/@endpoint/@description code that no longer compiles, or when asked to modernize a TS component to the current SDK."
---

# Migrating TypeScript Agents: base/decorator → fluent

The decorator/base surface of `@golemcloud/golem-ts-sdk` (`@agent()` classes extending `BaseAgent`, `@golemcloud/golem-ts-typegen`) has been **removed**. The fluent (Standard Schema) API — `defineAgent({...}).implement({...})` — is now the only `ts` authoring surface. This skill maps every old construct to its fluent replacement.

Verify each API name against the SDK source in `sdks/ts/packages/golem-ts-sdk/src/fluent/` and `src/host/` as you go; the fluent exports are re-exported from the package root (`src/index.ts` → `export * from './fluent'`).

## Mental model shift

| Decorator/base | Fluent |
|----------------|--------|
| `@agent()` class `extends BaseAgent` | `defineAgent({ name, id, methods })` (contract) + `.implement({ init, methods })` (behaviour) |
| `constructor(name) { super() }` params | `id` record on `defineAgent`; the values become the constructor parameters |
| private class fields (`this.value = 0`) | state object returned by `init()`; read/write via `this` in handlers |
| method signature types (`increment(): Promise<number>`) | `method({ input: {...}, returns: <schema> })` with Standard Schema |
| `this.getId()` | `this.getId()` (same, on the handler `this`) |
| `getPrincipal()` | `this.getPrincipal()` |

There are **no classes, no decorators, and no code generation**. Handlers are plain functions whose `this` is the state.

## Imports

The decorator exports are gone. Remove them and the typegen package:

```ts
// OLD — delete
import { BaseAgent, agent, prompt, description, endpoint, readonly, Config, Secret, Result } from '@golemcloud/golem-ts-sdk';
// import ... from '@golemcloud/golem-ts-typegen';   // gone entirely

// NEW — import what you use
import { z } from 'zod';                              // or valibot / arktype
import { defineAgent, method, s, http, clientFor, Result } from '@golemcloud/golem-ts-sdk';
```

`Result` still exists (host `Result.ok` / `Result.err`). `Config` and `Secret` as **constructor parameter types** are gone — config is now a `config` record on `defineAgent` and secrets are `s.secret(...)` markers surfaced as `Secret<T>` handles on `this.config`.

## tsconfig

Drop the decorator flags; keep bundler resolution:

```jsonc
{
  "compilerOptions": {
    "moduleResolution": "bundler",   // keep
    // "experimentalDecorators": true,   // REMOVE
    // "emitDecoratorMetadata": true,    // REMOVE
    "strict": true,
    "useDefineForClassFields": false
  }
}
```

## Agent class → defineAgent + implement

OLD:

```ts
@agent({ mount: '/counters/{name}' })
class CounterAgent extends BaseAgent {
  private readonly name: string;
  private value: number = 0;
  constructor(name: string) { super(); this.name = name; }

  @prompt('Increase the count by one')
  @description('Increases the count by one and returns the new value')
  @endpoint({ post: '/increment' })
  async increment(): Promise<number> {
    this.value += 1;
    return this.value;
  }
}
```

NEW:

```ts
import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const Counter = defineAgent({
  name: 'CounterAgent',
  id: { name: z.string() },                       // constructor param → id record
  http: http.mount('/counters/{name}'),           // @agent({ mount }) → http.mount
  methods: {
    increment: method({
      input: {},
      returns: z.number(),
      description: 'Increases the count by one and returns the new value',   // @description
      promptHint: 'Increase the count by one',                              // @prompt
      http: http.post('/increment'),                                        // @endpoint({ post })
    }),
  },
});

export const CounterImpl = Counter.implement({
  init: ({ id }) => ({ name: id.name, value: 0 }),  // class fields → init() state
  methods: {
    increment() { this.value += 1; return this.value; },  // `this` is the state
  },
});
```

Register the agent by importing its module from `src/main.ts` (`import './counter-agent.js';`) — there is no exported class for the runtime to find; `defineAgent`/`.implement` register at module load.

## Annotations → method metadata + HTTP

- `@description('...')` → `method({ description: '...' })`; agent-level `@description` → `defineAgent({ description })`.
- `@prompt('...')` → `method({ promptHint: '...' })`; agent-level → `defineAgent({ promptHint })`.
- `@readonly()` → `method({ readOnly: true })`.
- `@endpoint({ post: '/x' })` → `method({ http: http.post('/x') })`, with a mount declared via `defineAgent({ http: http.mount('/prefix/{idVar}', { cors, auth }) })`.
- `@agent({ mount, cors })` → `defineAgent({ http: http.mount(mount, { cors }) })`.

Verbs: `http.get/head/post/put/del/patch/options/connect/trace`, plus `http.custom(verb, path)`. Query binds via inline `?k={var}`; headers via `{ headers: { 'X-Name': 'param' } }`.

## Result<T,E> return → s.result(ok, err)

OLD `async m(): Promise<Result<T, E>>` returning `Result.ok`/`Result.err` becomes a typed `returns`:

```ts
divide: method({ input: { a: z.number(), b: z.number() }, returns: s.result(z.number(), z.string()) }),
// handler:
divide({ a, b }) { return b === 0 ? Result.err('div by zero') : Result.ok(a / b); }
```

The failure travels as a value inside the success payload (same semantics as the decorator SDK). `ok(...)` / `err(...)` are `Result.ok(...)` / `Result.err(...)`.

## Config<T> / Secret<T> constructor params → config record

OLD passed `Config<AgentConfig>` (with nested `Secret<T>` fields) as a constructor parameter. NEW declares a `config` record on `defineAgent`; wrap any secret field (at any depth) in `s.secret(inner)`:

```ts
export const ConfigAgent = defineAgent({
  name: 'ConfigAgent',
  id: { name: z.string() },
  config: {
    greeting: z.string(),                  // local  → this.config.greeting : string (read fresh)
    apiKey: s.secret(z.string()),          // secret → this.config.apiKey : Secret<string>
    nested: z.object({ a: z.string(), c: s.secret(z.object({ d: z.string() })) }),
  },
  methods: { keyTail: method({ input: {}, returns: z.string() }) },
});

export const ConfigAgentImpl = ConfigAgent.implement({
  init: () => ({}),
  methods: {
    keyTail() { return this.config.apiKey.get().slice(-4); },   // Secret<T>.get() reveals fresh
  },
});
```

Local fields read their decoded value directly off `this.config`; secret fields are lazy log-safe `Secret<T>` handles — call `.get()` at the point of use, never log them. Only object schemas are flattened into nested config fields; unions/arrays/maps are read whole.

## save/loadSnapshot overrides → snapshotting option

OLD overrode `saveSnapshot()` / `loadSnapshot()` on the `BaseAgent` subclass. NEW is declarative on `defineAgent`:

```ts
// Typed, scoped: only the declared fields of `this` are serialized.
snapshotting: { state: z.object({ count: z.number() }), policy: { everyNInvocations: 5 } },
```

Policy: `'disabled'` (default) | `'default'` | `{ everyNInvocations: n }` | `{ periodicSeconds: n }`. A bare policy (no `state`) falls back to reflective JSON serialization of the whole state.

For fully custom bytes, supply a `snapshot` block on `.implement(...)` — `this` is the state:

```ts
Def.implement({
  init: () => ({ /* ... */ }),
  methods: { /* ... */ },
  snapshot: {
    save: () => new Uint8Array(/* serialize this */),
    load: (bytes) => { /* restore this from bytes */ },
  },
});
```

## Known feature deltas (a migrating user MUST know)

The fluent API is intentionally narrower than the decorator API in several places. Do not assume the old option exists:

- **`readOnly` is boolean-only.** The decorator `@readonly({ cache: 'no-cache' | 'until-write' | { ttl } })` cache policies, TTLs, and principal-dependent caching are gone. `readOnly: true` only marks the method side-effect-free.
- **No `getWithConfig` RPC variant.** The old `Agent.getWithConfig(...)` / `getPhantomWithConfig(...)` / `newPhantomWithConfig(...)` (passing config to a remote agent) have no fluent equivalent. Use `clientFor(Def)(id, phantomId?)` — config is provisioned via `golem.yaml`, not passed at call time.
- **Custom snapshot is a bare `Uint8Array`.** No `{ data, mimeType }` return, no `application/json` vs `multipart/mixed` selection, and no automatic SQLite multipart part. `save` returns bytes, `load` takes bytes.
- **No mount-level header→id binding.** The mount only binds path `{var}` names to id fields; there is no header-to-id mapping on the mount.
- **No cancelable scheduling in the fluent client.** The client method has `.trigger(input)` (fire-and-forget) and `.schedule(at, input)` — there is no `scheduleCancelable` / returned `CancellationToken` on the fluent RPC client.
- **`Principal` is read via `this.getPrincipal()`** inside a handler (also `InitContext.principal` in `init`), not a constructor-injected value.

If a decorator feature you need has no fluent equivalent, stop and flag it rather than inventing an API name — verify against `src/fluent/` and `src/host/` first.

## Migration checklist

1. Delete decorator imports and any `@golemcloud/golem-ts-typegen` usage.
2. Remove `experimentalDecorators` / `emitDecoratorMetadata` from `tsconfig.json`.
3. Convert each `@agent` class to `defineAgent({...})` + `.implement({...})`; constructor params → `id`; class fields → `init()` state.
4. Convert method signatures to `method({ input, returns })` with Standard Schema; `@description`/`@prompt`/`@readonly`/`@endpoint` → method options + `http.mount`.
5. Convert `Result<T,E>` returns to `s.result(ok, err)`.
6. Convert `Config`/`Secret` constructor params to a `config` record (`s.secret(...)` for secrets).
7. Convert `save/loadSnapshot` overrides to `snapshotting` (or `.implement({ snapshot })`).
8. Ensure every agent module is imported from `src/main.ts`.
9. `golem build --yes` to verify.

## Archiving / sharing a migration branch off GitHub (`git bundle`)

For private marketing branches or migration work kept off GitHub, use a `git bundle` — a single-file archive of a branch (or range) that can be copied around and cloned like a remote.

```shell
# Create a bundle of just the commits on <branch> since <base> (e.g. main..feature)
git bundle create migrate-ts-fluent.bundle main..migrate-ts-fluent

# Or bundle a whole branch (self-contained, clonable):
git bundle create migrate-ts-fluent.bundle migrate-ts-fluent

# Verify the bundle is intact and lists its prerequisites/refs
git bundle verify migrate-ts-fluent.bundle

# Consume it: clone (or fetch/pull) directly from the file
git clone migrate-ts-fluent.bundle restored-repo
git fetch ./migrate-ts-fluent.bundle migrate-ts-fluent   # into an existing repo
```

A range bundle (`main..branch`) is smaller but requires the recipient already have `main`; a full-branch bundle is self-contained and clonable on its own. Never `git stash` in a shared Golem checkout to stage this work — use a worktree or local commits.
