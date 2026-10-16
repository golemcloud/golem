use golem_rust::agentic::SnapshotRestoreContext;
use golem_rust::{agent_definition, agent_implementation};

/// The byte at `index` of the state of a `LargeOplogEntryAgent`.
fn state_byte(index: u64) -> u8 {
    (index % 251) as u8
}

/// An agent whose oplog entries can be made arbitrarily large: its snapshot holds its whole
/// state, and the durable host call of `read_random_bytes` records all the bytes it returns.
#[agent_definition(snapshotting = "enabled")]
pub trait LargeOplogEntryAgent {
    fn new(name: String) -> Self;

    /// Sets the number of state bytes that the next snapshot holds.
    fn set_state_size(&mut self, size: u64) -> u64;

    fn state_size(&self) -> u64;

    /// Reads `length` random bytes from the host and returns how many it got.
    fn read_random_bytes(&self, length: u64) -> u64;
}

struct LargeOplogEntryAgentImpl {
    state: Vec<u8>,
}

#[agent_implementation]
impl LargeOplogEntryAgent for LargeOplogEntryAgentImpl {
    fn new(_name: String) -> Self {
        Self { state: Vec::new() }
    }

    fn set_state_size(&mut self, size: u64) -> u64 {
        self.state = (0..size).map(state_byte).collect();
        size
    }

    fn state_size(&self) -> u64 {
        self.state.len() as u64
    }

    fn read_random_bytes(&self, length: u64) -> u64 {
        golem_rust::wasip3::random::random::get_random_bytes(length).len() as u64
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.state.clone())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self { state: bytes })
    }
}
