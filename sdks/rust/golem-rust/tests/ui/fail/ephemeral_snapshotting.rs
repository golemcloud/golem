use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(ephemeral, snapshotting = "enabled")]
trait EphemeralSnapshotAgent {
    fn new() -> Self;

    fn get_value(&self) -> u32;
}

struct EphemeralSnapshotAgentImpl;

#[agent_implementation]
impl EphemeralSnapshotAgent for EphemeralSnapshotAgentImpl {
    fn new() -> Self {
        Self
    }

    fn get_value(&self) -> u32 {
        0
    }
}

fn main() {}
