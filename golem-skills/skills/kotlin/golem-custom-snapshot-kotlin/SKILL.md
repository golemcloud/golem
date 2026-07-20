---
name: golem-custom-snapshot-kotlin
description: "Enabling snapshot-based recovery and typed state snapshotting for Kotlin agents via the Snapshotted<S> mixin. Use when adding manual (snapshot-based) update support, or — equally importantly — when a long-running agent's oplog is growing large and recovery/replay is becoming slow (heartbeats, polling loops, recurring tasks, frequent state changes). Snapshotting compacts the oplog and lets recovery start from the latest snapshot instead of replaying full history. The Kotlin SDK is typed-only: there is no raw-bytes hook path; the codec is derived from your state type at compile time."
---

# Snapshots in Kotlin

Golem agents can implement snapshotting to support manual (snapshot-based) updates and snapshot-based recovery. The Kotlin SDK provides a single, typed approach: the `Snapshotted<S>` mixin. There is **no raw-bytes hook path** (unlike the Scala/Rust SDKs) — the byte codec for your state is derived by KSP at compile time.

## When to Use Snapshotting

Snapshotting solves two distinct problems:

1. **Manual / snapshot-based component updates** — required when updating agents between incompatible component versions. The host calls the generated `save-snapshot` on the old component revision and `load-snapshot` on the new one.
2. **Fast recovery and oplog compaction** — for long-running agents whose oplog grows over time (heartbeats, polling loops, recurring tasks, agents with frequent state changes). Without snapshotting, every recovery replays the full oplog from the beginning, which becomes increasingly expensive. With periodic snapshotting (`every(N)` or `periodic(...)`), recovery starts from the latest snapshot and replays only the entries after it.

> **You cannot opt out of oplog writes for a durable agent.** If you are worried about oplog volume or replay cost, do *not* try to skip persistence — enable snapshot-based recovery here instead.

## Enabling Snapshotting

Two things are required, and they are independent:

1. **Mix in `Snapshotted<S>`** — this provides the *state* that is saved and restored.
2. **Advertise a cadence** via `@Agent(snapshotting = ...)` — this tells the host *when* to snapshot.

The `save-snapshot`/`load-snapshot` guest exports are always generated, but a snapshot is only produced when a cadence is set.

### Snapshotting Modes

The `snapshotting` parameter accepts these values:

| Mode | Description |
|------|-------------|
| `"disabled"` | No snapshotting (default when omitted) |
| `"enabled"` | Enable snapshot support with the server's default policy. **The server default is `disabled`**, so this may have no effect. Use `"every(N)"` or `"periodic(…)"` to guarantee snapshotting is active. |
| `"every(N)"` | Snapshot every N successful function calls (use `"every(1)"` for every invocation). N must fit a u16 (`0..65535`). |
| `"periodic(duration)"` | Snapshot at most once per time interval (e.g., `"periodic(30s)"`). |

## Typed Snapshotting with `Snapshotted<S>`

Bundle all mutable state into a WIT-mappable type (typically a `data class`), then mix `Snapshotted<S>` into your agent alongside `BaseAgent`:

**1. Define the state type** (must be WIT-mappable — a `data class`, `List`/`Map`/`Pair`/`Triple`, enum, sealed class, primitive, `Datetime`, or `Either`):

```kotlin
data class CounterState(val value: Int)
```

**2. Mix in `Snapshotted<S>` and enable a cadence:**

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.Snapshotted
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint

@Agent(mount = "/counters/{name}", snapshotting = "every(1)")
class CounterAgent(val name: String) : BaseAgent(), Snapshotted<CounterState> {
    override var state = CounterState(0)

    @Endpoint(post = "/increment")
    fun increment(): Int {
        state = CounterState(state.value + 1)
        return state.value
    }

    @Endpoint(get = "/value")
    fun getValue(): Int = state.value
}
```

KSP detects `Snapshotted<S>`, resolves `S`'s schema at compile time, and generates the snapshot codec that serializes/deserializes `state` — no manual serialization code. The state is wrapped in a principal-carrying envelope, so the caller identity captured at `initialize` is restored on load. On a manual update the SDK reconstructs the agent from its own agent-id (the constructor parameters) inside `load-snapshot`, so typed state survives both a revision bump and a worker restart.

### Requirements for `Snapshotted<S>`

- Declare `override var state: S` on the agent.
- `S` must be a WIT-mappable type. A non-mappable `S` is a **compile-time error**, never a silent empty snapshot.
- An agent that does not mix in `Snapshotted` produces an empty snapshot (opt-out no-op).

There is no `stateSchema` field to declare (KSP derives it) and no `saveSnapshot`/`loadSnapshot` methods to write (there is no custom-hook path in the Kotlin SDK).

## Scaffolding

```bash
golem new --template kotlin/snapshotting --component-name example:counter --yes app
```

This scaffolds a ready-to-build agent using `Snapshotted<CounterState>` with `@Agent(snapshotting = "every(1)")`.

## Best Practices

1. **Keep state in one type** — bundle all mutable state into a single `var state: S` (a `data class`) for clean persistence.
2. **Keep snapshots small** — large snapshots impact recovery and update time.
3. **Test round-trips** — verify that save → load produces equivalent state (mutate state, run a manual update, restart, assert the value survived).
4. **Handle migration** — when the state type changes between versions, ensure the new `S` can decode snapshots produced by the old one (or accept a reset). The snapshot bytes are Kotlin-native; there is no cross-language snapshot migration.
