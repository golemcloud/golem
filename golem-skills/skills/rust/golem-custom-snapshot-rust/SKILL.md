---
name: golem-custom-snapshot-rust
description: "Implementing custom snapshot save/load functions for Rust agents. Use when adding manual update support, snapshot-based recovery, or custom state serialization for a Rust Golem agent."
---

# Custom Snapshots in Rust

Golem agents can implement custom `save_snapshot` and `load_snapshot` functions to support manual (snapshot-based) updates and snapshot-based recovery. This is required when updating agents between incompatible component versions.

## Enabling Snapshotting

Snapshotting must be enabled via the `snapshotting` attribute on `#[agent_definition]`. Without it, no snapshot exports are generated:

```rust
#[agent_definition(mount = "/counters/{name}", snapshotting = "every(1)")]
pub trait CounterAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;
}
```

### Snapshotting Modes

The `snapshotting` attribute accepts these values:

| Mode | Example | Description |
|------|---------|-------------|
| `"disabled"` | (default when omitted) | No snapshotting |
| `"enabled"` | `snapshotting = "enabled"` | Enable snapshot support with the server's default policy. **The server default is `disabled`**, so this may have no effect. Use `"every(N)"` or `"periodic(…)"` to guarantee snapshotting is active. |
| `"every(N)"` | `snapshotting = "every(1)"` | Snapshot every N successful function calls (use `"every(1)"` for every invocation) |
| `"periodic(duration)"` | `snapshotting = "periodic(30s)"` | Snapshot at most once per time interval (uses `humantime` durations) |

```rust
#[agent_definition(mount = "/periodic/{name}", snapshotting = "periodic(30s)")]
pub trait PeriodicAgent { ... }

#[agent_definition(mount = "/batch/{name}", snapshotting = "every(10)")]
pub trait BatchAgent { ... }
```

## Automatic Snapshotting (Default)

If the agent's struct implements `serde::Serialize` and `serde::de::DeserializeOwned`, the SDK automatically provides JSON-based snapshotting — no custom code needed. The `#[agent_implementation]` macro detects `Serialize`/`DeserializeOwned` on the agent type and auto-generates snapshot handlers.

```rust
use serde::{Serialize, Deserialize};
use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/counters/{name}", snapshotting = "every(1)")]
pub trait CounterAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;
}

#[derive(Serialize, Deserialize)]  // This enables automatic snapshotting
struct CounterImpl {
    name: String,
    count: u32,
}

#[agent_implementation(mount = "/counters/{name}")]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self { name, count: 0 }
    }

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
    // No save_snapshot/load_snapshot needed — serde handles it automatically
}
```

## Custom Snapshotting

For custom binary formats, compatibility with non-Rust components, or migration between different state schemas, implement both `save_snapshot` and `load_snapshot` on the agent implementation:

```rust
use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/snapshot-counters/{name}", snapshotting = "every(1)")]
pub trait CounterWithSnapshotAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation(mount = "/snapshot-counters/{name}")]
impl CounterWithSnapshotAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32 {
        self.count += 1;
        log::info!("The new value is {}", self.count);
        self.count
    }

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

### Rules

- **Both `save_snapshot` and `load_snapshot` must be implemented together**, or neither. The macro enforces this at compile time.
- When custom implementations are present, automatic serde-based snapshotting is bypassed.
- `save_snapshot` returns `Result<Vec<u8>, String>` — the bytes are the snapshot payload.
- `load_snapshot` receives `Vec<u8>` and returns `Result<(), String>` — it must restore the agent's state from the bytes.
- Both methods are `async` — they can perform asynchronous operations during serialization/deserialization.
- Returning `Err` from `load_snapshot` causes the update to fail and the agent reverts to the old version.

## Method Signatures

```rust
// Save: serialize the agent's current state to bytes
async fn save_snapshot(&self) -> Result<Vec<u8>, String>

// Load: restore the agent's state from previously saved bytes
async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String>
```

## Best Practices

1. **Prefer automatic (serde) snapshotting** unless you need a compact binary format or cross-version migration logic.
2. **Keep snapshots small** — large snapshots impact recovery and update time.
3. **Version your snapshot format** — include a version byte or tag so `load_snapshot` can handle snapshots from older versions.
4. **Test round-trips** — verify that `save_snapshot` → `load_snapshot` produces equivalent state.
5. **Handle migration** — when the state schema changes between versions, `load_snapshot` in the new version should be able to parse snapshots from the old version.

## Project Template

A ready-made project with snapshotting can be created using:

```shell
golem new --language rust --template snapshotting my-project
```
