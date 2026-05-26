---
title: "Golem 1.5 features — Part 6: User-defined snapshotting"
date: "2026-04-15T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-6-user-defined-snapshotting"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part6-user-defined-snapshotting/"
---

## Introduction

This post showcases new features of **Golem 1.5**, releasing end of April 2026. This series assumes familiarity with Golem; refer to [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for background.

Parts released so far:

- [Part 1: Code-first routes](https://blog.vigoo.dev/posts/golem15-part1-code-first-routes)
- [Part 2: Webhooks](https://blog.vigoo.dev/posts/golem15-part2-webhooks)
- [Part 3: MCP](https://blog.vigoo.dev/posts/golem15-part3-mcp)
- [Part 4: Node.js compatibility](https://blog.vigoo.dev/posts/golem15-part4-nodejs)
- [Part 5: Scala support](https://blog.vigoo.dev/posts/golem15-part5-scala)
- [Part 6: User-defined snapshotting](https://blog.vigoo.dev/posts/golem15-part6-user-defined-snapshotting)
- [Part 7: Configuration and Secrets](https://blog.vigoo.dev/posts/golem15-part7-config-and-secrets)
- [Part 8: Template simplifications and automatic updates](https://blog.vigoo.dev/posts/golem15-part8-template-simplifications)
- [Part 9: Agent skills](https://blog.vigoo.dev/posts/golem15-part9-skills)
- [Part 10: WebSocket client](https://blog.vigoo.dev/posts/golem15-part10-websocket)
- [Part 11: Bridge libraries](https://blog.vigoo.dev/posts/golem15-part11-bridges)
- [Part 12: REPL](https://blog.vigoo.dev/posts/golem15-part12-repl)
- [Part 13: Per-agent configuration](https://blog.vigoo.dev/posts/golem15-part13-per-agent-config)
- [Part 14: OpenTelemetry](https://blog.vigoo.dev/posts/golem15-part14-otlp)
- [Part 15: MoonBit](https://blog.vigoo.dev/posts/golem15-part15-moonbit)
- [Part 16: Quotas](https://blog.vigoo.dev/posts/golem15-part16-quotas)
- [Part 17: Semantic retry policies](https://blog.vigoo.dev/posts/golem15-part17-semantic-retry-policies)

## Snapshot based recovery

One of **Golem's** primary features enables transparent recovery of agent state. This works through replaying an **oplog** recording side-effect results, reconstructing application state during recovery. While effective, this becomes slow if agents perform CPU-intensive operations or accumulate lengthy oplogs.

Periodic **snapshots** address this — capturing agent memory and filesystem state. Recovery then requires replaying only the oplog portion following the last snapshot. Although automatic snapshotting experiments occurred, this remains unimplemented in current Golem.

**Golem 1.5** introduces a more limited yet arguably more powerful feature for many scenarios.

### User-defined snapshotting

Rather than automatically snapshotting entire agent memory and state, agents may **opt-in** to snapshot support by implementing load/save function pairs. This serializes only relevant state — requiring developer consideration but offering greater control.

These load/save snapshot functions existed since Golem's first release but previously assisted only with version migration when automatic updates weren't feasible.

This example demonstrates manual save/load implementation for the default template's `CounterAgent`:

```typescript
@agent()
class CounterAgent extends BaseAgent {
  private readonly name: string;
  private value: number = 0;
  // ...

  override async saveSnapshot(): Promise<Uint8Array> {
    const snapshot = new Uint8Array(4);
    const view = new DataView(snapshot.buffer);
    view.setUint32(0, this.value);
    return snapshot;
  }

  override async loadSnapshot(bytes: Uint8Array): Promise<void> {
    let view = new DataView(bytes.buffer);
    this.value = view.getUint32(0);
  }
}
```

```rust
#[derive(Serialize, Deserialize)]
struct CounterAgentImpl {
    count: u32,
}

#[agent_implementation()]
impl CounterAgent for CounterAgentImpl {
    // ...

    async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let arr: [u8; 4] = bytes
            .try_into()
            .map_err(|_| "Expected a 4-byte long snapshot")?;
        self.count = u32::from_be_bytes(arr);
        Ok(())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_be_bytes().to_vec())
    }
}
```

```scala
@agentImplementation()
final class SnapshotCounterImpl(@unused private val name: String) extends SnapshotCounter {
  private var value: Int = 0

  // ...

  def saveSnapshot(): Future[Array[Byte]] =
    Future.successful(encodeU32(value))

  def loadSnapshot(bytes: Array[Byte]): Future[Unit] =
    Future.successful {
      value = decodeU32(bytes)
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

```moonbit
///|
/// Counter agent with snapshot persistence
#derive.agent
struct CounterAgent {
  name : String
  mut value : UInt64
}

// ...

impl @agents.Snapshottable for CounterAgent with save_snapshot(self) -> Bytes {
  let snapshot = Bytes::make(8, 0)
  let value = self.value

  snapshot[0] = ((value >> 56) & 0xFF).to_byte()
  snapshot[1] = ((value >> 48) & 0xFF).to_byte()
  snapshot[2] = ((value >> 40) & 0xFF).to_byte()
  snapshot[3] = ((value >> 32) & 0xFF).to_byte()
  snapshot[4] = ((value >> 24) & 0xFF).to_byte()
  snapshot[5] = ((value >> 16) & 0xFF).to_byte()
  snapshot[6] = ((value >> 8) & 0xFF).to_byte()
  snapshot[7] = (value & 0xFF).to_byte()

  snapshot
}

impl @agents.Snapshottable for CounterAgent with load_snapshot(
  self,
  bytes : Bytes,
) -> Result[Unit, String] {
  if bytes.length() != 8 {
    return Err("Invalid snapshot length: expected 8, got " + bytes.length().to_string())
  }

  let value =
    (bytes[0].to_uint64() << 56) |
    (bytes[1].to_uint64() << 48) |
    (bytes[2].to_uint64() << 40) |
    (bytes[3].to_uint64() << 32) |
    (bytes[4].to_uint64() << 24) |
    (bytes[5].to_uint64() << 16) |
    (bytes[6].to_uint64() << 8) |
    bytes[7].to_uint64()

  self.value = value
  Ok(())
}
```

### Recovery configuration

Defining snapshotting function pairs enables **agent updating** but not **snapshot-based recovery**. Configure recovery through agent annotation:

```typescript
@agent({ snapshotting: { periodic: "5s" } })
class CounterAgent extends BaseAgent {
  // ...
}
```

```rust
#[agent_definition(snapshotting = "periodic(5s)")]
trait CounterAgent {
    // ...
}
```

```scala
@agentDefinition(snapshotting = "periodic(5 seconds)")
trait CounterAgent extends BaseAgent {
  // ...
}
```

```moonbit
#derive.agent(snapshotting="periodic(5)")
pub struct CounterAgent {
}
```

Options include `disabled`, `enabled` (using server-side default, initially disabled), `every(N)` (snapshot after every Nth oplog entry), or `periodic(5s)` (snapshot every 5 seconds).

### Default implementation

Manual serialization functions, while powerful, prove burdensome. **Golem 1.5** provides each language a **default snapshotting implementation** mechanism while permitting fully custom load/save method pairs.

```typescript
class CounterAgent extends BaseAgent {
  // For TypeScript, simply NOT defining loadSnapshot and saveSnapshot will
  // provide a default implementation that saves/loads the agent class itself
  // as JSON
}
```

```rust
#[derive(Serialize, Deserialize)]
struct CounterAgentImpl {
    count: u32,
    #[serde(skip)]
    _id: String,
}

#[agent_implementation]
impl CounterAgent for CounterAgentImpl {
    // Not overriding save_snapshot and load_snapshot will provide the
    // default implementation, if the agent type has serde Serialize
    // and Deserialize instances
}
```

```scala
// For Scala we need to explicitly define the state to be serialized
// and mix-in the `Snapshotted[T]` trait

final case class SnapshotCounterState(value: Int)
object SnapshotCounterState {
  implicit val schema: Schema[SnapshotCounterState] = Schema.derived
}

@agentImplementation()
final class CounterAgentImpl(@unused private val name: String)
    extends CounterAgent
    with Snapshotted[SnapshotCounterState] {

  var state: SnapshotCounterState                                 = SnapshotCounterState(0)
  val stateSchema: Schema[SnapshotCounterState] = SnapshotCounterState.schema
}
```

```moonbit
#derive.agent(snapshotting="every_n(1)")
struct Counter {
  name : String
  mut value : UInt64
} derive(ToJson, @json.FromJson)

// If an agent derives ToJson/FromJson and has no manual Snapshottable instance,
// the SDK provides a default implementation
```

### Observability

Using default snapshotting or implementing via `application/json` content type adds a feature: when debugging **agent oplogs**, snapshot entries display serialized JSON state!

Example:

```
#00021:
INVOKE COMPLETED
          at:                2026-04-15T13:19:04.618Z
          consumed fuel:     200796
          result:            AgentMethod(AgentInvocationOutputParameters { output: Tuple(ElementValues { elements: [] }) })
#00022:
SNAPSHOT
          at:                2026-04-15T13:19:04.619Z
          data:              {
  "principal": {
    "tag": "anonymous"
  },
  "state": {
    "name": "test1",
    "value": 5
  },
  "version": 1
}
#00023:
ENQUEUED INVOCATION increment
          at:                2026-04-15T13:19:05.355Z
          idempotency key:   3da50be7-f426-427b-8f50-a05ced00d20a
```
