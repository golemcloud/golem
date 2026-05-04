---
name: golem-custom-snapshot-moonbit
description: "Implementing custom snapshot save/load functions for MoonBit agents. Use when adding manual update support, snapshot-based recovery, or custom state serialization for a MoonBit Golem agent."
---

# Custom Snapshots in MoonBit

Golem agents can implement the `Snapshottable` trait to support manual (snapshot-based) updates and snapshot-based recovery.

## Automatic JSON Snapshotting (Default)

When the agent struct derives `ToJson` and `@json.FromJson`, the SDK's code generation automatically provides JSON-based snapshotting. Enable it via the `snapshotting` attribute on `#derive.agent`:

```moonbit
#derive.agent(snapshotting="every_n(1)")
struct Counter {
  name : String
  mut value : UInt64
} derive(ToJson, @json.FromJson)

fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

The code generation tool detects `ToJson` and `@json.FromJson` derives and generates a `Snapshottable` implementation that serializes the agent as JSON.

### Snapshotting Modes

The `snapshotting` attribute accepts these values:

| Mode | Example | Description |
|------|---------|-------------|
| (omitted) | `#derive.agent` | Snapshotting disabled |
| Every N | `#derive.agent(snapshotting="every_n(1)")` | Snapshot every N successful invocations |

## Custom Snapshotting

For custom binary serialization or cross-version migration, implement the `Snapshottable` trait manually:

```moonbit
pub(open) trait Snapshottable {
  save_snapshot(Self) -> Bytes
  load_snapshot(Self, Bytes) -> Result[Unit, String]
}
```

### Example

```moonbit
#derive.agent(snapshotting="every_n(1)")
struct Counter {
  name : String
  mut value : UInt64
}

fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}

///|
pub impl @agents.Snapshottable for Counter with save_snapshot(self) {
  // Serialize value as 8 big-endian bytes
  let bytes = Bytes::new(8)
  let v = self.value
  for i in 0..<8 {
    bytes[i] = ((v >> ((7 - i).to_uint64() * 8)).to_int() & 0xff).to_byte()
  }
  bytes
}

///|
pub impl @agents.Snapshottable for Counter with load_snapshot(self, bytes) {
  if bytes.length() != 8 {
    return Err("Expected an 8-byte long snapshot")
  }
  let mut v : UInt64 = 0
  for i in 0..<8 {
    v = v | (bytes[i].to_uint64() << ((7 - i).to_uint64() * 8))
  }
  self.value = v
  Ok(())
}
```

### Method Signatures

```moonbit
// Save: serialize the agent's current state to bytes
save_snapshot(Self) -> Bytes

// Load: restore the agent's state from previously saved bytes
// Return Err to signal the update should fail and the agent should revert
load_snapshot(Self, Bytes) -> Result[Unit, String]
```

## How the SDK Wires Snapshots

The code generation tool (`golem_sdk_tools agents`) produces a `ConstructedAgent` struct for each agent. When snapshotting is enabled:

1. If the agent has `ToJson` + `@json.FromJson` derives, the generated code automatically provides a `Snapshottable` implementation using JSON serialization.
2. If the agent has a manual `impl Snapshottable`, the custom implementation is used instead.
3. The `ConstructedAgent` records the `snapshottable` interface reference and `snapshot_format` (Json or Binary).
4. The SDK's `save-snapshot` and `load-snapshot` WIT exports delegate to these implementations.

## Best Practices

1. **Prefer automatic (JSON) snapshotting** — derive `ToJson` and `@json.FromJson` on the agent struct for zero-effort persistence.
2. **Keep snapshots small** — large snapshots impact recovery and update time.
3. **Version your snapshot format** — include a version byte so `load_snapshot` can handle snapshots from older versions.
4. **Test round-trips** — verify that `save_snapshot` → `load_snapshot` produces equivalent state.
5. **Handle migration** — when the state schema changes between versions, `load_snapshot` in the new version should be able to parse snapshots from the old version.
6. **Return `Err` to reject incompatible snapshots** — `load_snapshot` returning `Err` causes the update to fail gracefully, reverting the agent to the old version.
