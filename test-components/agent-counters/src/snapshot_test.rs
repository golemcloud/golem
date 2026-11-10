use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

use crate::busy_loop;

/// State that the core initializer derives from host inputs. Language runtimes do this for real
/// (Go seeds its scheduler's PRNG from `random_get` during `_initialize`); this state stands in
/// for that. It is a pure function of the random seed and of the environment that the initializer
/// reads while the component is instantiated, so every recovery of the same revision must
/// reproduce it.
static INITIALIZER_STATE: std::sync::OnceLock<InitializerState> = std::sync::OnceLock::new();

struct InitializerState {
    /// A hash of the random seed.
    seed_hash: u64,
    /// The number of environment variables, read through the durable `get-environment` call.
    environment_size: u32,
}

#[unsafe(export_name = "_initialize")]
pub extern "C" fn initialize_snapshot_clock() {
    // The reactor initializer runs during core instantiation, before snapshot loading.
    std::hint::black_box(std::time::Instant::now());
    // `RandomState::new` draws its keys from the platform random source.
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u32(0x5eed);
    let _ = INITIALIZER_STATE.set(InitializerState {
        seed_hash: hasher.finish(),
        environment_size: std::env::vars().count() as u32,
    });
}

fn initializer_state() -> &'static InitializerState {
    INITIALIZER_STATE
        .get()
        .expect("the component initializer did not run")
}

/// The state that the core initializer derived from its random seed.
fn initializer_seed_hash() -> u64 {
    initializer_state().seed_hash
}

/// The number of environment variables that the core initializer read.
fn initializer_environment_size() -> u32 {
    initializer_state().environment_size
}

#[agent_definition(snapshotting = "enabled")]
trait SnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get(&self) -> u32;
    /// Differs between agent-counters and agent-counters-v2, so a replay across
    /// a build change is detectable.
    fn component_version(&self) -> u32;
    /// The state the core initializer derived from its random seed.
    fn initializer_seed_hash(&self) -> u64;
    /// The number of environment variables that the core initializer read.
    fn initializer_environment_size(&self) -> u32;
}

struct SnapshotCounterImpl {
    count: u32,
    _id: String,
}

#[agent_implementation]
impl SnapshotCounter for SnapshotCounterImpl {
    fn new(id: String) -> Self {
        Self { _id: id, count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }

    fn component_version(&self) -> u32 {
        1
    }

    fn initializer_seed_hash(&self) -> u64 {
        initializer_seed_hash()
    }

    fn initializer_environment_size(&self) -> u32 {
        initializer_environment_size()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(count_bytes(self.count))
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let count = count_of(bytes)?;
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let id = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self { count, _id: id })
    }
}

/// A durable counter that snapshots every 10 invocations whatever the executor policy is, so
/// density benchmarks can size their replay work around a fixed snapshot interval.
#[agent_definition(snapshotting = "every(10)")]
trait PeriodicSnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn busy_for(&mut self, millis: u32) -> u32;
    fn oplog_heavy(&mut self, entries: u32) -> u32;
    fn get(&self) -> u32;
    fn was_recovered_from_snapshot(&self) -> bool;
}

struct PeriodicSnapshotCounterImpl {
    count: u32,
    _id: String,
    recovered_from_snapshot: bool,
}

#[agent_implementation]
impl PeriodicSnapshotCounter for PeriodicSnapshotCounterImpl {
    fn new(id: String) -> Self {
        Self {
            _id: id,
            count: 0,
            recovered_from_snapshot: false,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn busy_for(&mut self, millis: u32) -> u32 {
        let _ = busy_loop(millis);
        self.count += 1;
        self.count
    }

    fn oplog_heavy(&mut self, entries: u32) -> u32 {
        for _ in 0..entries {
            let mut buf = [0u8; 4];
            wstd::rand::get_random_bytes(&mut buf);
            self.count = self.count.wrapping_add(u32::from_le_bytes(buf));
        }
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }

    fn was_recovered_from_snapshot(&self) -> bool {
        self.recovered_from_snapshot
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(count_bytes(self.count))
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let count = count_of(bytes)?;
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let id = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self {
            count,
            _id: id,
            recovered_from_snapshot: true,
        })
    }
}

/// The application snapshot of a counter: the count in four little-endian bytes.
fn count_bytes(count: u32) -> Vec<u8> {
    count.to_le_bytes().to_vec()
}

/// The count of an application snapshot that [`count_bytes`] made.
fn count_of(bytes: Vec<u8>) -> Result<u32, String> {
    bytes
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|bytes: Vec<u8>| format!("Invalid snapshot size: {}", bytes.len()))
}

/// An ephemeral counter with snapshot hooks. An ephemeral agent cannot enable snapshots in its
/// definition; the hooks run only if the executor takes a snapshot of the agent.
#[agent_definition(ephemeral)]
trait EphemeralSnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
}

struct EphemeralSnapshotCounterImpl {
    count: u32,
}

#[agent_implementation]
impl EphemeralSnapshotCounter for EphemeralSnapshotCounterImpl {
    fn new(_id: String) -> Self {
        Self { count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(count_bytes(self.count))
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self {
            count: count_of(bytes)?,
        })
    }
}

#[agent_definition(snapshotting = "enabled")]
trait JsonSnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get(&self) -> u32;
    /// The state the core initializer derived from its random seed.
    fn initializer_seed_hash(&self) -> u64;
}

#[derive(Serialize, Deserialize)]
struct JsonSnapshotCounterImpl {
    count: u32,
    #[serde(skip)]
    _id: String,
}

#[agent_implementation]
impl JsonSnapshotCounter for JsonSnapshotCounterImpl {
    fn new(id: String) -> Self {
        Self { _id: id, count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }

    fn initializer_seed_hash(&self) -> u64 {
        initializer_seed_hash()
    }
}
