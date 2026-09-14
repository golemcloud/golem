use golem_rust::{agent_definition, agent_implementation, description};

#[agent_definition]
#[description("Update test agent V4 - always fails to load snapshot")]
pub trait UpdateTest {
    fn new() -> Self;
}

struct UpdateTestImpl {}

#[agent_implementation]
impl UpdateTest for UpdateTestImpl {
    fn new() -> Self {
        Self {}
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Err("Invalid snapshot - simulating failure".to_string())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotUpdateTest {
    fn new() -> Self;
    fn loaded_snapshot_revision(&self) -> u32;
    fn replay_revision(&self) -> u32;
    fn revision_two_only(&self) -> u32;
}

struct SnapshotUpdateTestImpl;

#[agent_implementation]
impl SnapshotUpdateTest for SnapshotUpdateTestImpl {
    fn new() -> Self {
        Self
    }

    fn loaded_snapshot_revision(&self) -> u32 {
        0
    }

    fn replay_revision(&self) -> u32 {
        4
    }

    fn revision_two_only(&self) -> u32 {
        4
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Err("Invalid snapshot - simulating failure".to_string())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(vec![4])
    }
}

#[agent_definition]
pub trait RevisionEnvAgent {
    fn new() -> Self;
    fn get_revision_from_env_var(&self) -> String;
}

struct RevisionEnvAgentImpl;

#[agent_implementation]
impl RevisionEnvAgent for RevisionEnvAgentImpl {
    fn new() -> Self {
        Self
    }

    fn get_revision_from_env_var(&self) -> String {
        std::env::var("GOLEM_COMPONENT_REVISION").unwrap_or_default()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self)
    }
}
