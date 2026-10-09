use golem_rust::agentic::{MultipartSnapshot, SnapshotRestoreContext};
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
    async fn save_snapshot_parts(&self) -> Result<MultipartSnapshot, String> {
        unreachable!()
    }
    async fn load_snapshot_parts(
        _: MultipartSnapshot,
        _: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        unreachable!()
    }
    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        unreachable!()
    }
    async fn load_snapshot(_: Vec<u8>, _: SnapshotRestoreContext) -> Result<Self, String> {
        unreachable!()
    }
}

fn main() {}
