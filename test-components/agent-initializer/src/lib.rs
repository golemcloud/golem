//! A component whose core initializer differs by revision, for the never-live tests. The base
//! revision's initializer reads the agent's environment. Each variant, chosen by
//! `INITIALIZER_VARIANT` at compile time, is a later revision that does the same read and then:
//!
//! - `key-value`: reads the key `PEER_PORT` of the bucket `snapshot-load`, a remote read. HTTP
//!   cannot run here: the core initializer is a synchronous task, which may not wait for a P3
//!   send;
//! - `panic`: panics.
//!
//! A variant acts only in an agent that has `PEER_PORT`. The component analysis of an upload
//! runs the initializer without it, so every revision can be uploaded.

use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

const VARIANT: Option<&str> = option_env!("INITIALIZER_VARIANT");

#[ctor::ctor]
fn initialize() {
    let Ok(port) = std::env::var("PEER_PORT") else {
        return;
    };
    match VARIANT.unwrap_or("base") {
        "key-value" => read_key(&port),
        "panic" => panic!("the initializer of this revision panics"),
        _ => {}
    }
}

/// Reads the key `key` of the bucket `snapshot-load` with a synchronous key-value call.
fn read_key(key: &str) {
    use golem_rust::bindings::wasi::keyvalue::eventual::{Bucket, get};
    if let Ok(bucket) = Bucket::open_bucket("snapshot-load") {
        let _ = get(&bucket, key);
    }
}

#[agent_definition]
pub trait Initialized {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
}

pub struct InitializedImpl {
    count: u32,
}

#[agent_implementation]
impl Initialized for InitializedImpl {
    fn new(_name: String) -> Self {
        Self { count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

/// Takes snapshots with the executor's default policy.
#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotInitialized {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
    /// Panics in a restored agent: a failure of the target code after a load.
    fn fail_if_restored(&self) -> u32;
}

#[derive(Serialize, Deserialize)]
pub struct SnapshotInitializedImpl {
    count: u32,
    #[serde(skip)]
    restored: bool,
}

#[agent_implementation]
impl SnapshotInitialized for SnapshotInitializedImpl {
    fn new(_name: String) -> Self {
        Self {
            count: 0,
            restored: false,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn fail_if_restored(&self) -> u32 {
        assert!(!self.restored, "the restored agent fails");
        self.count
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| error.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let mut state: Self = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        state.restored = true;
        Ok(state)
    }
}
