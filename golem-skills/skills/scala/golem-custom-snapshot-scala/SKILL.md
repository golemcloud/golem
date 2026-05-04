---
name: golem-custom-snapshot-scala
description: "Implementing custom snapshot save/load functions for Scala agents. Use when adding manual update support, snapshot-based recovery, or custom state serialization for a Scala Golem agent."
---

# Custom Snapshots in Scala

Golem agents can implement snapshotting to support manual (snapshot-based) updates and snapshot-based recovery. The Scala SDK provides two approaches: automatic JSON-based snapshotting via `Snapshotted[S]` and custom binary hooks.

## Enabling Snapshotting

Snapshotting must be enabled in the `@agentDefinition` annotation. Without it, no snapshot exports are generated:

```scala
@agentDefinition(snapshotting = "every(1)")
trait MyAgent extends BaseAgent {
  class Id(val value: String)
  def doSomething(): Future[String]
}
```

### Snapshotting Modes

The `snapshotting` parameter accepts these values:

| Mode | Description |
|------|-------------|
| `"disabled"` | No snapshotting (default when omitted) |
| `"enabled"` | Enable snapshot support with the server's default policy. **The server default is `disabled`**, so this may have no effect. Use `"every(N)"` or `"periodic(…)"` to guarantee snapshotting is active. |
| `"every(N)"` | Snapshot every N successful function calls (use `"every(1)"` for every invocation) |
| `"periodic(duration)"` | Snapshot at most once per time interval (e.g., `"periodic(30s)"`) |

```scala
@agentDefinition(snapshotting = "periodic(30s)")
trait PeriodicAgent extends BaseAgent { ... }

@agentDefinition(snapshotting = "every(10)")
trait BatchAgent extends BaseAgent { ... }
```

## Automatic JSON Snapshotting with `Snapshotted[S]`

The recommended approach. Bundle all mutable state into a case class with a `Schema` instance, then mix `Snapshotted[S]` into the implementation class:

**1. Define the state type:**

```scala
final case class CounterState(value: Int)
object CounterState {
  implicit val schema: Schema[CounterState] = Schema.derived
}
```

**2. Enable snapshotting on the agent definition:**

```scala
@agentDefinition(snapshotting = "every(1)")
@description("A counter with automatic JSON-based state persistence.")
trait AutoSnapshotCounter extends BaseAgent {
  class Id(val value: String)
  def increment(): Future[Int]
}
```

**3. Mix in `Snapshotted[S]` on the implementation:**

```scala
@agentImplementation()
final class AutoSnapshotCounterImpl(private val name: String)
    extends AutoSnapshotCounter
    with Snapshotted[CounterState] {

  var state: CounterState = CounterState(0)
  val stateSchema: Schema[CounterState] = Schema.derived

  override def increment(): Future[Int] =
    Future.successful {
      state = state.copy(value = state.value + 1)
      state.value
    }
}
```

The macro detects `Snapshotted[S]`, summons `Schema[S]` at compile time, and generates snapshot handlers that serialize/deserialize `state` as JSON using zio-schema. No manual serialization code needed.

### Requirements for `Snapshotted[S]`

- The implementation must have a `var state: S` field.
- The implementation must have a `val stateSchema: Schema[S]` field.
- `S` must be a case class with a `Schema` instance.

## Custom Snapshot Hooks

For custom binary serialization, define `saveSnapshot()` and `loadSnapshot()` convention methods directly on the implementation class:

```scala
@agentDefinition(snapshotting = "every(1)")
trait SnapshotCounter extends BaseAgent {
  class Id(val value: String)
  def increment(): Future[Int]
}

@agentImplementation()
final class SnapshotCounterImpl(private val name: String) extends SnapshotCounter {
  private var value: Int = 0

  def saveSnapshot(): Future[Array[Byte]] =
    Future.successful(encodeU32(value))

  def loadSnapshot(bytes: Array[Byte]): Future[Unit] =
    Future.successful {
      value = decodeU32(bytes)
    }

  override def increment(): Future[Int] =
    Future.successful {
      value += 1
      value
    }

  private def encodeU32(i: Int): Array[Byte] =
    Array(
      ((i >>> 24) & 0xff).toByte,
      ((i >>> 16) & 0xff).toByte,
      ((i >>> 8) & 0xff).toByte,
      (i & 0xff).toByte
    )

  private def decodeU32(bytes: Array[Byte]): Int =
    ((bytes(0) & 0xff) << 24) |
      ((bytes(1) & 0xff) << 16) |
      ((bytes(2) & 0xff) << 8) |
      (bytes(3) & 0xff)
}
```

### Method Signatures

```scala
// Save: serialize the agent's current state to bytes
def saveSnapshot(): Future[Array[Byte]]

// Load: restore the agent's state from previously saved bytes
def loadSnapshot(bytes: Array[Byte]): Future[Unit]
```

The macro detects these convention methods and wires them into the snapshot exports automatically.

## Best Practices

1. **Prefer `Snapshotted[S]`** — automatic JSON serialization via zio-schema is simpler and less error-prone.
2. **Keep state in one case class** — bundle all mutable state into a single `var state: S` for clean persistence.
3. **Keep snapshots small** — large snapshots impact recovery and update time.
4. **Use custom hooks for binary formats** — when you need compact encoding or compatibility with non-Scala components.
5. **Test round-trips** — verify that save → load produces equivalent state.
6. **Handle migration** — when the state schema changes between versions, `loadSnapshot` should handle snapshots from older versions.
