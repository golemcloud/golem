---
name: golem-custom-snapshot-ts
description: "Implementing custom snapshot save/load functions for TypeScript agents. Use when adding manual update support, snapshot-based recovery, or custom state serialization for a TypeScript Golem agent."
---

# Custom Snapshots in TypeScript

Golem agents can override `saveSnapshot` and `loadSnapshot` on `BaseAgent` to support manual (snapshot-based) updates and snapshot-based recovery.

## Enabling Snapshotting

Snapshotting is configured via the `snapshotting` option in the `@agent` decorator. Without it, no periodic snapshots are taken (but save/load are still available for manual updates):

```typescript
@agent({ mount: "/counters/{name}", snapshotting: { every: 1 } })
class CounterAgent extends BaseAgent { ... }
```

### Snapshotting Modes

The `snapshotting` option accepts these values:

| Mode | Example | Description |
|------|---------|-------------|
| `'disabled'` | (default when omitted) | No periodic snapshotting |
| `'enabled'` | `snapshotting: 'enabled'` | Enable snapshot support with the server's default policy. **The server default is `disabled`**, so this may have no effect. Use `{ every: N }` or `{ periodic: '…' }` to guarantee snapshotting is active. |
| `{ every: number }` | `snapshotting: { every: 1 }` | Snapshot every N successful function calls (use `{ every: 1 }` for every invocation) |
| `{ periodic: string }` | `snapshotting: { periodic: '30s' }` | Snapshot at most once per time interval |

```typescript
@agent({ mount: "/periodic/{name}", snapshotting: { periodic: '30s' } })
class PeriodicAgent extends BaseAgent { ... }

@agent({ mount: "/batch/{name}", snapshotting: { every: 10 } })
class BatchAgent extends BaseAgent { ... }
```

## Automatic Snapshotting (Default)

By default, `BaseAgent` provides automatic snapshotting that:
1. JSON-serializes all own properties of the agent (excluding `cachedAgentType`, `agentClassName`, functions, and internal types).
2. Automatically detects and serializes any `DatabaseSync` fields as binary SQLite snapshots.
3. When databases are present, uses a `multipart/mixed` format to bundle JSON state with binary database blobs.

No custom code is needed if the agent's state is JSON-serializable.

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';

@agent({ mount: "/counters/{name}", snapshotting: { every: 1 } })
class CounterAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    @endpoint({ post: "/increment" })
    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }
    // No saveSnapshot/loadSnapshot needed — automatic JSON snapshotting works
}
```

## Custom Snapshotting

Override both `saveSnapshot()` and `loadSnapshot()` to implement a custom binary format:

```typescript
import { BaseAgent, agent, prompt, description, endpoint } from '@golemcloud/golem-ts-sdk';

@agent({ mount: "/snapshot-counters/{name}", snapshotting: { every: 1 } })
class CounterWithSnapshotAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    @prompt("Increase the count by one")
    @description("Increases the count by one and returns the new value")
    @endpoint({ post: "/increment" })
    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }

    override async saveSnapshot(): Promise<Uint8Array> {
        const snapshot = new Uint8Array(4);
        const view = new DataView(snapshot.buffer);
        view.setUint32(0, this.value);
        console.info(`Saved snapshot: ${this.value}`);
        return snapshot;
    }

    override async loadSnapshot(bytes: Uint8Array): Promise<void> {
        const view = new DataView(bytes.buffer);
        this.value = view.getUint32(0);
        console.info(`Loaded snapshot!: ${this.value}`);
    }
}
```

## Method Signatures

```typescript
// Save: serialize the agent's state. Can return raw bytes or bytes with a MIME type.
async saveSnapshot(): Promise<Uint8Array | { data: Uint8Array; mimeType: string }>

// Load: restore the agent's state from previously saved snapshot bytes.
// The mimeType parameter indicates the format of the saved snapshot.
async loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void>
```

### Return Type Options for `saveSnapshot`

- **`Uint8Array`** — treated as `application/octet-stream`
- **`{ data: Uint8Array; mimeType: string }`** — explicit MIME type, use `application/json` for JSON or `application/octet-stream` for binary

### Error Handling

- `loadSnapshot` can **throw a string** to signal that the update should fail and the agent should revert to the old version.

## Working with SQLite Databases

If the agent uses `DatabaseSync` fields, the default `saveSnapshot` and `loadSnapshot` implementations automatically handle them via a `multipart/mixed` format. For custom overrides that still need database support, use the protected helper:

```typescript
// Inside a custom saveSnapshot override:
const databases = this.serializeTrackedDatabases();
// Returns Array<{ name: string; bytes: Uint8Array }>
```

## Best Practices

1. **Prefer automatic (JSON) snapshotting** unless you need a compact binary format or cross-version migration logic.
2. **Keep snapshots small** — large snapshots impact recovery and update time.
3. **Version your snapshot format** — include a version byte or marker so `loadSnapshot` can handle snapshots from older versions.
4. **Test round-trips** — verify that `saveSnapshot` → `loadSnapshot` produces equivalent state.
5. **Handle migration** — when the state schema changes between versions, `loadSnapshot` in the new version should be able to parse snapshots from the old version.
6. **Override both or neither** — always override `saveSnapshot` and `loadSnapshot` together to keep serialization consistent.

## Project Template

A ready-made project with snapshotting can be created using:

```shell
golem new --language ts --template snapshotting my-project
```
