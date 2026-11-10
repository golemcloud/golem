use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

/// State that the core initializer derives from host inputs. Language runtimes do this for real
/// (Go seeds its scheduler's PRNG from `random_get` during `_initialize`); this hash stands in for
/// that. It is a pure function of the random seed that the initializer reads while the component
/// is instantiated, so every recovery of the same revision must reproduce it.
static INITIALIZER_SEED_HASH: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

#[unsafe(export_name = "_initialize")]
pub extern "C" fn initialize_snapshot_clock() {
    // The reactor initializer runs during core instantiation, before snapshot loading.
    std::hint::black_box(std::time::Instant::now());
    // `RandomState::new` draws its keys from the platform random source.
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u32(0x5eed);
    let _ = INITIALIZER_SEED_HASH.set(hasher.finish());
}

/// The state that the core initializer derived from its random seed.
fn initializer_seed_hash() -> u64 {
    *INITIALIZER_SEED_HASH
        .get()
        .expect("the component initializer did not run")
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
        2
    }

    fn initializer_seed_hash(&self) -> u64 {
        initializer_seed_hash()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_le_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let count = if bytes.len() == 4 {
            u32::from_le_bytes(bytes.try_into().unwrap())
        } else {
            return Err(format!("Invalid snapshot size: {}", bytes.len()));
        };
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let id = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self { count, _id: id })
    }
}

#[agent_definition(snapshotting = "enabled")]
trait JsonSnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get(&self) -> u32;
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
}
