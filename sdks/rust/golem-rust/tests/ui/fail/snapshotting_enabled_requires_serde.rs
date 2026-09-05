use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
trait SnapshottingAgent {
    fn new(value: u32) -> Self;
    fn value(&self) -> u32;
}

struct SnapshottingAgentImpl {
    value: u32,
}

#[agent_implementation]
impl SnapshottingAgent for SnapshottingAgentImpl {
    fn new(value: u32) -> Self {
        Self { value }
    }

    fn value(&self) -> u32 {
        self.value
    }
}

fn main() {}
