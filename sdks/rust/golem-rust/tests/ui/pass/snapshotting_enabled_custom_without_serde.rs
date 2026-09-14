use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
trait CustomSnapshotAgent {
    fn new(value: u32) -> Self;
    fn value(&self) -> u32;
}

struct CustomSnapshotAgentImpl {
    value: u32,
}

#[agent_implementation]
impl CustomSnapshotAgent for CustomSnapshotAgentImpl {
    fn new(value: u32) -> Self {
        Self { value }
    }

    fn value(&self) -> u32 {
        self.value
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let bytes: [u8; 4] = bytes
            .try_into()
            .map_err(|_| "invalid snapshot length".to_string())?;
        Ok(Self {
            value: u32::from_be_bytes(bytes),
        })
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.value.to_be_bytes().to_vec())
    }
}

fn main() {}
