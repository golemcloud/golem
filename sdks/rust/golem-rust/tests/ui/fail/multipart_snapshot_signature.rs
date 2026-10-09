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
    async fn load_snapshot_parts(
        &self,
        snapshot: golem_rust::agentic::MultipartSnapshot,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        unreachable!()
    }
}

fn main() {}
