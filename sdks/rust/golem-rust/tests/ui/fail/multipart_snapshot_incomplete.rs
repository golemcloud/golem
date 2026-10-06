use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
trait PartsAgent {
    fn new() -> Self;
}
struct PartsImpl;

#[agent_implementation]
impl PartsAgent for PartsImpl {
    fn new() -> Self {
        Self
    }
    async fn save_snapshot_parts(&self) -> Result<golem_rust::agentic::MultipartSnapshot, String> {
        unreachable!()
    }
}

fn main() {}
