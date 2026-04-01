# Snapshotting

Golem supports snapshotting to persist agent state across restarts. The ZIO Golem SDK provides several ways to
configure snapshotting, from fully automatic JSON-based persistence to custom binary serialization.

## Table of Contents

- [Enabling Snapshotting](#enabling-snapshotting)
- [Snapshotting Modes](#snapshotting-modes)
- [Automatic JSON Snapshotting with Snapshotted\[S\]](#automatic-json-snapshotting-with-snapshotteds)
- [Custom Snapshot Hooks](#custom-snapshot-hooks)
- [Examples in the Codebase](#examples-in-the-codebase)
- [Best Practices](#best-practices)

---

## Enabling Snapshotting

To enable snapshotting for an agent, set the `snapshotting` parameter on `@agentDefinition`:

```scala
@agentDefinition(snapshotting = "enabled")
trait MyAgent extends BaseAgent {
  class Id(val value: String)
  def doSomething(): Future[String]
}
```

Without this annotation parameter, snapshotting is disabled and no snapshot exports are generated.

---

## Snapshotting Modes

The `snapshotting` parameter accepts the following values:

| Mode              | Description                                              |
|-------------------|----------------------------------------------------------|
| `"disabled"`      | No snapshotting (default when omitted)                   |
| `"enabled"`       | Snapshot on every successful function call                |
| `"periodic(10s)"` | Snapshot at most once every 10 seconds (duration format)  |
| `"every(5)"`      | Snapshot every 5 successful function calls               |

Examples:

```scala
@agentDefinition(snapshotting = "enabled")
trait EnabledAgent extends BaseAgent { ... }

@agentDefinition(snapshotting = "periodic(30s)")
trait PeriodicAgent extends BaseAgent { ... }

@agentDefinition(snapshotting = "every(10)")
trait BatchAgent extends BaseAgent { ... }
```

---

## Automatic JSON Snapshotting with `Snapshotted[S]`

The recommended approach for most agents. Bundle all mutable state into a case class with a `Schema` instance,
then mix `Snapshotted[S]` into the implementation class:

**1. Define the state type:**

```scala
final case class CounterState(value: Int)
object CounterState {
  implicit val schema: Schema[CounterState] = Schema.derived
}
```

**2. Enable snapshotting on the agent definition:**

```scala
@agentDefinition(snapshotting = "enabled")
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

The macro detects `Snapshotted[S]` on the implementation class, summons `Schema[S]` at compile time, and generates
snapshot handlers that serialize/deserialize `state` as JSON using zio-schema's `jsonCodec`. No manual serialization
code is needed.

---

## Custom Snapshot Hooks

For agents that need custom binary serialization (e.g., for performance or compatibility), define
`saveSnapshot()` and `loadSnapshot()` convention methods directly on the implementation class:

```scala
@agentDefinition(snapshotting = "enabled")
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

The macro detects these convention methods and wires them into the snapshot exports automatically.

**Method signatures:**

- `def saveSnapshot(): Future[Array[Byte]]` — serialize current state to bytes
- `def loadSnapshot(bytes: Array[Byte]): Future[Unit]` — restore state from bytes

---

## Examples in the Codebase

| Example                    | Approach              | Description                                     |
|----------------------------|-----------------------|-------------------------------------------------|
| `SnapshotCounterImpl`      | Custom hooks          | Manual binary serialization with save/load hooks |
| `AutoSnapshotCounterImpl`  | `Snapshotted[S]`      | Automatic JSON serialization via Schema          |

---

## Best Practices

1. **Prefer `Snapshotted[S]`** — automatic JSON serialization is simpler and less error-prone than custom hooks
2. **Keep state in one case class** — bundle all mutable state into a single `var state: S` for clean persistence
3. **Keep snapshots small** — large snapshots impact component startup time
4. **Use custom hooks for binary formats** — when you need compact encoding or compatibility with non-Scala components
5. **Test round-trips** — verify that save → load produces equivalent state
